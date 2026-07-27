//! RGBA→NV12 on the GPU.
//!
//! ## Why this exists
//!
//! The export path used to composite on the GPU, read 8 MB of RGBA back to the
//! CPU, and hand it to swscale to make 3 MB of NV12 for the hardware encoder to
//! upload. Measured on the Raptor Lake iGPU, that colour conversion and the
//! readback it forced were the largest single item in a 1080p export frame
//! after decoding — see `docs/research/zero-copy-encode.md`.
//!
//! This module deletes the swscale pass and halves the readback: a compute
//! shader reads the composited target and writes the two NV12 planes into one
//! storage buffer, and only 1.5 bytes per pixel cross the bus instead of 4.
//!
//! It is deliberately *not* the zero-copy path. The bytes still come back to
//! system memory and are still uploaded into a VA surface. It is the step that
//! is worth doing on its own, needs no DMA-BUF, no DRM format modifiers and no
//! `ash`, and works on any device wgpu will talk to.
//!
//! ## The layout that comes out
//!
//! One buffer, luma then chroma, each plane's rows padded out to
//! [`ROW_ALIGN`]:
//!
//! ```text
//!   0                        y_stride * height
//!   ├── Y plane ─────────────┼── UV plane ──────────────┤
//!       height rows              height/2 rows
//!       y_stride bytes each      uv_stride bytes each
//! ```
//!
//! Both strides are the same, and both are wider than the frame for most sizes,
//! so a row is `width` bytes of picture followed by padding nobody reads.
//! Everything downstream is handed the stride explicitly and none of it may
//! assume `stride == width` — that assumption is what [`ROW_ALIGN`] documents
//! the cost of.

use std::sync::atomic::AtomicU64;

use parking_lot::Mutex;

use super::context::RenderContext;
use super::error::{RenderError, Result};
use super::source::YuvRange;
use super::texture_pool::PooledTexture;

/// The format the compute pass reads. A view in this format over the sRGB
/// render target yields the stored bytes rather than linearised floats, which
/// is the only way this can agree with what swscale used to be handed.
pub const READ_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Matches `@workgroup_size` in `shaders/nv12.wgsl`.
const WORKGROUP: (u32, u32) = (8, 8);

/// Pixels one invocation converts, also from the shader. Four across so each
/// luma write is one aligned word; two down so each invocation produces exactly
/// one chroma word.
const BLOCK: (u32, u32) = (4, 2);

/// NV12 in system memory, with its strides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nv12Frame {
    pub width: u32,
    pub height: u32,
    pub y_stride: usize,
    pub uv_stride: usize,
    /// The Y plane followed by the UV plane. Use [`Self::y`] and [`Self::uv`]
    /// rather than slicing this by hand.
    pub data: Vec<u8>,
}

impl Nv12Frame {
    /// Byte offset of the chroma plane.
    pub fn uv_offset(&self) -> usize {
        self.y_stride * self.height as usize
    }

    pub fn y(&self) -> &[u8] {
        &self.data[..self.uv_offset()]
    }

    pub fn uv(&self) -> &[u8] {
        &self.data[self.uv_offset()..]
    }

    /// The luma byte at `(x, y)`. For tests and for anyone debugging a green
    /// frame.
    pub fn luma(&self, x: u32, y: u32) -> u8 {
        self.data[y as usize * self.y_stride + x as usize]
    }

    /// The `(Cb, Cr)` pair covering the 2x2 block at `(x, y)` in luma
    /// coordinates.
    pub fn chroma(&self, x: u32, y: u32) -> (u8, u8) {
        let base = self.uv_offset() + (y as usize / 2) * self.uv_stride + (x as usize / 2) * 2;
        (self.data[base], self.data[base + 1])
    }
}

/// Plane geometry for one frame size. Pure arithmetic, so it is tested without
/// a GPU — getting a stride wrong is how NV12 comes out sheared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nv12Layout {
    pub width: u32,
    pub height: u32,
    pub y_stride: usize,
    pub uv_stride: usize,
    pub uv_rows: usize,
}

/// What a row of either plane is padded to.
///
/// Four would be enough for the shader (one aligned word per luma write) and
/// enough for libavutil, and that is what this was for months. It is not enough
/// for the **encoder**. A VAAPI surface imported from a DMA-BUF whose pitch is
/// not a multiple of 64 encodes as vertical strips displaced against each
/// other, on Intel, silently — a 4:3 clip gives a 1440-wide canvas, and that is
/// how this was found.
///
/// The subtle part, and the reason it took a while: the *importer* honours
/// whatever pitch it is handed. `av_hwframe_transfer_data` reads such a surface
/// back byte-for-byte correct, so a round-trip test proves nothing about it —
/// `export::hwframes`' own tests pass at 1440x1080 while real exports at that
/// size were destroyed. Only the encode engine cares, and it does not complain;
/// it reads the plane with the pitch it wanted and encodes the result.
///
/// Measured on this hardware, zero-copy against the readback path, luma PSNR,
/// before and after this constant existed:
///
/// | size      | width mod 64 | before   | after    |
/// |-----------|--------------|----------|----------|
/// | 1920x1080 | 0            | 79.6 dB  | 79.6 dB  |
/// | 1408x1080 | 0            | 78.2 dB  | —        |
/// | 1440x1080 | 32           | 19.2 dB  | 78.1 dB  |
/// | 1360x768  | 16           | 17.9 dB  | infinite |
///
/// 128 rather than 64 because it costs at most 127 bytes a row — 0.1% of a
/// 1440x1080 frame — and covers drivers that ask for more than 64.
const ROW_ALIGN: usize = 128;

impl Nv12Layout {
    pub fn for_size(width: u32, height: u32) -> Self {
        let stride = (width as usize).div_ceil(ROW_ALIGN) * ROW_ALIGN;
        Self {
            width,
            height,
            y_stride: stride,
            uv_stride: stride,
            uv_rows: (height as usize).div_ceil(2),
        }
    }

    pub fn uv_offset(&self) -> usize {
        self.y_stride * self.height as usize
    }

    pub fn total_bytes(&self) -> usize {
        self.uv_offset() + self.uv_stride * self.uv_rows
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    width: u32,
    height: u32,
    y_stride_words: u32,
    uv_offset_words: u32,
    uv_stride_words: u32,
    range: u32,
    _pad: [u32; 2],
}

// `#[repr(C)]`, eight `u32`s, no padding of its own — plain old data. Hand
// written for the same reason `QuadUniform`'s impls are: no dependency on
// `bytemuck`'s derive feature.
unsafe impl bytemuck::Zeroable for Params {}
unsafe impl bytemuck::Pod for Params {}

/// The compute pipeline and its reusable buffers.
pub struct Nv12Converter {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    buffers: Mutex<Buffers>,
    /// Nanoseconds spent blocked on the GPU, for the compositor's stats.
    pub(super) wait_ns: AtomicU64,
}

#[derive(Default)]
struct Buffers {
    /// What the shader writes. Device-local.
    storage: Option<wgpu::Buffer>,
    /// What the CPU maps. Grown, never shrunk, exactly like the RGBA readback.
    staging: Option<wgpu::Buffer>,
    params: Option<wgpu::Buffer>,
    capacity: u64,
}

impl Nv12Converter {
    pub fn new(ctx: &RenderContext) -> Self {
        let device = ctx.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chukcut nv12 shader"),
            // `yuv.wgsl` carries both colour conversions and no entry point, so
            // it is prepended rather than imported — WGSL has no `#include`
            // and naga drops whatever the module does not reach.
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("shaders/yuv.wgsl"),
                    include_str!("shaders/nv12.wgsl")
                )
                .into(),
            ),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut nv12 bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<Params>() as u64
                        ),
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chukcut nv12 pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("chukcut nv12 pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("convert"),
            compilation_options: Default::default(),
            cache: None,
        });

        Self {
            pipeline,
            layout,
            buffers: Mutex::new(Buffers::default()),
            wait_ns: AtomicU64::new(0),
        }
    }

    /// Convert `target` and read the planes back, in limited range.
    ///
    /// `target` must have been created viewable as [`READ_FORMAT`]; a texture
    /// that was not is refused rather than silently converted through the sRGB
    /// view, which would produce a washed-out picture that still encodes.
    pub fn convert(&self, ctx: &RenderContext, target: &PooledTexture) -> Result<Nv12Frame> {
        self.convert_range(ctx, target, YuvRange::Limited)
    }

    /// [`Self::convert`], saying which range the samples are wanted in.
    ///
    /// A video encoder wants [`YuvRange::Limited`] and a JPEG encoder wants
    /// [`YuvRange::Full`]; getting it wrong produces a picture that is valid,
    /// plausible and washed out. See `luma_in` in `shaders/yuv.wgsl`.
    pub fn convert_range(
        &self,
        ctx: &RenderContext,
        target: &PooledTexture,
        range: YuvRange,
    ) -> Result<Nv12Frame> {
        let layout = Nv12Layout::for_size(target.width(), target.height());
        let total = layout.total_bytes() as u64;

        let device = ctx.device();
        let mut buffers = self.buffers.lock();
        buffers.ensure(device, total);
        let storage = buffers.storage.as_ref().expect("just ensured").clone();
        let staging = buffers.staging.as_ref().expect("just ensured").clone();
        drop(buffers);

        // Convert, then copy into something mappable in the same submission —
        // one round trip to the GPU rather than two.
        self.dispatch(ctx, target, &storage, layout, range, |encoder| {
            encoder.copy_buffer_to_buffer(&storage, 0, &staging, 0, total);
        })?;

        let slice = staging.slice(0..total);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let waited = std::time::Instant::now();
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|_| RenderError::Readback("map callback never fired".into()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        self.wait_ns.fetch_add(
            waited.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Unmapped unconditionally, for the same reason as the RGBA path: a
        // buffer left mapped can never be mapped again and this one is reused
        // for every frame of the export.
        let copied = match slice.get_mapped_range() {
            Ok(view) => Ok(view.to_vec()),
            Err(e) => Err(RenderError::Readback(e.to_string())),
        };
        staging.unmap();

        Ok(Nv12Frame {
            width: target.width(),
            height: target.height(),
            y_stride: layout.y_stride,
            uv_stride: layout.uv_stride,
            data: copied?,
        })
    }

    /// Convert `target` straight into `destination` and stop there.
    ///
    /// The zero-copy entry point: `destination` is a buffer whose memory has
    /// been exported as a DMA-BUF, so what this writes is what the encoder
    /// reads. Nothing comes back to system memory.
    ///
    /// Blocks until the GPU has finished, because libva cannot be handed a
    /// Vulkan semaphore and the importer on the other side has no other way to
    /// know the pixels are there. That stall is the price of the whole
    /// arrangement and it is still cheaper than the three copies it replaces.
    pub fn convert_into(
        &self,
        ctx: &RenderContext,
        target: &PooledTexture,
        destination: &wgpu::Buffer,
    ) -> Result<Nv12Layout> {
        self.convert_into_range(ctx, target, destination, YuvRange::Limited)
    }

    /// [`Self::convert_into`], saying which range the samples are wanted in.
    ///
    /// The preview's JPEG encoder is the caller that wants [`YuvRange::Full`].
    pub fn convert_into_range(
        &self,
        ctx: &RenderContext,
        target: &PooledTexture,
        destination: &wgpu::Buffer,
        range: YuvRange,
    ) -> Result<Nv12Layout> {
        let layout = Nv12Layout::for_size(target.width(), target.height());
        if destination.size() < layout.total_bytes() as u64 {
            return Err(RenderError::Readback(format!(
                "a {}x{} NV12 frame needs {} bytes and the exported buffer holds {}",
                target.width(),
                target.height(),
                layout.total_bytes(),
                destination.size()
            )));
        }

        self.dispatch(ctx, target, destination, layout, range, |_| {})?;

        let waited = std::time::Instant::now();
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        self.wait_ns.fetch_add(
            waited.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        Ok(layout)
    }

    /// Record and submit the compute pass writing `target` into `destination`.
    ///
    /// `also` gets the same encoder afterwards, so a caller that wants a copy
    /// out of `destination` pays for one submission rather than two.
    fn dispatch(
        &self,
        ctx: &RenderContext,
        target: &PooledTexture,
        destination: &wgpu::Buffer,
        layout: Nv12Layout,
        range: YuvRange,
        also: impl FnOnce(&mut wgpu::CommandEncoder),
    ) -> Result<()> {
        let (width, height) = (target.width(), target.height());

        let view = target.view_as(READ_FORMAT).ok_or_else(|| {
            RenderError::Readback(format!(
                "a {:?} render target cannot be read as {READ_FORMAT:?}; it was not created \
                 with that view format",
                target.format()
            ))
        })?;

        let device = ctx.device();
        let mut buffers = self.buffers.lock();
        buffers.ensure_params(device);
        let params_buffer = buffers.params.as_ref().expect("just ensured").clone();
        drop(buffers);

        let params = Params {
            width,
            height,
            y_stride_words: (layout.y_stride / 4) as u32,
            uv_offset_words: (layout.uv_offset() / 4) as u32,
            uv_stride_words: (layout.uv_stride / 4) as u32,
            range: range as u32,
            _pad: [0; 2],
        };
        ctx.queue()
            .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut nv12 bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: destination.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("chukcut nv12"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chukcut nv12 convert"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            let groups_x = width.div_ceil(BLOCK.0).div_ceil(WORKGROUP.0);
            let groups_y = height.div_ceil(BLOCK.1).div_ceil(WORKGROUP.1);
            pass.dispatch_workgroups(groups_x, groups_y, 1);
        }
        also(&mut encoder);
        ctx.queue().submit(Some(encoder.finish()));
        Ok(())
    }
}

impl Buffers {
    fn ensure_params(&mut self, device: &wgpu::Device) {
        if self.params.is_none() {
            self.params = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chukcut nv12 params"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
    }

    fn ensure(&mut self, device: &wgpu::Device, size: u64) {
        self.ensure_params(device);
        if self.capacity >= size && self.storage.is_some() {
            return;
        }
        // `COPY_BUFFER_ALIGNMENT` is 4 and every plane stride is already a
        // multiple of it, but round anyway: a future stride rule that is not
        // would otherwise fail validation inside `copy_buffer_to_buffer`
        // instead of here.
        let size = size.next_power_of_two().max(256);
        self.storage = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chukcut nv12 planes"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        self.staging = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chukcut nv12 staging"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));
        self.capacity = size;
    }
}

impl std::fmt::Debug for Nv12Converter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nv12Converter").finish()
    }
}

/// BT.601 limited-range RGB→YUV on the CPU, for the tests.
///
/// Deliberately a second implementation of the shader's arithmetic rather than
/// a shared helper: a test that reuses the code under test only proves the code
/// is consistent with itself.
#[cfg(test)]
pub(crate) fn reference_yuv(rgb: [u8; 3]) -> (f32, f32, f32) {
    reference_yuv_in(rgb, YuvRange::Limited)
}

/// The same, in whichever range. [`YuvRange::Full`] is the JFIF matrix, which
/// is what a JPEG encoder wants.
#[cfg(test)]
pub(crate) fn reference_yuv_in(rgb: [u8; 3], range: YuvRange) -> (f32, f32, f32) {
    let (r, g, b) = (
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
    );
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
    let cr = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
    match range {
        YuvRange::Limited => (16.0 + 219.0 * luma, 128.0 + 224.0 * cb, 128.0 + 224.0 * cr),
        YuvRange::Full => (
            255.0 * luma,
            (128.0 + 255.0 * cb).clamp(0.0, 255.0),
            (128.0 + 255.0 * cr).clamp(0.0, 255.0),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::render::texture_pool::{TextureKey, TexturePool};

    /// The invariant a hardware export depends on, checked without a GPU.
    ///
    /// This is deliberately an assertion about *alignment* and not about a
    /// particular number of bytes: the failure it guards against is somebody
    /// tightening `ROW_ALIGN` back down to save memory, at which point exports
    /// at any width that is not a multiple of it come out as displaced vertical
    /// strips, on hardware, silently, and only at some resolutions. See
    /// [`ROW_ALIGN`] for the measurements.
    #[test]
    fn every_stride_is_aligned_for_the_encoder() {
        // Real canvases: adopted from 4:3, 16:9 and vertical sources, plus odd
        // and prime-ish widths that no alignment rule can accidentally satisfy.
        for (width, height) in [
            (1920, 1080),
            (1440, 1080),
            (1360, 768),
            (1080, 1920),
            (1616, 1080),
            (720, 540),
            (2, 2),
            (1, 1),
            (1919, 1079),
            (997, 601),
        ] {
            let layout = Nv12Layout::for_size(width, height);
            assert_eq!(
                layout.y_stride % ROW_ALIGN,
                0,
                "{width}x{height} luma stride {} is not {ROW_ALIGN}-aligned",
                layout.y_stride
            );
            assert_eq!(
                layout.uv_stride % ROW_ALIGN,
                0,
                "{width}x{height} chroma stride {} is not {ROW_ALIGN}-aligned",
                layout.uv_stride
            );
            // A stride narrower than the picture would truncate every row, and
            // the plane offset has to stay aligned too or the chroma plane
            // starts mid-row.
            assert!(
                layout.y_stride >= width as usize,
                "{width}x{height} stride {} is narrower than the frame",
                layout.y_stride
            );
            assert_eq!(layout.uv_offset() % ROW_ALIGN, 0);
            assert!(layout.total_bytes() >= layout.uv_offset() + layout.uv_stride * layout.uv_rows);
        }
    }

    /// Convert a solid `width` x `height` block of one colour, on the GPU.
    ///
    /// Uses a `Rgba8Unorm` texture rather than the compositor's sRGB one so the
    /// bytes written are the bytes the shader sees, and this test is about the
    /// arithmetic. `srgb_target_is_read_as_stored_bytes` covers the
    /// reinterpretation separately.
    fn convert_solid(colour: [u8; 4], width: u32, height: u32) -> Option<Nv12Frame> {
        convert_solid_in(colour, width, height, YuvRange::Limited)
    }

    fn convert_solid_in(
        colour: [u8; 4],
        width: u32,
        height: u32,
        range: YuvRange,
    ) -> Option<Nv12Frame> {
        let ctx = crate::modules::render::test_context()?;
        let pool = TexturePool::default();
        let target = pool.acquire(
            ctx.device(),
            TextureKey::new(
                width,
                height,
                READ_FORMAT,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            ),
        );

        let pixels: Vec<u8> = colour
            .iter()
            .copied()
            .cycle()
            .take(width as usize * height as usize * 4)
            .collect();
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let converter = Nv12Converter::new(&ctx);
        Some(
            converter
                .convert_range(&ctx, &target, range)
                .expect("convert"),
        )
    }

    fn assert_matches_reference(frame: &Nv12Frame, colour: [u8; 4]) {
        let (y, cb, cr) = reference_yuv([colour[0], colour[1], colour[2]]);
        for row in 0..frame.height {
            for col in 0..frame.width {
                let got = frame.luma(col, row) as f32;
                assert!(
                    (got - y).abs() <= 1.0,
                    "luma at {col},{row}: got {got}, want {y}"
                );
            }
        }
        let (got_cb, got_cr) = frame.chroma(0, 0);
        assert!((got_cb as f32 - cb).abs() <= 1.0, "Cb {got_cb} want {cb}");
        assert!((got_cr as f32 - cr).abs() <= 1.0, "Cr {got_cr} want {cr}");
    }

    #[test]
    fn solid_colours_convert_to_the_textbook_values() {
        // Each of these is a value someone can check against a table, which is
        // the point: a shader that is subtly wrong still produces a plausible
        // picture, and "it encoded" is not evidence.
        for colour in [
            [0u8, 0, 0, 255],
            [255, 255, 255, 255],
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [37, 211, 102, 255],
        ] {
            let Some(frame) = convert_solid(colour, 64, 32) else {
                eprintln!("skipping: no GPU adapter");
                return;
            };
            assert_matches_reference(&frame, colour);
        }
    }

    /// The test that would catch a limited-range JPEG, which is the expensive
    /// bug this file's range parameter exists to make impossible.
    ///
    /// A JPEG file carries no range tag and every decoder reads it as 0..255.
    /// Encoding limited-range samples produces grey blacks and no white — a
    /// valid, plausible, washed-out picture that scores about 27 dB against the
    /// software encoder instead of 37. Black at 16 and white at 235 is what that
    /// looks like from here.
    #[test]
    fn full_range_puts_black_at_zero_and_white_at_255() {
        let Some(black) = convert_solid_in([0, 0, 0, 255], 64, 32, YuvRange::Full) else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        assert_eq!(black.luma(0, 0), 0, "full-range black must be 0, not 16");
        assert_eq!(black.chroma(0, 0), (128, 128), "black is colourless");

        let white = convert_solid_in([255, 255, 255, 255], 64, 32, YuvRange::Full).expect("white");
        assert_eq!(white.luma(0, 0), 255, "full-range white must be 255, not 235");
        assert_eq!(white.chroma(0, 0), (128, 128), "white is colourless");

        // And the other arm is untouched, because the export depends on it.
        let black = convert_solid_in([0, 0, 0, 255], 64, 32, YuvRange::Limited).expect("limited");
        assert_eq!(black.luma(0, 0), 16, "limited-range black is 16");
    }

    /// The compute pass and the CPU converter it replaces must agree.
    ///
    /// `preview::vaapi::rgba_to_nv12` is the full-range integer transform the
    /// hardware JPEG encoder was fed for months, and it is checked against the
    /// JFIF table by its own tests. Comparing against it is therefore a check
    /// against a *known-good* implementation rather than against a second copy
    /// of the same arithmetic — the two are written differently (float on the
    /// GPU, 8-bit fixed point on the CPU) and share nothing.
    #[test]
    fn full_range_agrees_with_the_cpu_converter_the_jpeg_encoder_used() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // Not a smooth gradient: a flat ramp hides a chroma-block offset, and
        // hard edges are where a 2x2 box average can disagree.
        let (width, height) = (64u32, 48u32);
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let noise = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) as u8;
                rgba.extend_from_slice(&[
                    (x * 255 / width) as u8,
                    noise,
                    if (x / 3 + y / 5) % 2 == 0 { 255 } else { 12 },
                    255,
                ]);
            }
        }

        let pool = TexturePool::default();
        let target = pool.acquire(
            ctx.device(),
            TextureKey::new(
                width,
                height,
                READ_FORMAT,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            ),
        );
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let gpu = Nv12Converter::new(&ctx)
            .convert_range(&ctx, &target, YuvRange::Full)
            .expect("convert");

        let (w, h) = (width as usize, height as usize);
        let mut y = vec![0u8; w * h];
        let mut uv = vec![0u8; w * h / 2];
        crate::modules::preview::vaapi::rgba_to_nv12(&rgba, w, h, &mut y, w, &mut uv, w);

        // One code value: the GPU works in floats and the CPU in 8-bit fixed
        // point, so they round differently and nothing more.
        for row in 0..height {
            for col in 0..width {
                let (got, want) = (gpu.luma(col, row), y[row as usize * w + col as usize]);
                assert!(
                    got.abs_diff(want) <= 1,
                    "luma at {col},{row}: GPU {got}, CPU {want}"
                );
            }
        }
        for row in (0..height).step_by(2) {
            for col in (0..width).step_by(2) {
                let (cb, cr) = gpu.chroma(col, row);
                let base = (row as usize / 2) * w + col as usize;
                assert!(
                    cb.abs_diff(uv[base]) <= 1 && cr.abs_diff(uv[base + 1]) <= 1,
                    "chroma at {col},{row}: GPU ({cb}, {cr}), CPU ({}, {})",
                    uv[base],
                    uv[base + 1]
                );
            }
        }
    }

    #[test]
    fn chroma_is_the_average_of_its_two_by_two_block() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // A 2x2 checkerboard of red and blue: every chroma sample covers one of
        // each, so the answer is the conversion of the average colour rather
        // than either input. A shader that converted first and averaged after
        // would land somewhere else.
        let (width, height) = (4u32, 4u32);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                if (x + y) % 2 == 0 {
                    pixels.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    pixels.extend_from_slice(&[0, 0, 255, 255]);
                }
            }
        }

        let pool = TexturePool::default();
        let target = pool.acquire(
            ctx.device(),
            TextureKey::new(
                width,
                height,
                READ_FORMAT,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            ),
        );
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let frame = Nv12Converter::new(&ctx)
            .convert(&ctx, &target)
            .expect("convert");

        // Half red, half blue.
        let (want_cb, want_cr) = {
            let (_, cb_r, cr_r) = reference_yuv([255, 0, 0]);
            let (_, cb_b, cr_b) = reference_yuv([0, 0, 255]);
            ((cb_r + cb_b) / 2.0, (cr_r + cr_b) / 2.0)
        };
        let (cb, cr) = frame.chroma(0, 0);
        assert!((cb as f32 - want_cb).abs() <= 1.5, "Cb {cb} want {want_cb}");
        assert!((cr as f32 - want_cr).abs() <= 1.5, "Cr {cr} want {want_cr}");

        // Luma is per pixel and must still alternate.
        let (y_red, ..) = reference_yuv([255, 0, 0]);
        let (y_blue, ..) = reference_yuv([0, 0, 255]);
        assert!((frame.luma(0, 0) as f32 - y_red).abs() <= 1.0);
        assert!((frame.luma(1, 0) as f32 - y_blue).abs() <= 1.0);
    }

    #[test]
    fn an_srgb_target_is_read_as_its_stored_bytes() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // This is the trap the whole `view_format` machinery exists for. If the
        // compute pass sampled the sRGB view it would see linearised floats and
        // a mid-grey would come out at luma ~70 instead of ~126.
        let (width, height) = (8u32, 8u32);
        let pool = TexturePool::default();
        let target = pool.acquire(
            ctx.device(),
            TextureKey::new(
                width,
                height,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            )
            .viewable_as(READ_FORMAT),
        );
        let pixels: Vec<u8> = [128u8, 128, 128, 255]
            .iter()
            .copied()
            .cycle()
            .take((width * height * 4) as usize)
            .collect();
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let frame = Nv12Converter::new(&ctx)
            .convert(&ctx, &target)
            .expect("convert");
        let (want, ..) = reference_yuv([128, 128, 128]);
        let got = frame.luma(0, 0) as f32;
        assert!(
            (got - want).abs() <= 1.0,
            "mid grey read as luma {got}, want {want} — the sRGB view leaked in"
        );
    }

    #[test]
    fn a_target_without_the_view_format_is_refused_rather_than_wrong() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let pool = TexturePool::default();
        let target = pool.acquire(
            ctx.device(),
            TextureKey::new(
                8,
                8,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            ),
        );
        assert!(Nv12Converter::new(&ctx).convert(&ctx, &target).is_err());
    }

    #[test]
    fn the_layout_pads_rows_to_whole_words() {
        // 1920 is already a multiple of `ROW_ALIGN`, so this one size still
        // needs no padding — which is exactly why it was the only size the
        // hardware export was ever tested at, and why the bug survived.
        let hd = Nv12Layout::for_size(1920, 1080);
        assert_eq!(hd.y_stride, 1920);
        assert_eq!(hd.uv_stride, 1920);
        assert_eq!(hd.uv_rows, 540);
        assert_eq!(hd.total_bytes(), 1920 * 1080 * 3 / 2);

        // The size a 4:3 clip produces. Padded up from 1440, and the whole
        // reason `ROW_ALIGN` exists.
        let four_three = Nv12Layout::for_size(1440, 1080);
        assert_eq!(four_three.y_stride, 1536);
        assert_eq!(four_three.total_bytes(), 1536 * 1080 + 1536 * 540);

        // 101 is the awkward case the RGBA readback test uses.
        let odd = Nv12Layout::for_size(101, 37);
        assert_eq!(odd.y_stride, 128);
        assert_eq!(odd.uv_rows, 19);
        assert_eq!(odd.total_bytes(), 128 * 37 + 128 * 19);
    }

    #[test]
    fn the_reference_matches_the_well_known_values() {
        // The three checks anyone debugging a green frame reaches for first.
        let (y, cb, cr) = reference_yuv([0, 0, 0]);
        assert!((y - 16.0).abs() < 0.5, "black luma {y}");
        assert!((cb - 128.0).abs() < 0.5 && (cr - 128.0).abs() < 0.5);

        let (y, _, _) = reference_yuv([255, 255, 255]);
        assert!((y - 235.0).abs() < 0.5, "white luma {y}");

        // Pure red is the classic: Y 81, Cb 90, Cr 240 in BT.601 limited.
        let (y, cb, cr) = reference_yuv([255, 0, 0]);
        assert!((y - 81.0).abs() < 1.0, "red luma {y}");
        assert!((cb - 90.0).abs() < 1.0, "red Cb {cb}");
        assert!((cr - 240.0).abs() < 1.0, "red Cr {cr}");
    }
}
