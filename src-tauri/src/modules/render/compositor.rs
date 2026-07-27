//! The compositor.
//!
//! `render_frame` is the whole module in one function: given a project and a
//! timeline position, produce the RGBA8 bytes a viewer would see. It is a pure
//! function of its arguments — call it twice with the same inputs and you get
//! the same pixels — which is what lets the preview and the exporter share it
//! without coordinating.
//!
//! Per frame it walks the segments live at `time` back-to-front, asks the
//! [`SourceProvider`] for each one's texture, and draws it as one quad. There
//! is no render graph and no per-segment target: a straight
//! painter's-algorithm loop into a single attachment, because that is what
//! source-over compositing is and anything more elaborate would only be there
//! for effects we have not built yet.
//!
//! The one exception is a transition, which by definition needs two clips at
//! once and blends them against the *frame* rather than against either clip's
//! quad. Those two clips are drawn into two full-canvas layers first — with the
//! same quad pipeline, so a hardware-decoded NV12 source needs no special case
//! anywhere — and the blend then lands in the painter's order where the segment
//! would have. See `modules/transitions/`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use parking_lot::Mutex;

use super::context::RenderContext;
use super::error::{RenderError, Result};
use super::layout::{self, QuadPlacement};
use super::nv12::{Nv12Converter, Nv12Frame, READ_FORMAT};
use super::source::{SourceFrame, SourceProvider, SourceRequest};
use super::texture_pool::{PooledTexture, TextureKey, TexturePool, DEFAULT_BUDGET_BYTES};
use crate::modules::project::document::{MaterialKind, Micros, Project, Segment};
use crate::modules::transitions::{self, TransitionParams, TransitionPipeline};

/// Alignment every `copy_texture_to_buffer` row must satisfy.
const COPY_ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// Usage every render target needs: drawn into, then read back.
///
/// `TEXTURE_BINDING` is not for the composite pass — a target is never one of
/// its own inputs — but for what happens afterwards: the RGBA→NV12 compute pass
/// samples it, and a preview that ever presents to a surface would too.
const TARGET_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::TEXTURE_BINDING);

#[repr(C)]
#[derive(Clone, Copy)]
struct QuadUniform {
    mvp: [f32; 16],
    crop: [f32; 4],
    opacity: f32,
    /// 1 when the source is two YUV planes rather than one RGBA texture.
    planar: u32,
    /// [`super::source::YuvMatrix`] and [`super::source::YuvRange`] as their
    /// discriminants.
    matrix: u32,
    range: u32,
    /// Clockwise quarter turns the shader must apply to the texture coordinate.
    turns: u32,
    _pad: [u32; 3],
}

// `#[repr(C)]`, every field a `f32` array, no padding: the definition of a
// plain-old-data type. Implemented by hand rather than derived so this module
// does not depend on `bytemuck`'s `derive` feature being switched on somewhere
// else in the dependency graph.
unsafe impl bytemuck::Zeroable for QuadUniform {}
unsafe impl bytemuck::Pod for QuadUniform {}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}

unsafe impl bytemuck::Zeroable for Vertex {}
unsafe impl bytemuck::Pod for Vertex {}

/// The unit quad, `±0.5` in local space, with UV origin at the top left.
///
/// Clip space is y-up and UV space is y-down, so the corner at `y = +0.5` is
/// `v = 0`. Getting this backwards renders everything upside down and is the
/// first thing to check if it ever does.
const QUAD_VERTICES: [Vertex; 4] = [
    Vertex {
        position: [-0.5, 0.5],
        uv: [0.0, 0.0],
    },
    Vertex {
        position: [0.5, 0.5],
        uv: [1.0, 0.0],
    },
    Vertex {
        position: [0.5, -0.5],
        uv: [1.0, 1.0],
    },
    Vertex {
        position: [-0.5, -0.5],
        uv: [0.0, 1.0],
    },
];

const QUAD_INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

/// Knobs the preview and the exporter set differently.
#[derive(Debug, Clone, Copy)]
pub struct CompositorConfig {
    /// Render target format. `Rgba8UnormSrgb` means blending happens in linear
    /// light and the bytes that come back are already sRGB-encoded, which is
    /// what a JPEG encoder or a PNG wants.
    pub format: wgpu::TextureFormat,
    /// How much idle texture memory the pool may hold.
    pub texture_budget_bytes: u64,
    /// What to do when the `SourceProvider` errors. The preview wants to keep
    /// drawing with one dead clip missing; an export would rather fail loudly
    /// than ship a hole.
    pub strict_sources: bool,
}

impl Default for CompositorConfig {
    fn default() -> Self {
        Self {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            texture_budget_bytes: DEFAULT_BUDGET_BYTES,
            strict_sources: false,
        }
    }
}

/// Where the time in a composited frame went, summed over every render.
///
/// This exists because `docs/research/zero-copy-encode.md` had to guess at the
/// readback's share of the frame budget and guessed wrong in both directions.
/// It is the number that sizes the whole zero-copy project, so it is measured
/// permanently rather than by a one-off patch that gets reverted.
///
/// Everything is nanoseconds of wall clock on the calling thread. Three
/// `Instant::now()` pairs per frame cost tens of nanoseconds against a frame
/// that costs tens of milliseconds, so this is always on — a counter nobody
/// switches on is a counter nobody reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub frames: u64,
    /// [`Compositor::collect_draws`]: asking the [`SourceProvider`] for each
    /// visible segment. On the export path this is where decoding happens.
    pub sources_ns: u64,
    /// Writing uniforms, building bind groups, recording and submitting the
    /// composite pass. Submission is not execution: the GPU work this queues is
    /// waited for in `readback_wait_ns`.
    pub composite_ns: u64,
    /// The whole of `read_back`, which is `readback_wait_ns` plus
    /// `readback_unpad_ns` plus the cost of recording the copy.
    pub readback_ns: u64,
    /// Blocked in `device.poll` waiting for the GPU to finish compositing *and*
    /// copying the target into a mappable buffer. This is the pipeline stall
    /// the zero-copy work is trying to delete.
    pub readback_wait_ns: u64,
    /// Copying the mapped rows into a `Vec`, stripping the 256-byte row
    /// padding. Pure CPU memory bandwidth.
    pub readback_unpad_ns: u64,
    /// The RGBA→NV12 compute pass and its readback, when that path is used.
    /// Disjoint from `readback_ns`: a frame does one or the other.
    pub nv12_ns: u64,
}

impl RenderStats {
    /// Mean nanoseconds per frame for one field, or 0 with no frames.
    pub fn per_frame(&self, total_ns: u64) -> f64 {
        if self.frames == 0 {
            0.0
        } else {
            total_ns as f64 / self.frames as f64
        }
    }
}

/// The atomic form of [`RenderStats`], so `render_frame` can stay `&self`.
#[derive(Default)]
struct StatCounters {
    frames: AtomicU64,
    sources_ns: AtomicU64,
    composite_ns: AtomicU64,
    readback_ns: AtomicU64,
    readback_wait_ns: AtomicU64,
    readback_unpad_ns: AtomicU64,
    nv12_ns: AtomicU64,
}

impl StatCounters {
    fn snapshot(&self) -> RenderStats {
        RenderStats {
            frames: self.frames.load(Ordering::Relaxed),
            sources_ns: self.sources_ns.load(Ordering::Relaxed),
            composite_ns: self.composite_ns.load(Ordering::Relaxed),
            readback_ns: self.readback_ns.load(Ordering::Relaxed),
            readback_wait_ns: self.readback_wait_ns.load(Ordering::Relaxed),
            readback_unpad_ns: self.readback_unpad_ns.load(Ordering::Relaxed),
            nv12_ns: self.nv12_ns.load(Ordering::Relaxed),
        }
    }

    fn reset(&self) {
        for counter in [
            &self.frames,
            &self.sources_ns,
            &self.composite_ns,
            &self.readback_ns,
            &self.readback_wait_ns,
            &self.readback_unpad_ns,
            &self.nv12_ns,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }
}

fn add(counter: &AtomicU64, since: Instant) {
    counter.fetch_add(since.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// One frame's worth of pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, `width * height * 4` bytes, no row padding.
    pub data: Vec<u8>,
}

impl Frame {
    /// The RGBA texel at `(x, y)`, y down from the top left.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }
}

/// Project + time -> frame.
///
/// Cheap to keep around and safe to share: `render_frame` takes `&self` and all
/// the mutable machinery (texture pool, scratch buffers) is behind locks, so
/// the exporter's worker threads and the preview can hold the same one.
pub struct Compositor {
    ctx: Arc<RenderContext>,
    config: CompositorConfig,
    pool: TexturePool,
    pipeline: wgpu::RenderPipeline,
    /// [`Self::pipeline`] with blending disabled, for transition layers.
    layer_pipeline: wgpu::RenderPipeline,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// What binding 2 gets when the source is RGBA and there is no chroma
    /// plane. One neutral texel, never sampled — the shader's `planar` branch
    /// does not read it — but the binding still has to exist.
    chroma_placeholder: wgpu::TextureView,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// Per-draw uniforms, addressed with dynamic offsets. Grown, never shrunk —
    /// a project's segment count is stable over a render.
    uniforms: Mutex<Scratch>,
    /// Staging buffer for readback, likewise reused.
    readback: Mutex<Scratch>,
    /// Stride between uniform blocks, satisfying the device's dynamic-offset
    /// alignment.
    uniform_stride: u32,
    stats: StatCounters,
    /// The RGBA→NV12 compute pass, built on first use.
    ///
    /// Lazy because the preview never wants it and building it costs a shader
    /// compile: a machine that only ever previews should not pay for the
    /// export path's pipeline.
    nv12: OnceLock<Option<Nv12Converter>>,
    /// The transition pipelines, built on first use.
    ///
    /// Lazy for the same reason `nv12` is: it is five shader compiles, and a
    /// project with no transitions in it should not pay for them. Built once
    /// the first frame containing a transition is rendered, which is a stutter
    /// on that frame and never again — `TransitionPipeline::new` builds every
    /// kind at once precisely so scrubbing into a second kind does not compile
    /// anything.
    transitions: OnceLock<TransitionPipeline>,
}

/// A reusable GPU buffer that only ever grows.
#[derive(Default)]
struct Scratch {
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
}

impl Scratch {
    fn ensure(
        &mut self,
        device: &wgpu::Device,
        size: u64,
        usage: wgpu::BufferUsages,
        label: &str,
    ) -> &wgpu::Buffer {
        if self.capacity < size || self.buffer.is_none() {
            let size = size.next_power_of_two().max(256);
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            }));
            self.capacity = size;
        }
        self.buffer.as_ref().expect("just ensured")
    }
}

impl Compositor {
    pub fn new(ctx: Arc<RenderContext>) -> Self {
        Self::with_config(ctx, CompositorConfig::default())
    }

    pub fn with_config(ctx: Arc<RenderContext>, config: CompositorConfig) -> Self {
        let device = ctx.device();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chukcut quad shader"),
            // `yuv.wgsl` has no entry point and no bindings; it is prepended
            // rather than imported because WGSL has no `#include`, and it is a
            // separate file because `nv12.wgsl` needs the other half of it.
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("shaders/yuv.wgsl"),
                    include_str!("shaders/quad.wgsl")
                )
                .into(),
            ),
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut quad uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<QuadUniform>() as u64
                    ),
                },
                count: None,
            }],
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut quad source"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // The chroma plane of a hardware-decoded frame. Always bound,
                // because WebGPU has no optional binding and a placeholder
                // texel is cheaper than a second pipeline.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chukcut quad pipeline layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)],
            immediate_size: 0,
        });

        let quad_pipeline = |label: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 8,
                                shader_location: 1,
                            },
                        ],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // A horizontal or vertical flip reverses the winding order,
                    // so culling would make flipped clips vanish. Two triangles
                    // are not worth the trouble.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        // Straight-alpha source-over. Matches what the shader emits.
        let pipeline = quad_pipeline(
            "chukcut quad pipeline",
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );

        // The same quad with the blend switched off, for a transition layer.
        //
        // A layer holds exactly one clip over a transparent clear, so there is
        // nothing to blend with — and blending would be actively wrong here.
        // `ALPHA_BLENDING` over a transparent destination leaves *premultiplied*
        // colour behind, while `transition.wgsl` samples its layers as straight
        // alpha and premultiplies them itself. A half-transparent clip would
        // then be multiplied by its own opacity twice, which reads as a dark
        // halo creeping in from the edges of anything that does not cover the
        // canvas. Writing the fragment through untouched is what makes the two
        // agree.
        let layer_pipeline = quad_pipeline("chukcut quad layer pipeline", None);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("chukcut quad sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let placeholder = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("chukcut chroma placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // 128, 128 is achromatic. It is never read, but a texture left
        // uninitialised is a texture wgpu clears on first use anyway, and
        // writing the neutral value means a future bug that *does* sample it
        // produces a grey frame rather than a green one.
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &placeholder,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[128u8, 128u8],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(2),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let chroma_placeholder =
            placeholder.create_view(&wgpu::TextureViewDescriptor::default());

        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chukcut quad vertices"),
            size: std::mem::size_of_val(&QUAD_VERTICES) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue()
            .write_buffer(&vertices, 0, bytemuck::cast_slice(&QUAD_VERTICES));

        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chukcut quad indices"),
            size: std::mem::size_of_val(&QUAD_INDICES) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue()
            .write_buffer(&indices, 0, bytemuck::cast_slice(&QUAD_INDICES));

        let align = ctx.limits().min_uniform_buffer_offset_alignment.max(1) as u64;
        let uniform_stride =
            (std::mem::size_of::<QuadUniform>() as u64).div_ceil(align) * align;

        Self {
            ctx,
            pool: TexturePool::new(config.texture_budget_bytes),
            config,
            pipeline,
            layer_pipeline,
            uniform_layout,
            texture_layout,
            sampler,
            chroma_placeholder,
            vertices,
            indices,
            uniforms: Mutex::new(Scratch::default()),
            readback: Mutex::new(Scratch::default()),
            uniform_stride: uniform_stride as u32,
            stats: StatCounters::default(),
            nv12: OnceLock::new(),
            transitions: OnceLock::new(),
        }
    }

    /// What every frame rendered so far cost, broken down by phase.
    pub fn stats(&self) -> RenderStats {
        self.stats.snapshot()
    }

    /// Zero the counters, so a benchmark can exclude its warm-up.
    pub fn reset_stats(&self) {
        self.stats.reset();
    }

    pub fn context(&self) -> &Arc<RenderContext> {
        &self.ctx
    }

    pub fn config(&self) -> &CompositorConfig {
        &self.config
    }

    /// The texture pool backing render targets. Exposed so a caller that holds
    /// a [`PooledTexture`] from [`Self::render_to_texture`] can give it back.
    pub fn pool(&self) -> &TexturePool {
        &self.pool
    }

    /// Composite `project` at `time` and read the result back as RGBA8.
    ///
    /// `size` is the output resolution, which is *not* required to equal the
    /// canvas size — the preview renders the same composition small and the
    /// exporter renders it large. Aspect handling comes from the canvas, so a
    /// proxy-resolution render is the full-resolution one scaled, not a
    /// differently framed one.
    pub fn render_frame(
        &self,
        project: &Project,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
    ) -> Result<Vec<u8>> {
        Ok(self.render(project, time, size, sources)?.data)
    }

    /// The key every render target is acquired with.
    ///
    /// Always asks for the non-sRGB view format, whether or not this render
    /// will use it. Making it conditional would split the pool into two buckets
    /// of otherwise interchangeable textures, and the flag costs nothing on a
    /// texture nobody reinterprets.
    fn target_key(&self, size: (u32, u32)) -> TextureKey {
        TextureKey::new(size.0, size.1, self.config.format, TARGET_USAGE)
            .viewable_as(READ_FORMAT)
    }

    /// Composite at `time` and hand back NV12 rather than RGBA.
    ///
    /// This is the export path's entry point. It differs from
    /// [`Self::render_frame`] in what crosses the bus: 1.5 bytes per pixel
    /// instead of 4, with the colour conversion done by a compute pass instead
    /// of by swscale on the CPU. The result is what a VAAPI or QSV frame pool
    /// wants uploaded into it.
    ///
    /// Errors the same way `render_frame` does; a caller that wants to keep
    /// exporting on a device where the compute pass will not build should fall
    /// back to `render_frame` plus swscale.
    pub fn render_nv12(
        &self,
        project: &Project,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
    ) -> Result<Nv12Frame> {
        let converter = self.nv12_converter().ok_or_else(|| {
            RenderError::Readback("this device has no RGBA to NV12 compute pass".into())
        })?;
        let target = self.render_to_texture(project, time, size, sources)?;

        let started = Instant::now();
        let before_wait = converter.wait_ns.load(Ordering::Relaxed);
        let frame = converter.convert(&self.ctx, &target);
        let waited = converter.wait_ns.load(Ordering::Relaxed) - before_wait;
        add(&self.stats.nv12_ns, started);
        self.stats
            .readback_wait_ns
            .fetch_add(waited, Ordering::Relaxed);

        self.pool.release(target);
        frame
    }

    /// Composite at `time` and write NV12 straight into `destination`.
    ///
    /// The zero-copy entry point. `destination` is expected to be the wgpu
    /// handle of a buffer whose memory was exported as a DMA-BUF (see
    /// [`super::dmabuf`]), so what this writes is what a hardware encoder reads
    /// and nothing comes back to system memory at all.
    ///
    /// Returns once the GPU has finished, which is the synchronisation the
    /// importer on the other side depends on.
    pub fn render_nv12_into(
        &self,
        project: &Project,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
        destination: &wgpu::Buffer,
    ) -> Result<()> {
        self.render_nv12_into_range(
            project,
            time,
            size,
            sources,
            destination,
            crate::modules::render::source::YuvRange::Limited,
        )
    }

    /// [`Self::render_nv12_into`], saying which range the encoder on the other
    /// side wants.
    ///
    /// The export's H.264 and H.265 encoders want limited; the preview's JPEG
    /// encoder wants full. A JPEG file carries no range tag, so limited-range
    /// samples in one come out as grey blacks — see `shaders/yuv.wgsl`.
    pub fn render_nv12_into_range(
        &self,
        project: &Project,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
        destination: &wgpu::Buffer,
        range: crate::modules::render::source::YuvRange,
    ) -> Result<()> {
        let converter = self.nv12_converter().ok_or_else(|| {
            RenderError::Readback("this device has no RGBA to NV12 compute pass".into())
        })?;
        let target = self.render_to_texture(project, time, size, sources)?;

        let started = Instant::now();
        let before_wait = converter.wait_ns.load(Ordering::Relaxed);
        let result = converter.convert_into_range(&self.ctx, &target, destination, range);
        let waited = converter.wait_ns.load(Ordering::Relaxed) - before_wait;
        add(&self.stats.nv12_ns, started);
        self.stats
            .readback_wait_ns
            .fetch_add(waited, Ordering::Relaxed);

        self.pool.release(target);
        result.map(|_| ())
    }

    /// The compute converter, built once. `None` on a device where it will not
    /// build, which is a reason to fall back rather than to fail.
    fn nv12_converter(&self) -> Option<&Nv12Converter> {
        self.nv12
            .get_or_init(|| Some(Nv12Converter::new(&self.ctx)))
            .as_ref()
    }

    /// [`Self::render_frame`], but keeping the dimensions attached.
    pub fn render(
        &self,
        project: &Project,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
    ) -> Result<Frame> {
        let target = self.render_to_texture(project, time, size, sources)?;
        let data = self.read_back(&target);
        self.pool.release(target);
        Ok(Frame {
            width: size.0,
            height: size.1,
            data: data?,
        })
    }

    /// Composite into a pooled texture and stop there.
    ///
    /// This is the seam the preview pipeline document calls out: today the
    /// frame is read back and JPEG-encoded, and if the preview ever moves to a
    /// native surface underneath the webview it is this texture that gets
    /// presented instead. Return it with `pool().release(...)` when done.
    pub fn render_to_texture(
        &self,
        project: &Project,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
    ) -> Result<PooledTexture> {
        self.ctx.check_size(size)?;

        let sourced = Instant::now();
        let draws = self.collect_draws(project, time, size, sources)?;
        add(&self.stats.sources_ns, sourced);

        let composited = Instant::now();
        let target = self.pool.acquire(self.ctx.device(), self.target_key(size));

        let device = self.ctx.device();
        let mut uniforms = self.uniforms.lock();
        let stride = self.uniform_stride as u64;
        let uniform_buffer = uniforms.ensure(
            device,
            stride * draws.slots.max(1) as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            "chukcut quad uniforms",
        );

        for quad in draws.quads() {
            let block = QuadUniform {
                mvp: quad.placement.mvp,
                crop: quad.placement.crop,
                opacity: quad.placement.opacity,
                planar: u32::from(quad.frame.is_planar()),
                matrix: quad.frame.matrix as u32,
                range: quad.frame.range as u32,
                turns: quad.frame.turns % 4,
                _pad: [0; 3],
            };
            self.ctx.queue().write_buffer(
                uniform_buffer,
                quad.slot as u64 * stride,
                bytemuck::bytes_of(&block),
            );
        }

        let uniform_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut quad uniform bind group"),
            layout: &self.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: uniform_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<QuadUniform>() as u64),
                }),
            }],
        });

        // Indexed by uniform slot, so a transition layer and an ordinary quad
        // are looked up the same way.
        let mut source_groups: Vec<Option<wgpu::BindGroup>> =
            (0..draws.slots).map(|_| None).collect();
        for quad in draws.quads() {
            source_groups[quad.slot as usize] = Some(self.source_group(device, &quad.frame));
        }

        let bg = project.canvas.background;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("chukcut frame"),
        });

        // Every transition's two layers, drawn into full-canvas targets of
        // their own before the frame is composited. They have to be complete
        // passes rather than draws inside the composite pass: a render pass
        // cannot sample the attachment it is writing.
        let mut layer_targets: Vec<PooledTexture> = Vec::new();
        let mut layer_groups: Vec<Option<wgpu::BindGroup>> = Vec::with_capacity(draws.items.len());
        for item in &draws.items {
            let Draw::Transition { from, to, .. } = item else {
                layer_groups.push(None);
                continue;
            };
            let pipeline = self.transition_pipeline();
            let from_target = self.pool.acquire(device, self.target_key(size));
            let to_target = self.pool.acquire(device, self.target_key(size));
            self.draw_layer(
                &mut encoder,
                &uniform_group,
                &source_groups,
                from.as_ref(),
                from_target.view(),
            );
            self.draw_layer(
                &mut encoder,
                &uniform_group,
                &source_groups,
                to.as_ref(),
                to_target.view(),
            );
            layer_groups.push(Some(pipeline.bind_layers(
                &self.ctx,
                from_target.view(),
                to_target.view(),
            )));
            layer_targets.push(from_target);
            layer_targets.push(to_target);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("chukcut composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0] as f64,
                            g: bg[1] as f64,
                            b: bg[2] as f64,
                            a: bg[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);

            let mut transition_slot = 0u32;
            for (i, item) in draws.items.iter().enumerate() {
                match item {
                    Draw::Quad(quad) => {
                        let Some(group) = source_groups[quad.slot as usize].as_ref() else {
                            continue;
                        };
                        pass.set_pipeline(&self.pipeline);
                        pass.set_bind_group(0, &uniform_group, &[quad.slot * self.uniform_stride]);
                        pass.set_bind_group(1, group, &[]);
                        pass.draw_indexed(0..QUAD_INDICES.len() as u32, 0, 0..1);
                    }
                    Draw::Transition { params, .. } => {
                        let Some(layers) = layer_groups[i].as_ref() else {
                            continue;
                        };
                        self.transition_pipeline().draw(
                            &self.ctx,
                            &mut pass,
                            transition_slot,
                            params,
                            layers,
                        );
                        transition_slot += 1;
                    }
                }
            }
        }
        self.ctx.queue().submit(Some(encoder.finish()));
        drop(uniforms);
        for layer in layer_targets {
            self.pool.release(layer);
        }
        add(&self.stats.composite_ns, composited);
        self.stats.frames.fetch_add(1, Ordering::Relaxed);

        Ok(target)
    }

    /// The transition pipelines, built on first use. See [`Self::transitions`].
    fn transition_pipeline(&self) -> &TransitionPipeline {
        self.transitions
            .get_or_init(|| TransitionPipeline::new(&self.ctx, self.config.format))
    }

    fn source_group(&self, device: &wgpu::Device, frame: &SourceFrame) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut quad source bind group"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&frame.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        frame
                            .chroma_view
                            .as_deref()
                            .unwrap_or(&self.chroma_placeholder),
                    ),
                },
            ],
        })
    }

    /// Draw one side of a transition into its own full-canvas target.
    ///
    /// Cleared to transparent and drawn with [`Self::layer_pipeline`], so what
    /// lands in the texture is the clip's own straight-alpha colour where it
    /// covers the canvas and nothing at all where it does not. That is what
    /// `transition.wgsl` expects to sample; the letterbox bars around a clip
    /// have to be *transparent* rather than background-coloured, or a
    /// transition on an upper track would punch a hole in the tracks below it.
    ///
    /// `quad` is `None` when that side has no picture — a source that failed, a
    /// clip faded fully out, a still-empty layer — and the transparent clear is
    /// then the whole layer, so the transition fades from or to nothing rather
    /// than the frame going missing.
    fn draw_layer(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniform_group: &wgpu::BindGroup,
        source_groups: &[Option<wgpu::BindGroup>],
        quad: Option<&QuadDraw>,
        view: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chukcut transition layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        let Some(quad) = quad else {
            return;
        };
        let Some(group) = source_groups[quad.slot as usize].as_ref() else {
            return;
        };
        pass.set_pipeline(&self.layer_pipeline);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.set_bind_group(0, uniform_group, &[quad.slot * self.uniform_stride]);
        pass.set_bind_group(1, group, &[]);
        pass.draw_indexed(0..QUAD_INDICES.len() as u32, 0, 0..1);
    }

    /// What the frame is made of: one entry per thing that gets drawn, in
    /// painter's order.
    fn collect_draws(
        &self,
        project: &Project,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
    ) -> Result<DrawList> {
        let canvas = (project.canvas.width, project.canvas.height);
        let mut draws = DrawList::default();

        for (track, segment) in layout::visible_segments(project, time) {
            // A transition claims the segment the compositor was about to draw
            // and replaces it with a blend of two. Exactly one of the two clips
            // contains any instant of the window — the cut is the boundary
            // between them — so no transition is ever drawn twice, and the
            // segment on the far side of the cut is not separately visible.
            if let Some(instant) = transitions::instant_for(track, &project.materials, segment, time)
            {
                let from = self.quad(
                    canvas,
                    size,
                    sources,
                    instant.from.segment,
                    instant.from.kind,
                    instant.from.source_time,
                    time,
                    &mut draws,
                )?;
                let to = self.quad(
                    canvas,
                    size,
                    sources,
                    instant.to.segment,
                    instant.to.kind,
                    instant.to.source_time,
                    time,
                    &mut draws,
                )?;
                if from.is_some() || to.is_some() {
                    draws.items.push(Draw::Transition {
                        params: TransitionParams::from(&instant),
                        from,
                        to,
                    });
                }
                continue;
            }

            let Some(kind) = project.materials.kind_of(&segment.material_id) else {
                // `Project::validate` reports this as an error; refusing to
                // render because of it would make a broken document
                // un-openable, which is worse.
                tracing::warn!(
                    segment = %segment.id,
                    material = %segment.material_id,
                    "segment references a material that is not in the pool"
                );
                continue;
            };

            let Some(source_time) = segment.source_time_at(time) else {
                continue;
            };

            let quad = self.quad(
                canvas, size, sources, segment, kind, source_time, time, &mut draws,
            )?;
            if let Some(quad) = quad {
                draws.items.push(Draw::Quad(quad));
            }
        }

        Ok(draws)
    }

    /// One segment's texture and where it goes, with a uniform slot reserved.
    ///
    /// `None` when there is nothing to draw: an audio material, a source that
    /// had no frame, a clip faded fully out. On the transition path that is a
    /// transparent layer rather than a missing frame — see [`Self::draw_layer`].
    #[allow(clippy::too_many_arguments)]
    fn quad(
        &self,
        canvas: (u32, u32),
        size: (u32, u32),
        sources: &dyn SourceProvider,
        segment: &Segment,
        kind: MaterialKind,
        source_time: Micros,
        time: Micros,
        draws: &mut DrawList,
    ) -> Result<Option<QuadDraw>> {
        if kind == MaterialKind::Audio {
            return Ok(None);
        }

        let request = SourceRequest {
            material_id: &segment.material_id,
            kind,
            source_time,
            segment_id: &segment.id,
            max_size: size,
        };

        let frame = match sources.frame(&self.ctx, &request) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(None),
            Err(e) => {
                if self.config.strict_sources {
                    return Err(RenderError::Source {
                        material_id: segment.material_id.clone(),
                        source_time,
                        source: e,
                    });
                }
                tracing::warn!(
                    segment = %segment.id,
                    material = %segment.material_id,
                    error = %e,
                    "skipping segment: source unavailable"
                );
                return Ok(None);
            }
        };

        let transform = layout::animated_transform(segment, time);
        let Some(placement) = layout::place_quad(canvas, frame.size(), &transform, segment.crop)
        else {
            return Ok(None);
        };

        let slot = draws.slots as u32;
        draws.slots += 1;
        Ok(Some(QuadDraw {
            frame,
            placement,
            slot,
        }))
    }

    /// Copy a render target back to the CPU, unpadding the rows.
    ///
    /// `copy_texture_to_buffer` demands 256-byte row alignment, so anything
    /// whose width is not a multiple of 64 pixels comes back with padding that
    /// has to be stripped. Skipping this is the classic "why is my frame
    /// sheared diagonally" bug.
    fn read_back(&self, target: &PooledTexture) -> Result<Vec<u8>> {
        let started = Instant::now();
        let result = self.read_back_inner(target);
        add(&self.stats.readback_ns, started);
        result
    }

    fn read_back_inner(&self, target: &PooledTexture) -> Result<Vec<u8>> {
        let (width, height) = (target.width(), target.height());
        let bytes_per_pixel = target
            .format()
            .block_copy_size(None)
            .ok_or_else(|| RenderError::Readback(format!("{:?} is not copyable", target.format())))?;
        let unpadded = width * bytes_per_pixel;
        let padded = unpadded.div_ceil(COPY_ALIGN) * COPY_ALIGN;
        let total = padded as u64 * height as u64;

        let device = self.ctx.device();
        let mut readback = self.readback.lock();
        let buffer = readback.ensure(
            device,
            total,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            "chukcut frame readback",
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("chukcut readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.ctx.queue().submit(Some(encoder.finish()));

        let slice = buffer.slice(0..total);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let waited = Instant::now();
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|_| RenderError::Readback("map callback never fired".into()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        add(&self.stats.readback_wait_ns, waited);

        // Unmap unconditionally: a buffer left mapped cannot be mapped again,
        // and this one is reused for every subsequent frame.
        let unpadded_at = Instant::now();
        let copied = {
            let result = slice.get_mapped_range();
            match result {
                Ok(view) => {
                    let mut out = Vec::with_capacity(unpadded as usize * height as usize);
                    for row in 0..height as usize {
                        let start = row * padded as usize;
                        out.extend_from_slice(&view[start..start + unpadded as usize]);
                    }
                    Ok(out)
                }
                Err(e) => Err(RenderError::Readback(e.to_string())),
            }
        };
        buffer.unmap();
        add(&self.stats.readback_unpad_ns, unpadded_at);

        copied
    }
}

impl std::fmt::Debug for Compositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compositor")
            .field("config", &self.config)
            .field("pool", &self.pool)
            .finish()
    }
}

/// One textured quad: a segment's frame, where it goes, and which uniform
/// block holds that.
///
/// The slot is explicit rather than positional because a transition contributes
/// two quads that are *not* drawn in the composite pass — they are drawn into
/// layers beforehand — so "the nth draw" and "the nth uniform block" stopped
/// being the same number.
struct QuadDraw {
    frame: SourceFrame,
    placement: QuadPlacement,
    slot: u32,
}

/// One thing the composite pass draws.
enum Draw {
    Quad(QuadDraw),
    /// Two clips blended against the frame. Either side may be `None`, which is
    /// a transparent layer; both being `None` is not built at all.
    Transition {
        params: TransitionParams,
        from: Option<QuadDraw>,
        to: Option<QuadDraw>,
    },
}

/// Everything a frame draws, plus how many uniform blocks it needs.
#[derive(Default)]
struct DrawList {
    items: Vec<Draw>,
    /// Number of uniform slots handed out. Not `items.len()`: a transition is
    /// one item and up to two quads.
    slots: usize,
}

impl DrawList {
    /// Every quad, in the order the slots were handed out.
    fn quads(&self) -> impl Iterator<Item = &QuadDraw> {
        self.items
            .iter()
            .flat_map(|item| match item {
                Draw::Quad(quad) => [Some(quad), None],
                Draw::Transition { from, to, .. } => [from.as_ref(), to.as_ref()],
            })
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, Easing, Segment, TimeRange, Track, TrackKind, Transform, TransitionDirection,
        TransitionKind, TransitionMaterial,
    };
    use crate::modules::render::source::{SolidColorProvider, SolidSource, YuvMatrix, YuvRange};

    /// wgpu is not available on every machine this suite runs on. Skip rather
    /// than fail — a red suite that means "this box has no GPU" trains people
    /// to ignore it.
    fn compositor() -> Option<Compositor> {
        let ctx = crate::modules::render::test_context()?;
        Some(Compositor::with_config(
            ctx,
            CompositorConfig {
                // Unorm rather than sRGB so the tests can assert on exact byte
                // values without doing gamma maths.
                format: wgpu::TextureFormat::Rgba8Unorm,
                ..Default::default()
            },
        ))
    }

    fn project(background: [f32; 4]) -> Project {
        Project::new(
            "test",
            CanvasConfig {
                width: 640,
                height: 480,
                background,
            },
            30.0,
        )
    }

    fn segment(material: &str, start: Micros, duration: Micros) -> Segment {
        Segment {
            id: format!("seg-{material}-{start}"),
            material_id: material.into(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(0, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// Register `id` as a video material so `kind_of` finds it.
    fn add_video(project: &mut Project, id: &str) {
        project
            .materials
            .videos
            .push(crate::modules::project::document::VideoMaterial {
                id: id.into(),
                path: format!("/nonexistent/{id}.mp4"),
                width: 640,
                height: 480,
                duration: 10_000_000,
                fps: 30.0,
                has_audio: false,
                rotation: 0,
            });
    }

    fn frame(
        compositor: &Compositor,
        project: &Project,
        time: Micros,
        p: &dyn SourceProvider,
    ) -> Frame {
        compositor
            .render(project, time, (640, 480), p)
            .expect("render")
    }

    /// A compositor configured the way the application configures it.
    ///
    /// The `Rgba8Unorm` one above exists so tests can assert exact bytes
    /// without gamma maths, which is the right trade for geometry and blending.
    /// It is the wrong trade for colour conversion: the YUV path linearises in
    /// the shader precisely because the render target re-encodes, and a linear
    /// target would leave every expected value looking arbitrary.
    fn srgb_compositor() -> Option<Compositor> {
        Some(Compositor::new(crate::modules::render::test_context()?))
    }

    #[test]
    fn empty_project_is_the_background_colour() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let project = project([0.0, 0.25, 0.5, 1.0]);
        let provider = SolidColorProvider::new();
        let f = frame(&c, &project, 0, &provider);

        assert_eq!(f.data.len(), 640 * 480 * 4);
        assert_eq!(f.pixel(320, 240), [0, 64, 128, 255]);
        assert_eq!(provider.call_count(), 0);
    }

    #[test]
    fn a_matching_clip_covers_the_canvas() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = project([0.0, 0.0, 0.0, 1.0]);
        add_video(&mut project, "red");
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(segment("red", 0, 4_000_000));
        project.tracks.push(track);

        let provider =
            SolidColorProvider::new().with("red", SolidSource::new([1.0, 0.0, 0.0, 1.0], 640, 480));
        let f = frame(&c, &project, 1_000_000, &provider);

        assert_eq!(f.pixel(320, 240), [255, 0, 0, 255]);
        assert_eq!(f.pixel(2, 2), [255, 0, 0, 255]);
    }

    #[test]
    fn a_wide_clip_on_a_narrow_canvas_is_letterboxed() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // 640x480 canvas (4:3), 16:9 source: fitted height is 360, so 60 rows
        // of background survive top and bottom.
        let mut project = project([0.0, 0.0, 1.0, 1.0]);
        add_video(&mut project, "green");
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(segment("green", 0, 4_000_000));
        project.tracks.push(track);

        let provider = SolidColorProvider::new()
            .with("green", SolidSource::new([0.0, 1.0, 0.0, 1.0], 1920, 1080));
        let f = frame(&c, &project, 0, &provider);

        assert_eq!(f.pixel(320, 240), [0, 255, 0, 255], "centre is the clip");
        assert_eq!(f.pixel(320, 5), [0, 0, 255, 255], "top band is background");
        assert_eq!(
            f.pixel(320, 474),
            [0, 0, 255, 255],
            "bottom band is background"
        );
    }

    #[test]
    fn render_index_decides_who_is_on_top() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = project([0.0, 0.0, 0.0, 1.0]);
        add_video(&mut project, "under");
        add_video(&mut project, "over");

        let mut lower = Track::new(TrackKind::Video, "V1");
        let mut seg = segment("under", 0, 4_000_000);
        seg.render_index = 0;
        lower.segments.push(seg);

        let mut upper = Track::new(TrackKind::Video, "V2");
        let mut seg = segment("over", 0, 4_000_000);
        seg.render_index = 1;
        upper.segments.push(seg);

        project.tracks.push(lower);
        project.tracks.push(upper);

        let provider = SolidColorProvider::new()
            .with("under", SolidSource::new([1.0, 0.0, 0.0, 1.0], 640, 480))
            .with("over", SolidSource::new([0.0, 1.0, 0.0, 1.0], 640, 480));
        let f = frame(&c, &project, 0, &provider);
        assert_eq!(f.pixel(320, 240), [0, 255, 0, 255]);
    }

    #[test]
    fn hidden_tracks_are_never_even_asked_for() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = project([0.0, 0.0, 0.0, 1.0]);
        add_video(&mut project, "red");
        let mut track = Track::new(TrackKind::Video, "V1");
        track.hidden = true;
        track.segments.push(segment("red", 0, 4_000_000));
        project.tracks.push(track);

        let provider =
            SolidColorProvider::new().with("red", SolidSource::new([1.0, 0.0, 0.0, 1.0], 640, 480));
        let f = frame(&c, &project, 0, &provider);

        assert_eq!(f.pixel(320, 240), [0, 0, 0, 255]);
        assert_eq!(provider.call_count(), 0);
    }

    #[test]
    fn opacity_blends_with_what_is_underneath() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = project([0.0, 0.0, 0.0, 1.0]);
        add_video(&mut project, "white");
        let mut track = Track::new(TrackKind::Video, "V1");
        let mut seg = segment("white", 0, 4_000_000);
        seg.transform.opacity = 0.5;
        track.segments.push(seg);
        project.tracks.push(track);

        let provider = SolidColorProvider::new()
            .with("white", SolidSource::new([1.0, 1.0, 1.0, 1.0], 640, 480));
        let f = frame(&c, &project, 0, &provider);

        let [r, _, _, a] = f.pixel(320, 240);
        assert!((r as i32 - 128).abs() <= 2, "half of white over black, got {r}");
        assert_eq!(a, 255);
    }

    #[test]
    fn segments_outside_the_playhead_are_not_drawn() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = project([0.0, 0.0, 0.0, 1.0]);
        add_video(&mut project, "red");
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(segment("red", 2_000_000, 1_000_000));
        project.tracks.push(track);

        let provider =
            SolidColorProvider::new().with("red", SolidSource::new([1.0, 0.0, 0.0, 1.0], 640, 480));

        assert_eq!(frame(&c, &project, 0, &provider).pixel(320, 240), [0, 0, 0, 255]);
        assert_eq!(
            frame(&c, &project, 2_500_000, &provider).pixel(320, 240),
            [255, 0, 0, 255]
        );
        // The segment is half-open: its end instant belongs to the next clip.
        assert_eq!(
            frame(&c, &project, 3_000_000, &provider).pixel(320, 240),
            [0, 0, 0, 255]
        );
    }

    #[test]
    fn odd_frame_widths_read_back_without_row_padding() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // 101 * 4 = 404 bytes per row, padded to 512 on the GPU.
        let project = project([1.0, 0.0, 0.0, 1.0]);
        let provider = SolidColorProvider::new();
        let f = c
            .render(&project, 0, (101, 37), &provider)
            .expect("render");
        assert_eq!(f.data.len(), 101 * 37 * 4);
        for y in 0..37 {
            for x in 0..101 {
                assert_eq!(f.pixel(x, y), [255, 0, 0, 255], "at {x},{y}");
            }
        }
    }

    #[test]
    fn a_source_that_fails_does_not_kill_the_frame() {
        struct Broken;
        impl SourceProvider for Broken {
            fn frame(
                &self,
                _ctx: &RenderContext,
                _request: &SourceRequest<'_>,
            ) -> anyhow::Result<Option<SourceFrame>> {
                anyhow::bail!("decoder exploded")
            }
        }

        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = project([0.0, 0.0, 0.0, 1.0]);
        add_video(&mut project, "red");
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(segment("red", 0, 4_000_000));
        project.tracks.push(track);

        let lenient = c.render(&project, 0, (64, 64), &Broken);
        assert!(lenient.is_ok(), "preview keeps drawing around a dead clip");

        let strict = Compositor::with_config(
            c.context().clone(),
            CompositorConfig {
                format: wgpu::TextureFormat::Rgba8Unorm,
                strict_sources: true,
                ..Default::default()
            },
        );
        assert!(strict.render(&project, 0, (64, 64), &Broken).is_err());
    }

    #[test]
    fn rendering_twice_reuses_the_target_texture() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let project = project([0.0, 0.0, 0.0, 1.0]);
        let provider = SolidColorProvider::new();
        for _ in 0..3 {
            frame(&c, &project, 0, &provider);
        }
        let stats = c.pool().stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 2);
    }

    /// A provider that answers with two NV12 planes, uploaded from the CPU.
    ///
    /// The real source of those planes is a hardware decoder's surface,
    /// imported without a copy — but the *shader* cannot tell the difference,
    /// and this way the colour conversion and the rotation are testable on any
    /// machine with a GPU, including one with no VAAPI at all. What it does not
    /// cover is the import itself; `examples/hwdecode_pipeline.rs --verify` does that.
    #[derive(Clone, Copy)]
    struct PlanarProvider {
        y: u8,
        cb: u8,
        cr: u8,
        width: u32,
        height: u32,
        matrix: YuvMatrix,
        range: YuvRange,
        turns: u32,
    }

    impl SourceProvider for PlanarProvider {
        fn frame(
            &self,
            ctx: &RenderContext,
            _request: &SourceRequest<'_>,
        ) -> anyhow::Result<Option<SourceFrame>> {
            let plane = |w: u32, h: u32, format: wgpu::TextureFormat, texel: &[u8]| {
                let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let mut data = Vec::new();
                for _ in 0..(w * h) {
                    data.extend_from_slice(texel);
                }
                ctx.queue().write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * texel.len() as u32),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
                Arc::new(texture)
            };

            Ok(Some(SourceFrame::from_planes(
                plane(
                    self.width,
                    self.height,
                    wgpu::TextureFormat::R8Unorm,
                    &[self.y],
                ),
                plane(
                    self.width / 2,
                    self.height / 2,
                    wgpu::TextureFormat::Rg8Unorm,
                    &[self.cb, self.cr],
                ),
                self.matrix,
                self.range,
                self.turns,
                None,
            )))
        }
    }

    fn planar_project() -> Project {
        let mut project = project([0.0, 0.0, 0.0, 1.0]);
        add_video(&mut project, "nv12");
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(segment("nv12", 0, 4_000_000));
        project.tracks.push(track);
        project
    }

    /// Limited-range mid grey is code 128 luma with achromatic chroma, and it
    /// has to come out lighter than 128 in RGB, because 128 sits 112/219 of the
    /// way up the limited excursion rather than half way.
    ///
    /// Asserting the *number* rather than "it is grey" is the point: a shader
    /// that skipped the range rescale would still produce a perfectly
    /// convincing grey, just the wrong one, and that is the failure that ships.
    #[test]
    fn a_planar_source_is_converted_with_the_limited_range_rescale() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let provider = PlanarProvider {
            y: 128,
            cb: 128,
            cr: 128,
            width: 640,
            height: 480,
            matrix: YuvMatrix::Bt709,
            range: YuvRange::Limited,
            turns: 0,
        };
        let f = frame(&c, &planar_project(), 0, &provider);
        let [r, g, b, a] = f.pixel(320, 240);
        // (128 - 16) * 255/219 = 130.4, and the test compositor renders to a
        // linear target so no transfer function is applied on the way out.
        for (channel, value) in [("r", r), ("g", g), ("b", b)] {
            assert!(
                (value as i32 - 130).abs() <= 2,
                "{channel} is {value}, expected the limited-range rescale of 128"
            );
        }
        assert_eq!(a, 255);
    }

    /// The two matrices have to actually differ, or "take it from the file" is
    /// decoration. A saturated chroma sample is where they disagree most.
    #[test]
    fn the_colour_matrix_changes_the_result() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let project = planar_project();
        let base = PlanarProvider {
            y: 128,
            cb: 90,
            cr: 200,
            width: 640,
            height: 480,
            matrix: YuvMatrix::Bt709,
            range: YuvRange::Limited,
            turns: 0,
        };
        let bt709 = frame(&c, &project, 0, &base).pixel(320, 240);
        let bt601 = frame(
            &c,
            &project,
            0,
            &PlanarProvider {
                matrix: YuvMatrix::Bt601,
                ..base
            },
        )
        .pixel(320, 240);

        let spread = (0..3)
            .map(|i| (bt709[i] as i32 - bt601[i] as i32).abs())
            .max()
            .unwrap_or(0);
        assert!(
            spread >= 8,
            "BT.709 {bt709:?} and BT.601 {bt601:?} differ by only {spread} code values"
        );
    }

    /// A sideways source fills the canvas the way its *display* aspect says it
    /// should, not the way its stored one does.
    #[test]
    fn a_quarter_turned_planar_source_is_fitted_by_its_display_aspect() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // Stored 640x480 (4:3), displayed 480x640 (3:4) on a 4:3 canvas: the
        // fit is height-limited, so 640 * 3/4 = 360 wide, leaving 140 columns
        // of background on each side.
        let provider = PlanarProvider {
            y: 235,
            cb: 128,
            cr: 128,
            width: 640,
            height: 480,
            matrix: YuvMatrix::Bt709,
            range: YuvRange::Limited,
            turns: 1,
        };
        let mut project = planar_project();
        project.canvas.background = [0.0, 0.0, 1.0, 1.0];
        let f = frame(&c, &project, 0, &provider);

        assert!(f.pixel(320, 240)[0] > 240, "centre is the clip");
        assert_eq!(f.pixel(5, 240), [0, 0, 255, 255], "left band is background");
        assert_eq!(
            f.pixel(634, 240),
            [0, 0, 255, 255],
            "right band is background"
        );
    }

    // -----------------------------------------------------------------------
    // Transitions
    // -----------------------------------------------------------------------

    /// One test material: either an ordinary RGBA texture, as a software
    /// decoder produces, or two NV12 planes, as a hardware one does.
    ///
    /// Both cases exist here rather than in two providers because the thing
    /// most worth pinning about a transition is that it does not care which it
    /// gets. A transition that blends only software-decoded clips is not a
    /// feature, it is a bug that waits for a file the machine can decode on the
    /// GPU — which, since `DEFAULT_ACCELERATION` is `Auto`, is most of them.
    #[derive(Clone, Copy)]
    enum TestSource {
        Rgba([u8; 4], u32, u32),
        /// Full-range NV12, so a luma of 255 is white and 0 is black with no
        /// rescale to reason about.
        Planar(u8, u8, u8, u32, u32),
    }

    #[derive(Default)]
    struct MixedProvider {
        materials: std::collections::HashMap<String, TestSource>,
        calls: AtomicU64,
    }

    impl MixedProvider {
        fn with(mut self, id: &str, source: TestSource) -> Self {
            self.materials.insert(id.into(), source);
            self
        }

        fn calls(&self) -> u64 {
            self.calls.load(Ordering::Relaxed)
        }

        fn plane(
            ctx: &RenderContext,
            w: u32,
            h: u32,
            format: wgpu::TextureFormat,
            texel: &[u8],
        ) -> Arc<wgpu::Texture> {
            let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("mixed provider plane"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let mut data = Vec::with_capacity((w * h) as usize * texel.len());
            for _ in 0..(w * h) {
                data.extend_from_slice(texel);
            }
            ctx.queue().write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * texel.len() as u32),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            Arc::new(texture)
        }
    }

    impl SourceProvider for MixedProvider {
        fn frame(
            &self,
            ctx: &RenderContext,
            request: &SourceRequest<'_>,
        ) -> anyhow::Result<Option<SourceFrame>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let Some(source) = self.materials.get(request.material_id).copied() else {
                return Ok(None);
            };
            Ok(Some(match source {
                TestSource::Rgba(color, w, h) => SourceFrame::from_texture(Self::plane(
                    ctx,
                    w,
                    h,
                    wgpu::TextureFormat::Rgba8Unorm,
                    &color,
                )),
                TestSource::Planar(y, cb, cr, w, h) => SourceFrame::from_planes(
                    Self::plane(ctx, w, h, wgpu::TextureFormat::R8Unorm, &[y]),
                    Self::plane(ctx, w / 2, h / 2, wgpu::TextureFormat::Rg8Unorm, &[cb, cr]),
                    YuvMatrix::Bt709,
                    YuvRange::Full,
                    0,
                    None,
                ),
            }))
        }
    }

    /// Two abutting four-second clips with a transition at the cut.
    ///
    /// The window is centred on 4 s and `Easing::Linear` is set explicitly, so
    /// progress at the cut is exactly 0.5 and every assertion below is about
    /// the blend rather than about the easing curve.
    fn cut_project(
        kind: TransitionKind,
        duration: Micros,
        size: (u32, u32),
    ) -> Project {
        let mut project = project([0.0, 0.0, 0.0, 1.0]);
        for id in ["left", "right"] {
            project
                .materials
                .videos
                .push(crate::modules::project::document::VideoMaterial {
                    id: id.into(),
                    path: format!("/nonexistent/{id}.mp4"),
                    width: size.0,
                    height: size.1,
                    duration: 10_000_000,
                    fps: 30.0,
                    has_audio: false,
                    rotation: 0,
                });
        }

        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(segment("left", 0, 4_000_000));
        // The incoming clip starts two seconds into its file, so the first half
        // of the window has a real head handle to borrow from.
        let mut right = segment("right", 4_000_000, 4_000_000);
        right.source_range = TimeRange::new(2_000_000, 4_000_000);
        track.segments.push(right);

        let mut material = TransitionMaterial::new(kind, duration);
        material.easing = Easing::Linear;
        track.segments[1].extras.push(material.id.clone());
        project.materials.transitions.push(material);
        project.tracks.push(track);
        project
    }

    /// The transition on the project built above, for a test that wants to
    /// change one of its parameters.
    fn transition_mut(project: &mut Project) -> &mut TransitionMaterial {
        &mut project.materials.transitions[0]
    }

    const RED: TestSource = TestSource::Rgba([255, 0, 0, 255], 640, 480);
    const BLUE: TestSource = TestSource::Rgba([0, 0, 255, 255], 640, 480);

    #[test]
    fn a_crossfade_runs_from_one_clip_to_the_other_across_its_window() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // A one-second dissolve at a cut on 4 s: the window is [3.5 s, 4.5 s).
        let project = cut_project(TransitionKind::Dissolve, 1_000_000, (640, 480));
        let provider = MixedProvider::default().with("left", RED).with("right", BLUE);

        // Before and after the window there is no transition at all, just the
        // clip that owns the instant.
        assert_eq!(frame(&c, &project, 3_000_000, &provider).pixel(320, 240), [255, 0, 0, 255]);
        assert_eq!(frame(&c, &project, 5_000_000, &provider).pixel(320, 240), [0, 0, 255, 255]);

        // At the very start of the window the outgoing clip is still whole.
        assert_eq!(frame(&c, &project, 3_500_000, &provider).pixel(320, 240), [255, 0, 0, 255]);

        // At the cut, half way: the two clips in equal measure. This is the
        // number the whole feature is about.
        let middle = frame(&c, &project, 4_000_000, &provider).pixel(320, 240);
        assert!(
            (middle[0] as i32 - 128).abs() <= 2 && (middle[2] as i32 - 128).abs() <= 2,
            "half way through a dissolve should be half of each clip, got {middle:?}"
        );
        assert_eq!(middle[1], 0, "nothing green went in, so nothing comes out");
        assert_eq!(middle[3], 255, "the frame is opaque throughout");

        // A quarter of the way through, the outgoing clip still dominates.
        let quarter = frame(&c, &project, 3_750_000, &provider).pixel(320, 240);
        assert!(
            (quarter[0] as i32 - 191).abs() <= 3 && (quarter[2] as i32 - 64).abs() <= 3,
            "a quarter through should be three parts outgoing, got {quarter:?}"
        );

        // And at the last microsecond of the window it is all but arrived.
        let end = frame(&c, &project, 4_499_999, &provider).pixel(320, 240);
        assert!(end[2] > 250 && end[0] < 5, "{end:?}");
    }

    /// The blend must not care which decoder produced either side.
    ///
    /// Hardware decode is the default on any machine that can import a decoded
    /// surface as a texture, so a transition that only works on the software
    /// path is one that fails on most real footage — and fails *plausibly*, as
    /// a wrong-looking mix rather than as an error. White and black are used
    /// because they land on the same code values through both paths, so the one
    /// expected number covers all four combinations.
    #[test]
    fn a_crossfade_is_the_same_however_each_side_was_decoded() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let project = cut_project(TransitionKind::Dissolve, 1_000_000, (640, 480));

        let white_sw = TestSource::Rgba([255, 255, 255, 255], 640, 480);
        let black_sw = TestSource::Rgba([0, 0, 0, 255], 640, 480);
        let white_hw = TestSource::Planar(255, 128, 128, 640, 480);
        let black_hw = TestSource::Planar(0, 128, 128, 640, 480);

        for (name, left, right) in [
            ("software over software", white_sw, black_sw),
            ("hardware over software", white_hw, black_sw),
            ("software over hardware", white_sw, black_hw),
            ("hardware over hardware", white_hw, black_hw),
        ] {
            let provider = MixedProvider::default().with("left", left).with("right", right);

            let start = frame(&c, &project, 3_500_000, &provider).pixel(320, 240);
            assert!(start[0] > 250, "{name}: the window opens on white, got {start:?}");
            let end = frame(&c, &project, 4_499_999, &provider).pixel(320, 240);
            assert!(end[0] < 5, "{name}: the window closes on black, got {end:?}");

            // Half white and half black *in linear light* is 0.5, which an sRGB
            // target stores as 188 — not 128. Asserting the linear number is
            // the point: a crossfade computed on the encoded bytes would give
            // 128 here, and it is the classic dark-through-the-middle
            // dissolve that everyone can see and nobody can name.
            let middle = frame(&c, &project, 4_000_000, &provider).pixel(320, 240);
            for (channel, value) in [("r", middle[0]), ("g", middle[1]), ("b", middle[2])] {
                assert!(
                    (value as i32 - 188).abs() <= 3,
                    "{name}: {channel} is {value}, expected the linear-light midpoint"
                );
            }
            assert_eq!(middle[3], 255, "{name}: the frame is opaque");
        }
    }

    /// The bars around a letterboxed clip have to stay background.
    ///
    /// This is the assertion that pins the straight-alpha layer: a layer drawn
    /// with ordinary source-over blending over a transparent clear holds
    /// *premultiplied* colour, the transition shader premultiplies it a second
    /// time, and the visible symptom is a dark halo creeping in from wherever
    /// the clip does not cover the canvas. Here that would show as the blue
    /// background going dark for the duration of the transition.
    #[test]
    fn a_transition_between_letterboxed_clips_leaves_the_background_alone() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // 16:9 sources on a 4:3 canvas: 60 rows of background top and bottom.
        let mut project = cut_project(TransitionKind::Dissolve, 1_000_000, (1920, 1080));
        project.canvas.background = [0.0, 0.0, 1.0, 1.0];
        let provider = MixedProvider::default()
            .with("left", TestSource::Rgba([255, 0, 0, 255], 1920, 1080))
            .with("right", TestSource::Rgba([0, 255, 0, 255], 1920, 1080));

        let f = frame(&c, &project, 4_000_000, &provider);
        assert_eq!(f.pixel(320, 5), [0, 0, 255, 255], "top band");
        assert_eq!(f.pixel(320, 474), [0, 0, 255, 255], "bottom band");
        let middle = f.pixel(320, 240);
        assert!(
            (middle[0] as i32 - 128).abs() <= 2 && (middle[1] as i32 - 128).abs() <= 2,
            "and the picture itself still crossfades, got {middle:?}"
        );
    }

    /// A half-transparent clip must look the same just inside the window as
    /// just outside it.
    ///
    /// This is the assertion the letterbox one above cannot make, because two
    /// opaque clips hide the bug: a layer drawn with ordinary source-over
    /// blending over a transparent clear holds *premultiplied* colour, and
    /// `transition.wgsl` premultiplies what it samples a second time. At
    /// opacity 0.5 that is half the brightness it should be — and it appears
    /// the instant the window opens, so the clip visibly darkens as the
    /// transition begins and brightens again as it ends.
    #[test]
    fn a_half_transparent_clip_does_not_change_brightness_when_the_window_opens() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = cut_project(TransitionKind::Dissolve, 1_000_000, (640, 480));
        project.tracks[0].segments[0].transform.opacity = 0.5;
        let provider = MixedProvider::default().with("left", RED).with("right", BLUE);

        // Outside the window: half of red over black, through the ordinary
        // quad path.
        let outside = frame(&c, &project, 3_000_000, &provider).pixel(320, 240);
        assert!((outside[0] as i32 - 128).abs() <= 2, "{outside:?}");

        // The first instant of the window is progress 0, which is the outgoing
        // clip and nothing else — so it has to be the same pixel.
        let inside = frame(&c, &project, 3_500_000, &provider).pixel(320, 240);
        assert!(
            (inside[0] as i32 - outside[0] as i32).abs() <= 2,
            "the clip darkened when the transition began: {outside:?} became {inside:?}"
        );
        assert_eq!(inside[3], 255, "the frame stays opaque over the background");
    }

    #[test]
    fn a_dip_reaches_its_colour_at_the_midpoint_and_neither_clip_shows_through() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = cut_project(TransitionKind::DipToColor, 1_000_000, (640, 480));
        transition_mut(&mut project).color = [0.0, 1.0, 0.0, 1.0];
        let provider = MixedProvider::default().with("left", RED).with("right", BLUE);

        assert_eq!(
            frame(&c, &project, 4_000_000, &provider).pixel(320, 240),
            [0, 255, 0, 255],
            "the middle of a dip is the colour and nothing else"
        );
        // A quarter of the way in, the colour is half laid over the outgoing
        // clip: half red, half green, no blue at all.
        let quarter = frame(&c, &project, 3_750_000, &provider).pixel(320, 240);
        assert!(
            (quarter[0] as i32 - 128).abs() <= 2
                && (quarter[1] as i32 - 128).abs() <= 2
                && quarter[2] == 0,
            "{quarter:?}"
        );
    }

    #[test]
    fn a_wipe_puts_the_edge_where_the_progress_says() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = cut_project(TransitionKind::Wipe, 1_000_000, (640, 480));
        {
            let transition = transition_mut(&mut project);
            transition.direction = TransitionDirection::Right;
            transition.softness = 0.0;
        }
        let provider = MixedProvider::default().with("left", RED).with("right", BLUE);

        let f = frame(&c, &project, 4_000_000, &provider);
        assert_eq!(f.pixel(4, 240), [0, 0, 255, 255], "the left half has arrived");
        assert_eq!(f.pixel(635, 240), [255, 0, 0, 255], "the right half has not");
    }

    /// A transition costs two source lookups and no more.
    ///
    /// Worth pinning because the obvious wrong implementation — resolving the
    /// transition per segment rather than per instant — asks for both sides
    /// twice, and on the export path a source lookup is a decode.
    #[test]
    fn a_transition_asks_for_each_side_exactly_once() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let project = cut_project(TransitionKind::Dissolve, 1_000_000, (640, 480));
        let provider = MixedProvider::default().with("left", RED).with("right", BLUE);

        frame(&c, &project, 4_000_000, &provider);
        assert_eq!(provider.calls(), 2, "inside the window, both sides");

        let outside = MixedProvider::default().with("left", RED).with("right", BLUE);
        frame(&c, &project, 3_000_000, &outside);
        assert_eq!(outside.calls(), 1, "outside it, only the live clip");
    }

    /// A transition whose neighbours no longer touch renders as an ordinary
    /// cut rather than as anything at all.
    ///
    /// `timeline::ops::detach_broken_transitions` removes the material when an
    /// edit breaks the join, but the compositor must not depend on that having
    /// happened — a document opened from disk, or one mid-edit, can carry a
    /// transition whose cut has gone.
    #[test]
    fn a_transition_whose_cut_has_gone_does_not_render() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let mut project = cut_project(TransitionKind::Dissolve, 1_000_000, (640, 480));
        // Trim the outgoing clip so the two no longer meet.
        project.tracks[0].segments[0].target_range.duration = 3_000_000;
        let provider = MixedProvider::default().with("left", RED).with("right", BLUE);

        // The gap shows the background, and the clip on each side is whole.
        assert_eq!(frame(&c, &project, 2_500_000, &provider).pixel(320, 240), [255, 0, 0, 255]);
        assert_eq!(frame(&c, &project, 3_500_000, &provider).pixel(320, 240), [0, 0, 0, 255]);
        assert_eq!(frame(&c, &project, 4_100_000, &provider).pixel(320, 240), [0, 0, 255, 255]);
    }

    /// A missing source on one side fades to nothing rather than losing the
    /// frame.
    #[test]
    fn a_transition_with_one_side_missing_still_renders_the_other() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let project = cut_project(TransitionKind::Dissolve, 1_000_000, (640, 480));
        // Only the outgoing clip has a source; the incoming one answers `None`.
        let provider = MixedProvider::default().with("left", RED);

        let middle = frame(&c, &project, 4_000_000, &provider).pixel(320, 240);
        assert!(
            (middle[0] as i32 - 128).abs() <= 2 && middle[3] == 255,
            "half the outgoing clip over the background, got {middle:?}"
        );
    }

    /// Turn one NV12 frame back into RGB the way a decoder would.
    ///
    /// The forward direction is `yuv.wgsl`'s `luma`/`chroma`, which are BT.601
    /// limited; this is their inverse, spelled out rather than reused so a
    /// change to one of them fails this test rather than cancelling out in it.
    fn nv12_pixel(frame: &Nv12Frame, x: u32, y: u32) -> [u8; 3] {
        let (cb, cr) = frame.chroma(x, y);
        let y = (frame.luma(x, y) as f32 - 16.0) * (255.0 / 219.0) / 255.0;
        let cb = (cb as f32 - 128.0) * (255.0 / 224.0) / 255.0;
        let cr = (cr as f32 - 128.0) * (255.0 / 224.0) / 255.0;
        let r = y + 1.402 * cr;
        let g = y - 0.344136 * cb - 0.714136 * cr;
        let b = y + 1.772 * cb;
        [
            (r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (b.clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }

    /// The preview and the export must show the same transition.
    ///
    /// They do not share a code path all the way down: the preview reads the
    /// composited target back as RGBA and JPEG-encodes it, while a hardware
    /// export runs the RGBA→NV12 compute pass over the same target and never
    /// touches system memory in between. Both call `render_to_texture`, which
    /// is where this work went in — so this test is really the claim that the
    /// transition went in *there* and not into one of the two callers.
    ///
    /// The frame is deliberately taken mid-transition, where the two paths have
    /// the most to disagree about, and at three points across it.
    #[test]
    fn the_preview_and_the_export_agree_on_a_frame_mid_transition() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let project = cut_project(TransitionKind::Dissolve, 1_000_000, (640, 480));
        let provider = MixedProvider::default()
            // One side hardware-decoded and one side software, so the
            // comparison covers the planar path too.
            .with("left", TestSource::Planar(255, 128, 128, 640, 480))
            .with("right", TestSource::Rgba([0, 0, 255, 255], 640, 480));

        for at in [3_600_000, 4_000_000, 4_400_000] {
            let preview = c
                .render(&project, at, (640, 480), &provider)
                .expect("preview render");
            let Ok(export) = c.render_nv12(&project, at, (640, 480), &provider) else {
                eprintln!("skipping: no RGBA to NV12 compute pass on this device");
                return;
            };

            for (x, y) in [(0, 0), (320, 240), (639, 479), (17, 300)] {
                let expected = preview.pixel(x, y);
                let actual = nv12_pixel(&export, x, y);
                for channel in 0..3 {
                    assert!(
                        (expected[channel] as i32 - actual[channel] as i32).abs() <= 3,
                        "at {at} µs, ({x},{y}): preview {expected:?} against export {actual:?}"
                    );
                }
            }

            // And the two are genuinely mid-transition rather than agreeing on
            // a frame where nothing is happening.
            let [r, _, b, _] = preview.pixel(320, 240);
            assert!(r > 10 && b > 10, "expected both clips at {at} µs, got {:?}", preview.pixel(320, 240));
        }
    }

    #[test]
    fn oversized_frames_are_refused_rather_than_allocated() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let project = project([0.0, 0.0, 0.0, 1.0]);
        let provider = SolidColorProvider::new();
        let too_big = c.context().max_texture_dimension_2d() + 1;
        assert!(c.render(&project, 0, (too_big, 16), &provider).is_err());
        assert!(c.render(&project, 0, (0, 16), &provider).is_err());
    }
}
