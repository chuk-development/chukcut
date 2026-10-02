//! Does a decoded VA surface actually import into wgpu as a texture?
//!
//! ```bash
//! cargo run --release --example dmabuf_import -- clip.mp4
//! ```
//!
//! `media::dmabuf` produces DRM descriptors and asserts nothing about whether
//! anything can consume them. This is the other half: it opens a wgpu device,
//! imports the descriptors through `wgpu-hal`'s `texture_from_dmabuf_fd`, reads
//! the result back and compares it against the same frame decoded in software.
//!
//! It lives in `examples/` rather than in `modules/render/` on purpose — the
//! import belongs in `render/`, and this is the experiment that establishes what
//! that code has to look like before it is written there. `docs/research/
//! hardware-decode.md` quotes its output and carries the patch.
//!
//! The question it answers is narrow and load-bearing: FFmpeg exports NV12 as
//! **two** DRM layers and `texture_from_dmabuf_fd` is single-plane, so either
//! two textures import from one buffer at two offsets — in which case the
//! compositor grows a YUV shader — or they do not, and the conversion has to
//! happen before the export. Guessing costs a week either way.

use std::path::PathBuf;

use chukcut_engine::modules::media::decoder::Acceleration;
use chukcut_engine::modules::media::dmabuf::Plane;
use chukcut_engine::modules::media::VideoDecoder;

fn main() {
    let file = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            eprintln!("usage: dmabuf_import FILE");
            std::process::exit(2);
        });
    let at: i64 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(500_000);

    let Some(gpu) = Gpu::open() else {
        println!("no Vulkan device with VK_EXT_external_memory_dma_buf; nothing to try");
        return;
    };
    println!(
        "device: {} ({:?}), dma-buf import feature: {}",
        gpu.info.name, gpu.info.backend, gpu.can_import
    );
    if !gpu.can_import {
        println!("the adapter does not advertise VULKAN_EXTERNAL_MEMORY_DMA_BUF; stopping");
        return;
    }

    let mut decoder = match VideoDecoder::open_with(&file, Acceleration::Vaapi) {
        Ok(decoder) => decoder,
        Err(error) => {
            println!("cannot hardware-decode {}: {error}", file.display());
            return;
        }
    };
    let mapped = match decoder.seek_and_map(at) {
        Ok(mapped) => mapped,
        Err(error) => {
            println!("cannot map a surface: {error}");
            return;
        }
    };
    println!("{:?}", mapped.dmabuf);

    let planes = mapped.dmabuf.planes();
    let mut read: Vec<Option<Vec<u8>>> = Vec::new();
    for (index, plane) in planes.iter().enumerate() {
        let format = wgpu_format(plane);
        print!(
            "plane {index} `{}` {}x{} as {format:?}: ",
            plane.fourcc_name(),
            plane.width,
            plane.height
        );
        match gpu.import(plane, format) {
            Some(texture) => {
                let bytes = gpu.read_back(&texture, plane, format);
                println!(
                    "imported, {} bytes read back, first row mean {:.1}",
                    bytes.len(),
                    mean(&bytes[..(plane.width as usize).min(bytes.len())])
                );
                read.push(Some(bytes));
            }
            None => {
                println!("REFUSED");
                read.push(None);
            }
        }
    }

    // Both planes together, reconstructed, against the software decode. The
    // luma alone would not catch a swapped U and V — the classic way a YUV
    // import goes subtly wrong — because luma is untouched by it.
    if planes.len() == 2 {
        if let (Some(luma), Some(chroma)) = (read[0].as_ref(), read[1].as_ref()) {
            compare_reconstructed(&file, at, &planes[0], &planes[1], luma, chroma);
        }
    }
}

/// What a DRM fourcc means to wgpu.
///
/// Only the three that come out of a VAAPI NV12 or packed-RGB surface; anything
/// else is a format this experiment was not written for and should say so
/// rather than guess.
fn wgpu_format(plane: &Plane) -> wgpu::TextureFormat {
    match plane.fourcc_name().as_str() {
        "R8  " => wgpu::TextureFormat::R8Unorm,
        // Two bytes per texel at half resolution in both axes: the interleaved
        // U and V of NV12, which is exactly `Rg8Unorm`.
        "GR88" => wgpu::TextureFormat::Rg8Unorm,
        "AR24" | "XR24" => wgpu::TextureFormat::Bgra8Unorm,
        "AB24" | "XB24" => wgpu::TextureFormat::Rgba8Unorm,
        _ => wgpu::TextureFormat::R8Unorm,
    }
}

fn mean(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return f64::NAN;
    }
    bytes.iter().map(|b| *b as u64).sum::<u64>() as f64 / bytes.len() as f64
}

/// Turn the two imported planes back into RGB and compare against a software
/// decode of the same instant.
///
/// "The import succeeded" is a weak claim on its own: a wrongly described tiling
/// produces a texture that is the right size, reads back without error and
/// contains a scrambled picture. And luma alone is not enough either — swapping
/// U and V leaves luma untouched and turns skin green, which is the failure that
/// gets shipped.
///
/// The maths here is the same BT.709 limited-range conversion a compositor
/// shader would do, which is the point: if this agrees with swscale, the shader
/// will too.
fn compare_reconstructed(
    file: &std::path::Path,
    at: i64,
    luma_plane: &Plane,
    chroma_plane: &Plane,
    luma: &[u8],
    chroma: &[u8],
) {
    let mut software = match VideoDecoder::open_with(file, Acceleration::Software) {
        Ok(decoder) => decoder,
        Err(error) => {
            println!("  (no software reference: {error})");
            return;
        }
    };
    let Ok(frame) = software.seek_and_decode(at) else {
        println!("  (no software reference)");
        return;
    };

    let width = luma_plane.width.min(frame.width) as usize;
    let height = luma_plane.height.min(frame.height) as usize;
    let chroma_width = chroma_plane.width as usize;

    let mut total = [0f64; 3];
    let mut worst = 0f64;
    let mut count = 0usize;
    let mut swapped_total = 0f64;

    for y in 0..height {
        for x in 0..width {
            let y_sample = luma[y * width + x] as f64;
            let c = ((y / 2) * chroma_width + x / 2) * 2;
            let (u, v) = (chroma[c] as f64, chroma[c + 1] as f64);

            let (r, g, b) = yuv709_to_rgb(y_sample, u, v);
            // The same pixel with U and V exchanged, scored alongside. If the
            // swapped number is the smaller one, the planes are the wrong way
            // round and the "matching" verdict would be backwards.
            let (sr, sg, sb) = yuv709_to_rgb(y_sample, v, u);

            let i = (y * frame.width as usize + x) * 4;
            let want = [
                frame.data[i] as f64,
                frame.data[i + 1] as f64,
                frame.data[i + 2] as f64,
            ];
            for (channel, got) in [r, g, b].iter().enumerate() {
                let delta = (want[channel] - got).abs();
                total[channel] += delta;
                worst = worst.max(delta);
            }
            swapped_total += (want[0] - sr).abs() + (want[1] - sg).abs() + (want[2] - sb).abs();
            count += 1;
        }
    }

    let n = count.max(1) as f64;
    let mean = (total[0] + total[1] + total[2]) / (n * 3.0);
    let swapped = swapped_total / (n * 3.0);
    println!(
        "  reconstructed RGB vs software decode: mean |Δ| {mean:.2} \
         (R {:.2} G {:.2} B {:.2}), worst {worst:.0}",
        total[0] / n,
        total[1] / n,
        total[2] / n
    );
    println!(
        "  chroma order control (U and V exchanged): mean |Δ| {swapped:.2} — {}",
        if swapped > mean * 2.0 {
            "the planes are the right way round"
        } else {
            "INCONCLUSIVE: this frame is too desaturated to tell"
        }
    );
    println!(
        "  verdict: {}",
        if mean < 6.0 {
            "the imported surface is the same picture"
        } else {
            "NOT the same picture — tiling, layout or plane order is wrong"
        }
    );
}

/// BT.709 limited-range YUV to full-range RGB, as a shader would do it.
fn yuv709_to_rgb(y: f64, u: f64, v: f64) -> (f64, f64, f64) {
    let y = (y - 16.0) * (255.0 / 219.0);
    let u = (u - 128.0) * (255.0 / 224.0);
    let v = (v - 128.0) * (255.0 / 224.0);
    (
        (y + 1.5748 * v).clamp(0.0, 255.0),
        (y - 0.1873 * u - 0.4681 * v).clamp(0.0, 255.0),
        (y + 1.8556 * u).clamp(0.0, 255.0),
    )
}

/// A wgpu device opened directly, because `render::RenderContext` asks for no
/// optional features and this experiment needs one.
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
    can_import: bool,
}

impl Gpu {
    fn open() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .ok()?;

        let wanted = wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
        let can_import = adapter.features().contains(wanted);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("dmabuf import probe"),
            required_features: if can_import {
                wanted
            } else {
                wgpu::Features::empty()
            },
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;

        let info = adapter.get_info();
        Some(Self {
            device,
            queue,
            info,
            can_import,
        })
    }

    /// Import one plane as a texture wgpu will bind.
    fn import(&self, plane: &Plane, format: wgpu::TextureFormat) -> Option<wgpu::Texture> {
        let fd = plane.dup_fd().ok()?;
        let size = wgpu::Extent3d {
            width: plane.width,
            height: plane.height,
            depth_or_array_layers: 1,
        };
        let hal_descriptor = wgpu_hal::TextureDescriptor {
            label: Some("imported decode surface"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUses::RESOURCE | wgpu::TextureUses::COPY_SRC,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };

        // SAFETY: `texture_from_dmabuf_fd` requires a descriptor that describes
        // the buffer truthfully and takes ownership of the descriptor. Both
        // hold: `plane` came straight out of the driver's own
        // `AVDRMFrameDescriptor`, and `fd` is a duplicate made for this call —
        // libavutil keeps its own and closes that one itself.
        let hal_texture = unsafe {
            let hal = self.device.as_hal::<wgpu_hal::api::Vulkan>()?;
            hal.texture_from_dmabuf_fd(
                fd,
                &hal_descriptor,
                plane.modifier,
                plane.pitch,
                plane.offset,
            )
            .ok()?
        };

        // SAFETY: the wgpu descriptor has to agree with the hal one, and does.
        // The initial state says the texture already holds data the GPU may
        // read, which is what stops wgpu treating it as uninitialised and
        // clearing the decoded picture before anything samples it.
        Some(unsafe {
            self.device
                .create_texture_from_hal::<wgpu_hal::api::Vulkan>(
                    hal_texture,
                    &wgpu::TextureDescriptor {
                        label: Some("imported decode surface"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    },
                    wgpu::TextureUses::RESOURCE,
                )
        })
    }

    /// Copy the texture into a mappable buffer and read it, tightly packed.
    fn read_back(
        &self,
        texture: &wgpu::Texture,
        plane: &Plane,
        format: wgpu::TextureFormat,
    ) -> Vec<u8> {
        let texel = format.block_copy_size(None).unwrap_or(1);
        let row = plane.width * texel;
        // wgpu requires 256-byte row alignment in a texture-to-buffer copy.
        let padded = row.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * plane.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(plane.height),
                },
            },
            wgpu::Extent3d {
                width: plane.width,
                height: plane.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        rx.recv().expect("callback").expect("map");

        let view = slice.get_mapped_range().expect("range");
        let mut out = Vec::with_capacity((row * plane.height) as usize);
        for y in 0..plane.height as usize {
            let start = y * padded as usize;
            out.extend_from_slice(&view[start..start + row as usize]);
        }
        drop(view);
        buffer.unmap();
        out
    }
}
