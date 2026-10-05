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
use super::nv12::{Nv12Converter, Nv12Frame, Nv12PlaneWriter, READ_FORMAT};
use super::source::{SourceFrame, SourceProvider, SourceRequest};
use super::texture_pool::{PooledTexture, TextureKey, TexturePool, DEFAULT_BUDGET_BYTES};
use crate::modules::fx::{self, FxInstance, FxRenderer, OverDraw};
use crate::modules::motion;
use crate::modules::project::document::{MaterialKind, MaterialPool, Micros, Project, Segment};
use crate::modules::project::document::{TransitionDirection, TransitionKind};
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
    /// Colour adjustments as `(brightness, contrast, saturation, temperature)`.
    /// Read by the shader only when `color_active` is 1.
    color: [f32; 4],
    /// The LUT's `DOMAIN_MIN` in xyz, its intensity in w. Read only when
    /// `lut_active` is 1.
    lut_lo: [f32; 4],
    /// The LUT's `DOMAIN_MAX` in xyz, its edge size `N` (as a float) in w.
    lut_hi: [f32; 4],
    opacity: f32,
    /// 1 when the source is two YUV planes rather than one RGBA texture.
    planar: u32,
    /// [`super::source::YuvMatrix`] and [`super::source::YuvRange`] as their
    /// discriminants.
    matrix: u32,
    range: u32,
    /// Clockwise quarter turns the shader must apply to the texture coordinate.
    turns: u32,
    /// 1 when `color` holds a non-identity adjustment. A flag rather than
    /// identity values, so an ungraded clip takes exactly the code path it
    /// always took — the byte-identity test below depends on it.
    color_active: u32,
    /// 1 when the bind group's LUT texture is this clip's LUT rather than the
    /// placeholder. Gated like `color_active`, and also how a missing LUT
    /// file renders the clip untouched.
    lut_active: u32,
    /// The extended grade's live stages, [`super::grade::feature`] bits.
    features: u32,
    tone: [f32; 4],
    tone2: [f32; 4],
    detail: [f32; 4],
    vignette: [f32; 4],
    lift: [f32; 4],
    gain: [f32; 4],
    gamma: [f32; 4],
    offset: [f32; 4],
    hsl: [[f32; 4]; 8],
    /// Masks and chroma key, [`super::matte::MatteBlock`]. Flags 0 for a
    /// clip with neither, which then takes the path it always took.
    matte_flags: [u32; 4],
    matte_size: [f32; 4],
    key: [f32; 4],
    key2: [f32; 4],
    masks: [[f32; 4]; crate::modules::project::compositing::MAX_MASKS * 3],
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
    /// The premultiplied quad summed additively into a float layer: several
    /// weighted draws of one clip (frame blending, motion blur). See
    /// `render::accumulate`.
    accumulate_pipeline: wgpu::RenderPipeline,
    /// Turns that float layer back into a clip layer, built on first use.
    resolver: OnceLock<super::accumulate::Resolver>,
    /// Effects limited by a matte. See [`super::matte_mix`].
    matte_mix: OnceLock<super::matte_mix::MatteMix>,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// What binding 2 gets when the source is RGBA and there is no chroma
    /// plane. One neutral texel, never sampled — the shader's `planar` branch
    /// does not read it — but the binding still has to exist.
    chroma_placeholder: wgpu::TextureView,
    /// What binding 3 gets when the segment has no LUT, for the same
    /// no-optional-bindings reason.
    lut_placeholder: wgpu::TextureView,
    /// Path → uploaded 3D LUT, revalidated by mtime per lookup. Runtime state
    /// only — the document stores nothing but the path.
    luts: super::lut::LutCache,
    /// What binding 4 gets when the segment has no tone curves.
    curve_placeholder: wgpu::TextureView,
    /// Curve points → baked tables. See [`super::grade::CurveCache`].
    curves: super::grade::CurveCache,
    /// What binding 5 gets when the segment has no background matte: one
    /// opaque texel, never read without `M_BACKGROUND`.
    matte_placeholder: wgpu::TextureView,
    /// Baked "Remove background" mattes. See [`super::background`].
    backgrounds: super::background::MatteFrames,
    /// Baked optical-flow frames. See [`super::flow`].
    flows: super::flow::FlowFrames,
    /// Face landmark tracks, for the retouch effect (`fx::retouch`).
    faces: super::faces::FaceTracks,
    /// Remade frames ("Remove object", "Enhance quality"). See
    /// [`super::enhance`].
    enhanced: super::enhance::EnhancedFrames,
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
    /// The RGBA→NV12 *render* pass, built on first use.
    ///
    /// Separate from `nv12` above because it writes two images rather than one
    /// buffer, and lazy for the same reason: two more shader compiles that only
    /// the preview's surface-drawing path ever wants.
    nv12_planes: OnceLock<Nv12PlaneWriter>,
    /// The transition pipelines, built on first use.
    ///
    /// Lazy for the same reason `nv12` is: it is five shader compiles, and a
    /// project with no transitions in it should not pay for them. Built once
    /// the first frame containing a transition is rendered, which is a stutter
    /// on that frame and never again — `TransitionPipeline::new` builds every
    /// kind at once precisely so scrubbing into a second kind does not compile
    /// anything.
    transitions: OnceLock<TransitionPipeline>,
    /// The built-in effect pipelines (`modules/fx`), built on first use for
    /// the same reason as `transitions`: a project with no effects in it
    /// should not pay for the shader module.
    fx: OnceLock<FxRenderer>,
    /// Nested views and nested frames of compound clips, kept between
    /// frames. See [`super::nested`]. Never held across a render: a nested
    /// render reaches it again from inside.
    nested: Mutex<super::nested::NestedCache>,
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
                // The clip's 3D LUT, likewise always bound. Non-filterable
                // because it is `Rgba32Float` read with `textureLoad` only —
                // the shader interpolates by hand for exactness; see
                // `sample_lut` in `quad.wgsl`.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                // The clip's baked tone curves, likewise always bound and
                // read with `textureLoad` only; see `curve_at` in `quad.wgsl`.
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The clip's "Remove background" matte, filtered (it is
                // stored smaller than most sources), likewise always bound.
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
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

        let quad_pipeline = |label: &str,
                             entry: &str,
                             blend: Option<wgpu::BlendState>,
                             format: wgpu::TextureFormat| {
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
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        // Source-over. The shader premultiplies (`fs_premultiplied`) rather
        // than leaving `SrcAlpha` to the blender: the sum is the same, but a
        // blender may round its factors to the target's 8 bits first, and
        // NVIDIA does, which steps the faint end of every mask feather and
        // every low opacity. See `fs_premultiplied` in `quad.wgsl`.
        let pipeline = quad_pipeline(
            "chukcut quad pipeline",
            "fs_premultiplied",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            config.format,
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
        let layer_pipeline = quad_pipeline(
            "chukcut quad layer pipeline",
            "fs_main",
            None,
            config.format,
        );
        let accumulate_pipeline = quad_pipeline(
            "chukcut quad accumulate pipeline",
            "fs_premultiplied",
            Some(super::accumulate::ADDITIVE),
            super::accumulate::ACCUMULATION_FORMAT,
        );

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
        let chroma_placeholder = placeholder.create_view(&wgpu::TextureViewDescriptor::default());

        // The dead LUT binding: one black texel, never read, because the
        // shader's `lut_active` branch does not run without a real LUT bound.
        let lut_placeholder = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("chukcut lut placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let lut_placeholder = lut_placeholder.create_view(&wgpu::TextureViewDescriptor::default());

        // The dead curve binding, for the same reason.
        let curve_placeholder = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("chukcut curve placeholder"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

        // The dead background-matte binding: one opaque texel.
        let matte_placeholder = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("chukcut matte placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &matte_placeholder,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let matte_placeholder =
            matte_placeholder.create_view(&wgpu::TextureViewDescriptor::default());

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
        let uniform_stride = (std::mem::size_of::<QuadUniform>() as u64).div_ceil(align) * align;

        Self {
            ctx,
            pool: TexturePool::new(config.texture_budget_bytes),
            config,
            pipeline,
            layer_pipeline,
            accumulate_pipeline,
            resolver: OnceLock::new(),
            matte_mix: OnceLock::new(),
            uniform_layout,
            texture_layout,
            sampler,
            chroma_placeholder,
            lut_placeholder,
            luts: super::lut::LutCache::default(),
            curve_placeholder,
            curves: super::grade::CurveCache::default(),
            matte_placeholder,
            backgrounds: super::background::MatteFrames::default(),
            flows: super::flow::FlowFrames::default(),
            faces: super::faces::FaceTracks::default(),
            enhanced: super::enhance::EnhancedFrames::default(),
            vertices,
            indices,
            uniforms: Mutex::new(Scratch::default()),
            readback: Mutex::new(Scratch::default()),
            uniform_stride: uniform_stride as u32,
            stats: StatCounters::default(),
            nv12: OnceLock::new(),
            nv12_planes: OnceLock::new(),
            transitions: OnceLock::new(),
            fx: OnceLock::new(),
            nested: Mutex::new(Default::default()),
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

    /// How the compound clip cache has done since this compositor was made.
    pub fn nested_stats(&self) -> super::nested::NestedStats {
        self.nested.lock().stats()
    }

    /// Drop every kept nested view and frame. Nothing needs this for
    /// correctness — the keys change with the document — but a finished
    /// render session can give the texture memory back.
    pub fn clear_nested(&self) {
        self.nested.lock().clear();
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
        TextureKey::new(size.0, size.1, self.config.format, TARGET_USAGE).viewable_as(READ_FORMAT)
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

    /// Composite and draw NV12 straight into two images somebody else owns.
    ///
    /// The other zero-copy entry point, and the one that works on Intel's JPEG
    /// engine. [`Self::render_nv12_into_range`] writes a buffer whose layout we
    /// chose and hands the media driver a `DRM_FORMAT_MOD_LINEAR` descriptor;
    /// this writes two colour attachments over a surface the **media driver**
    /// allocated, so the tiling is the encoder's own and nothing here declares
    /// anything about it. `docs/research/preview-zerocopy-jpeg.md` is why the
    /// distinction exists.
    ///
    /// Returns once the GPU has finished, which is the only synchronisation the
    /// encoder on the other side can be given.
    #[allow(clippy::too_many_arguments)]
    pub fn render_nv12_into_planes(
        &self,
        project: &Project,
        time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
        luma: &wgpu::TextureView,
        chroma: &wgpu::TextureView,
        range: crate::modules::render::source::YuvRange,
    ) -> Result<()> {
        let writer = self.nv12_planes();
        let target = self.render_to_texture(project, time, size, sources)?;

        let started = Instant::now();
        let before_wait = writer.wait_ns.load(Ordering::Relaxed);
        let result = writer.write_planes(&self.ctx, &target, luma, chroma, range);
        let waited = writer.wait_ns.load(Ordering::Relaxed) - before_wait;
        add(&self.stats.nv12_ns, started);
        self.stats
            .readback_wait_ns
            .fetch_add(waited, Ordering::Relaxed);

        self.pool.release(target);
        result
    }

    /// The compute converter, built once. `None` on a device where it will not
    /// build, which is a reason to fall back rather than to fail.
    fn nv12_converter(&self) -> Option<&Nv12Converter> {
        self.nv12
            .get_or_init(|| Some(Nv12Converter::new(&self.ctx)))
            .as_ref()
    }

    /// The plane writer, built once. Unlike the compute converter this has no
    /// `None` case: a render pass to an `R8Unorm` target is core WebGPU, so if
    /// it will not build there is nothing to fall back *from*.
    fn nv12_planes(&self) -> &Nv12PlaneWriter {
        self.nv12_planes
            .get_or_init(|| Nv12PlaneWriter::new(&self.ctx))
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
            let (lut_lo, lut_hi) = match &quad.lut {
                Some((lut, intensity)) => (
                    [
                        lut.domain_min[0],
                        lut.domain_min[1],
                        lut.domain_min[2],
                        *intensity,
                    ],
                    [
                        lut.domain_max[0],
                        lut.domain_max[1],
                        lut.domain_max[2],
                        lut.size as f32,
                    ],
                ),
                // Never read — `lut_active` is 0 — but a degenerate domain
                // would still be a division by zero waiting for a bug, so the
                // dead values are a sane identity domain.
                None => ([0.0, 0.0, 0.0, 0.0], [1.0, 1.0, 1.0, 2.0]),
            };
            let grade = &quad.grade;
            let block = QuadUniform {
                mvp: quad.placement.mvp,
                crop: quad.placement.crop,
                color: quad.color.unwrap_or([0.0, 1.0, 1.0, 0.0]),
                lut_lo,
                lut_hi,
                opacity: quad.placement.opacity,
                planar: u32::from(quad.frame.is_planar()),
                matrix: quad.frame.matrix as u32,
                range: quad.frame.range as u32,
                turns: quad.frame.turns % 4,
                color_active: u32::from(quad.color.is_some()),
                lut_active: u32::from(quad.lut.is_some()),
                features: grade.features,
                tone: grade.tone,
                tone2: grade.tone2,
                detail: grade.detail,
                vignette: grade.vignette,
                lift: grade.lift,
                gain: grade.gain,
                gamma: grade.gamma,
                offset: grade.offset,
                hsl: grade.hsl,
                matte_flags: quad.matte.flags,
                matte_size: quad.matte.size,
                key: quad.matte.key,
                key2: quad.matte.key2,
                masks: quad.matte.masks,
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
            source_groups[quad.slot as usize] = Some(self.source_group(device, quad));
        }

        let bg = project.canvas.background;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("chukcut frame"),
        });

        // The effect passes of this frame, if it has any. Opened only when an
        // item needs it, so a frame without effects takes exactly the path it
        // always took.
        let needs_fx = draws.items.iter().any(|item| match item {
            Draw::Effected { .. }
            | Draw::Adjust { .. }
            | Draw::Blended { .. }
            | Draw::Accumulated { .. } => true,
            Draw::Transition { from_fx, to_fx, .. } => !from_fx.is_empty() || !to_fx.is_empty(),
            Draw::Quad(_) => false,
        });
        let mut fx_frame = needs_fx.then(|| self.fx_renderer().begin(&self.ctx, &self.pool));

        // Every transition's two layers, and every effected clip's layer,
        // drawn into full-canvas targets of their own before the frame is
        // composited. They have to be complete passes rather than draws inside
        // the composite pass: a render pass cannot sample the attachment it is
        // writing.
        let mut layer_targets: Vec<PooledTexture> = Vec::new();
        let mut layer_groups: Vec<Option<wgpu::BindGroup>> = Vec::with_capacity(draws.items.len());
        let mut overs: Vec<Option<OverDraw>> = Vec::with_capacity(draws.items.len());
        // A blended clip's finished layer, by item, for the blend pass that
        // lays it on once the frame beneath it is composited.
        let mut blend_layers: Vec<Option<PooledTexture>> = Vec::with_capacity(draws.items.len());
        for item in &draws.items {
            match item {
                Draw::Transition {
                    from,
                    to,
                    from_fx,
                    to_fx,
                    ..
                } => {
                    let pipeline = self.transition_pipeline();
                    let mut sides = Vec::with_capacity(2);
                    for (quad, chain) in [(from, from_fx), (to, to_fx)] {
                        let layer = self.pool.acquire(device, self.target_key(size));
                        self.draw_layer(
                            &mut encoder,
                            &uniform_group,
                            &source_groups,
                            quad.as_ref(),
                            layer.view(),
                        );
                        let layer = match fx_frame.as_mut().filter(|_| !chain.is_empty()) {
                            Some(frame) => {
                                let out = frame.apply(
                                    &mut encoder,
                                    layer.view(),
                                    size,
                                    chain,
                                    self.target_key(size),
                                );
                                layer_targets.push(layer);
                                out
                            }
                            None => layer,
                        };
                        sides.push(layer);
                    }
                    layer_groups.push(Some(pipeline.bind_layers(
                        &self.ctx,
                        sides[0].view(),
                        sides[1].view(),
                    )));
                    overs.push(None);
                    blend_layers.push(None);
                    layer_targets.extend(sides);
                }
                Draw::Effected {
                    quad,
                    chain,
                    mask,
                    neighbours,
                } => {
                    let frame = fx_frame.as_mut().expect("opened for an effected clip");
                    let layer = self.pool.acquire(device, self.target_key(size));
                    self.draw_layer(
                        &mut encoder,
                        &uniform_group,
                        &source_groups,
                        Some(quad),
                        layer.view(),
                    );
                    let around = self.neighbour_layers(
                        &mut encoder,
                        &uniform_group,
                        &source_groups,
                        neighbours,
                        size,
                    );
                    let out = frame.apply_with_neighbours(
                        &mut encoder,
                        layer.view(),
                        size,
                        chain,
                        self.target_key(size),
                        [
                            around[0].as_ref().map(PooledTexture::view),
                            around[1].as_ref().map(PooledTexture::view),
                        ],
                    );
                    layer_targets.extend(around.into_iter().flatten());
                    let out = self.masked(
                        &mut encoder,
                        &uniform_group,
                        &source_groups,
                        &layer,
                        out,
                        mask.as_ref(),
                        size,
                        &mut layer_targets,
                    );
                    overs.push(Some(frame.prepare_over(
                        out.view(),
                        self.config.format,
                        size,
                    )));
                    layer_groups.push(None);
                    blend_layers.push(None);
                    layer_targets.push(layer);
                    layer_targets.push(out);
                }
                Draw::Blended {
                    quad,
                    chain,
                    mask,
                    neighbours,
                    ..
                } => {
                    let frame = fx_frame.as_mut().expect("opened for a blended clip");
                    let layer = self.pool.acquire(device, self.target_key(size));
                    self.draw_layer(
                        &mut encoder,
                        &uniform_group,
                        &source_groups,
                        Some(quad),
                        layer.view(),
                    );
                    let layer = if chain.is_empty() {
                        layer
                    } else {
                        let around = self.neighbour_layers(
                            &mut encoder,
                            &uniform_group,
                            &source_groups,
                            neighbours,
                            size,
                        );
                        let out = frame.apply_with_neighbours(
                            &mut encoder,
                            layer.view(),
                            size,
                            chain,
                            self.target_key(size),
                            [
                                around[0].as_ref().map(PooledTexture::view),
                                around[1].as_ref().map(PooledTexture::view),
                            ],
                        );
                        layer_targets.extend(around.into_iter().flatten());
                        let out = self.masked(
                            &mut encoder,
                            &uniform_group,
                            &source_groups,
                            &layer,
                            out,
                            mask.as_ref(),
                            size,
                            &mut layer_targets,
                        );
                        layer_targets.push(layer);
                        out
                    };
                    blend_layers.push(Some(layer));
                    layer_groups.push(None);
                    overs.push(None);
                }
                Draw::Accumulated {
                    quads,
                    chain,
                    mode,
                    mask,
                } => {
                    let frame = fx_frame.as_mut().expect("opened for an accumulated clip");
                    let (sum, layer) = self.accumulate_layer(
                        &mut encoder,
                        &uniform_group,
                        &source_groups,
                        quads,
                        size,
                    );
                    layer_targets.push(sum);
                    let layer = if chain.is_empty() {
                        layer
                    } else {
                        let out = frame.apply(
                            &mut encoder,
                            layer.view(),
                            size,
                            chain,
                            self.target_key(size),
                        );
                        let out = self.masked(
                            &mut encoder,
                            &uniform_group,
                            &source_groups,
                            &layer,
                            out,
                            mask.as_ref(),
                            size,
                            &mut layer_targets,
                        );
                        layer_targets.push(layer);
                        out
                    };
                    layer_groups.push(None);
                    if mode.is_some() {
                        blend_layers.push(Some(layer));
                        overs.push(None);
                    } else {
                        overs.push(Some(frame.prepare_over(
                            layer.view(),
                            self.config.format,
                            size,
                        )));
                        blend_layers.push(None);
                        layer_targets.push(layer);
                    }
                }
                Draw::Quad(_) | Draw::Adjust { .. } => {
                    layer_groups.push(None);
                    overs.push(None);
                    blend_layers.push(None);
                }
            }
        }

        // The composite, in painter's order. An effect clip splits it: the
        // pass so far is ended, the effect clip's effects run over what it
        // drew into a fresh target, and compositing carries on into that. A
        // clip with a blend mode splits it the same way: its layer is laid
        // onto the frame so far by the blend pass, into a fresh target.
        let mut target = target;
        let mut retired: Vec<PooledTexture> = Vec::new();
        let mut transition_slot = 0u32;
        let mut start = 0usize;
        loop {
            let end = draws.items[start..]
                .iter()
                .position(|item| {
                    matches!(
                        item,
                        Draw::Adjust { .. }
                            | Draw::Blended { .. }
                            | Draw::Accumulated { mode: Some(_), .. }
                    )
                })
                .map_or(draws.items.len(), |at| start + at);
            {
                let load = if start == 0 {
                    wgpu::LoadOp::Clear(wgpu::Color {
                        r: bg[0] as f64,
                        g: bg[1] as f64,
                        b: bg[2] as f64,
                        a: bg[3] as f64,
                    })
                } else {
                    wgpu::LoadOp::Load
                };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("chukcut composite"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target.view(),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });

                for (i, item) in draws.items.iter().enumerate().take(end).skip(start) {
                    match item {
                        Draw::Quad(quad) => {
                            let Some(group) = source_groups[quad.slot as usize].as_ref() else {
                                continue;
                            };
                            // Set per draw rather than once per pass: an
                            // effected clip or a transition in between binds
                            // pipelines of its own.
                            pass.set_pipeline(&self.pipeline);
                            pass.set_vertex_buffer(0, self.vertices.slice(..));
                            pass.set_index_buffer(
                                self.indices.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                            pass.set_bind_group(
                                0,
                                &uniform_group,
                                &[quad.slot * self.uniform_stride],
                            );
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
                        Draw::Effected { .. } | Draw::Accumulated { mode: None, .. } => {
                            if let Some(over) = overs[i].as_ref() {
                                over.draw(&mut pass);
                            }
                        }
                        Draw::Adjust { .. }
                        | Draw::Blended { .. }
                        | Draw::Accumulated { mode: Some(_), .. } => {
                            unreachable!("a pass ends at an effect clip or a blended clip")
                        }
                    }
                }
            }
            if end >= draws.items.len() {
                break;
            }
            let frame = fx_frame
                .as_mut()
                .expect("opened for an effect or blended clip");
            let out = match &draws.items[end] {
                Draw::Adjust { chain } => frame.apply(
                    &mut encoder,
                    target.view(),
                    size,
                    chain,
                    self.target_key(size),
                ),
                Draw::Blended { mode, .. }
                | Draw::Accumulated {
                    mode: Some(mode), ..
                } => {
                    let layer = blend_layers[end].as_ref().expect("drawn above");
                    frame.blend(
                        &mut encoder,
                        target.view(),
                        layer.view(),
                        size,
                        *mode,
                        self.target_key(size),
                    )
                }
                _ => unreachable!("found by position above"),
            };
            retired.push(std::mem::replace(&mut target, out));
            start = end + 1;
        }
        self.ctx.queue().submit(Some(encoder.finish()));
        drop(uniforms);
        for layer in layer_targets
            .into_iter()
            .chain(retired)
            .chain(blend_layers.into_iter().flatten())
        {
            self.pool.release(layer);
        }
        if let Some(frame) = fx_frame {
            frame.finish();
        }
        add(&self.stats.composite_ns, composited);
        self.stats.frames.fetch_add(1, Ordering::Relaxed);

        Ok(target)
    }

    /// The built-in effect renderer, built on first use. See [`Self::fx`].
    fn fx_renderer(&self) -> &FxRenderer {
        self.fx.get_or_init(|| FxRenderer::new(&self.ctx))
    }

    /// The transition pipelines, built on first use. See [`Self::transitions`].
    fn transition_pipeline(&self) -> &TransitionPipeline {
        self.transitions
            .get_or_init(|| TransitionPipeline::new(&self.ctx, self.config.format))
    }

    fn source_group(&self, device: &wgpu::Device, quad: &QuadDraw) -> wgpu::BindGroup {
        let frame = &quad.frame;
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(
                        quad.lut
                            .as_ref()
                            .map(|(lut, _)| &lut.view)
                            .unwrap_or(&self.lut_placeholder),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(
                        quad.curves
                            .as_ref()
                            .map(|curves| &curves.view)
                            .unwrap_or(&self.curve_placeholder),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(
                        quad.background
                            .as_ref()
                            .map(|matte| &matte.view)
                            .unwrap_or(&self.matte_placeholder),
                    ),
                },
            ],
        })
    }

    /// A temporal denoise's neighbour quads, each drawn into a layer of its
    /// own the way the clip's is. `None` where there is no neighbour.
    fn neighbour_layers(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniform_group: &wgpu::BindGroup,
        source_groups: &[Option<wgpu::BindGroup>],
        neighbours: &Neighbours,
        size: (u32, u32),
    ) -> [Option<PooledTexture>; 2] {
        neighbours.each_ref().map(|quad| {
            quad.as_ref().map(|quad| {
                let layer = self.pool.acquire(self.ctx.device(), self.target_key(size));
                self.draw_layer(
                    encoder,
                    uniform_group,
                    source_groups,
                    Some(quad),
                    layer.view(),
                );
                layer
            })
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

    /// `after` (a clip's layer with its effects) limited by `mask`: mixed
    /// with `before` (the layer without them) by the clip's matte. `after`
    /// itself when there is no mask. The textures the mix reads go into
    /// `keep`, which outlives the submit.
    #[allow(clippy::too_many_arguments)]
    fn masked(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniform_group: &wgpu::BindGroup,
        source_groups: &[Option<wgpu::BindGroup>],
        before: &PooledTexture,
        after: PooledTexture,
        mask: Option<&FxMask>,
        size: (u32, u32),
        keep: &mut Vec<PooledTexture>,
    ) -> PooledTexture {
        let Some(mask) = mask else {
            return after;
        };
        let device = self.ctx.device();
        let weights = self.pool.acquire(device, self.target_key(size));
        self.draw_layer(
            encoder,
            uniform_group,
            source_groups,
            Some(&mask.quad),
            weights.view(),
        );
        let mixed = self.pool.acquire(device, self.target_key(size));
        self.matte_mix
            .get_or_init(|| super::matte_mix::MatteMix::new(&self.ctx, self.config.format))
            .mix(
                &self.ctx,
                encoder,
                before.view(),
                after.view(),
                weights.view(),
                mask.background,
                mixed.view(),
            );
        keep.push(after);
        keep.push(weights);
        mixed
    }

    /// Several weighted draws of one clip summed in a float layer, then
    /// resolved into an ordinary straight-alpha clip layer. Answers both
    /// textures: the sum has to outlive the submit as much as the layer does.
    /// See `render::accumulate`.
    fn accumulate_layer(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniform_group: &wgpu::BindGroup,
        source_groups: &[Option<wgpu::BindGroup>],
        quads: &[QuadDraw],
        size: (u32, u32),
    ) -> (PooledTexture, PooledTexture) {
        let device = self.ctx.device();
        let sum = self.pool.acquire(
            device,
            TextureKey::new(
                size.0,
                size.1,
                super::accumulate::ACCUMULATION_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("chukcut accumulate"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: sum.view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.accumulate_pipeline);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
            for quad in quads {
                let Some(group) = source_groups[quad.slot as usize].as_ref() else {
                    continue;
                };
                pass.set_bind_group(0, uniform_group, &[quad.slot * self.uniform_stride]);
                pass.set_bind_group(1, group, &[]);
                pass.draw_indexed(0..QUAD_INDICES.len() as u32, 0, 0..1);
            }
        }
        let layer = self.pool.acquire(device, self.target_key(size));
        self.resolver
            .get_or_init(|| super::accumulate::Resolver::new(&self.ctx, self.config.format))
            .resolve(&self.ctx, encoder, sum.view(), layer.view());
        (sum, layer)
    }

    /// Give the retouch effects in `chain` the faces of `segment`'s video at
    /// `source_time`, mapped through the quad's `crop` into its unit quad.
    /// A frame without landmarks leaves them faceless, drawn as they are.
    fn place_faces(
        &self,
        materials: &crate::modules::project::document::MaterialPool,
        segment: &Segment,
        source_time: Micros,
        crop: [f32; 4],
        chain: &mut [fx::FxInstance],
    ) {
        use crate::modules::fx::{catalog::RETOUCH, retouch};
        if !chain.iter().any(|f| f.desc.id == RETOUCH) {
            return;
        }
        let Some(video) = materials.video(&segment.material_id) else {
            return;
        };
        let fps = if video.fps.is_finite() && video.fps > 1.0 {
            video.fps
        } else {
            30.0
        };
        let period = (1_000_000.0 / fps) as Micros;
        let faces = self.faces.faces(&video.path, source_time, period);
        let quads: Vec<_> = faces
            .iter()
            .take(retouch::MAX_FACES)
            .map(|f| retouch::to_quad(&crate::modules::landmarks::shape::key_points(f), crop))
            .collect();
        for instance in chain.iter_mut().filter(|f| f.desc.id == RETOUCH) {
            for (slot, face) in instance.faces.iter_mut().zip(&quads) {
                *slot = Some(*face);
            }
        }
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
            // An effect clip draws nothing of its own: it applies its effects
            // to everything composited before it, at this point in the
            // painter's order. Decision 0016.
            if project.materials.is_effect_clip(segment) {
                if let Some(source_time) = project.materials.time_map(segment).source_time_at(time)
                {
                    let chain = fx::chain_for(&project.materials, segment, source_time, None);
                    if !chain.is_empty() {
                        draws.items.push(Draw::Adjust { chain });
                    }
                }
                continue;
            }

            // A transition claims the segment the compositor was about to draw
            // and replaces it with a blend of two. Exactly one of the two clips
            // contains any instant of the window — the cut is the boundary
            // between them — so no transition is ever drawn twice, and the
            // segment on the far side of the cut is not separately visible.
            if let Some(instant) =
                transitions::instant_for(track, &project.materials, segment, time)
            {
                let from_segment =
                    crate::modules::tracking::follow::resolve(project, instant.from.segment, time);
                let to_segment =
                    crate::modules::tracking::follow::resolve(project, instant.to.segment, time);
                // A stabilised clip is drawn through its moving crop window.
                let from_segment =
                    crate::modules::analysis::stabilise::resolve(project, from_segment, time);
                let to_segment =
                    crate::modules::analysis::stabilise::resolve(project, to_segment, time);
                let from = self.quad(
                    canvas,
                    &project.materials,
                    size,
                    sources,
                    &from_segment,
                    instant.from.kind,
                    instant.from.source_time,
                    time,
                    &mut draws,
                )?;
                let to = self.quad(
                    canvas,
                    &project.materials,
                    size,
                    sources,
                    &to_segment,
                    instant.to.kind,
                    instant.to.source_time,
                    time,
                    &mut draws,
                )?;
                if from.is_some() || to.is_some() {
                    // Each side carries its own clip's effects into its layer
                    // before the two are blended.
                    let side_fx =
                        |quad: &Option<QuadDraw>, layer: &transitions::TransitionLayer| {
                            quad.as_ref().map_or_else(Vec::new, |q| {
                                let mut chain = fx::chain_for(
                                    &project.materials,
                                    layer.segment,
                                    layer.source_time,
                                    Some(q.placement.mvp),
                                );
                                self.place_faces(
                                    &project.materials,
                                    layer.segment,
                                    layer.source_time,
                                    q.placement.crop,
                                    &mut chain,
                                );
                                chain
                            })
                        };
                    let from_fx = side_fx(&from, &instant.from);
                    let to_fx = side_fx(&to, &instant.to);
                    draws.items.push(Draw::Transition {
                        params: TransitionParams::from(&instant),
                        from,
                        to,
                        from_fx,
                        to_fx,
                    });
                }
                continue;
            }

            let kind = match project.materials.kind_of(&segment.material_id) {
                Some(kind) => kind,
                // The material was removed from the pool. `RemoveMaterial`
                // leaves the clip behind on purpose and `Project::validate`
                // warns about it; the honest picture is a placeholder where
                // the clip would be, not a silent hole. On an audio lane there
                // is nothing to draw, and everywhere else the segment is
                // treated as video so the provider can answer with its
                // missing-media field.
                None if track.kind == crate::modules::project::document::TrackKind::Audio => {
                    continue;
                }
                None => {
                    tracing::debug!(
                        segment = %segment.id,
                        material = %segment.material_id,
                        "segment references a material that is not in the pool; \
                         drawing the missing-media placeholder"
                    );
                    MaterialKind::Video
                }
            };

            let Some(source_time) = project.materials.time_map(segment).source_time_at(time) else {
                continue;
            };
            // An overlay that follows a motion track is placed by the track;
            // see `modules::tracking::follow`. Borrowed when it does not.
            let resolved = crate::modules::tracking::follow::resolve(project, segment, time);
            // A stabilised clip is drawn through its moving crop window; see
            // `modules::analysis::stabilise`. Borrowed when it is not.
            let resolved = crate::modules::analysis::stabilise::resolve(project, resolved, time);

            // Frame blending draws the source frame before this instant
            // and mixes in the one after; see `speed::blend`.
            let blend = (kind == MaterialKind::Video)
                .then(|| {
                    crate::modules::speed::blend::blend_for(
                        &project.materials,
                        segment,
                        source_time,
                    )
                })
                .flatten();
            // Optical flow draws the frame RIFE made for this instant, once
            // it is baked; until then the clip takes the frame blend above.
            // RIFE's frames are made from the decoded ones, so a clip whose
            // frames are remade (an object removed) blends its remade
            // frames instead.
            let flowed = blend
                .filter(|_| crate::modules::speed::flow::is_on(&project.materials, segment))
                .filter(|_| {
                    crate::modules::enhance::Chain::of(&project.materials, segment).is_none()
                })
                .and_then(|b| crate::modules::speed::flow::FlowSample::of(&b))
                .and_then(|sample| {
                    let video = project.materials.video(&segment.material_id)?;
                    self.flows.get(&self.ctx, &video.path, sample)
                });
            // Reduce noise in its Temporal mode mixes in the source frames
            // either side of this one (`fx::temporal`). The frame before is
            // asked for before this one and the frame after once it is in,
            // so the provider's two-frame cache answers the first two and
            // the decoder only ever moves forwards.
            let temporal = kind == MaterialKind::Video
                && blend.is_none()
                && fx::temporal::is_on(&project.materials, segment, source_time)
                && fx::motion_blur::motion_blur_of(&project.materials, segment, source_time)
                    .is_none();
            let around = project
                .materials
                .video(&segment.material_id)
                .filter(|_| temporal)
                .map(|video| {
                    fx::temporal::neighbours(source_time, fx::temporal::period(video.fps))
                });
            let before = around.and_then(|(before, _)| before).and_then(|at| {
                self.neighbour_frame(&project.materials, sources, segment, at, size)
            });
            let quad = self.quad(
                canvas,
                &project.materials,
                size,
                sources,
                &resolved,
                kind,
                blend.map_or(source_time, |b| b.first),
                time,
                &mut draws,
            )?;
            let after = around.filter(|_| quad.is_some()).and_then(|(_, after)| {
                self.neighbour_frame(&project.materials, sources, segment, after, size)
            });
            // The decoded frame placed the clip; the made frame, the same
            // picture's aspect, is what it shows.
            let (quad, blend) = match (quad, flowed) {
                (Some(mut quad), Some(frame)) => {
                    quad.frame = frame;
                    (Some(quad), None)
                }
                (quad, _) => (quad, blend),
            };
            if let Some(quad) = quad {
                let mut chain = fx::chain_for(
                    &project.materials,
                    segment,
                    source_time,
                    Some(quad.placement.mvp),
                );
                self.place_faces(
                    &project.materials,
                    segment,
                    source_time,
                    quad.placement.crop,
                    &mut chain,
                );
                // A blend mode needs the frame beneath, so the clip goes
                // through a layer and the blend pass. Normal, and a mode
                // this build does not know, take the ordinary draw.
                let mode = project
                    .materials
                    .compositing_of(segment)
                    .and_then(|m| m.blend.code());
                let accumulated = self.temporal_samples(
                    project,
                    segment,
                    time,
                    source_time,
                    blend,
                    &quad,
                    size,
                    sources,
                    &mut draws,
                );
                let mask = if chain.is_empty() {
                    None
                } else {
                    fx_mask(&project.materials, segment, &quad, &mut draws)
                };
                // The neighbours drawn like the clip, each with a slot of
                // its own; only when the chain still has the temporal pass.
                let neighbours: Neighbours =
                    if accumulated.is_none() && chain.iter().any(FxInstance::is_temporal_denoise) {
                        [before, after].map(|frame| {
                            frame.map(|frame| {
                                let mut draw = quad.clone();
                                draw.frame = frame;
                                draw.slot = draws.slots as u32;
                                draws.slots += 1;
                                draw
                            })
                        })
                    } else {
                        [None, None]
                    };
                draws.items.push(match (accumulated, mode) {
                    (Some(quads), mode) => Draw::Accumulated {
                        quads,
                        chain,
                        mode,
                        mask,
                    },
                    (None, Some(mode)) => Draw::Blended {
                        quad,
                        chain,
                        mode,
                        mask,
                        neighbours,
                    },
                    (None, None) => blurred(project, segment, time, quad, chain, mask, neighbours),
                });
            }
        }

        Ok(draws)
    }

    /// The draws that average into `quad` when the clip blends frames or has
    /// motion blur: every source frame (with its weight) at every placement
    /// along the shutter (each `1/n`), the weight folded into the opacity.
    /// `None` when one draw is the whole picture — no blending at this
    /// instant, a clip that does not move within the shutter, or a blur
    /// animation running, which owns the clip's layer and wins.
    ///
    /// The first draw keeps `quad`'s uniform slot; the others get new ones.
    #[allow(clippy::too_many_arguments)]
    fn temporal_samples(
        &self,
        project: &Project,
        segment: &Segment,
        time: Micros,
        source_time: Micros,
        blend: Option<crate::modules::speed::blend::BlendSample>,
        quad: &QuadDraw,
        size: (u32, u32),
        sources: &dyn SourceProvider,
        draws: &mut DrawList,
    ) -> Option<Vec<QuadDraw>> {
        let blur = fx::motion_blur::motion_blur_of(&project.materials, segment, source_time);
        if blend.is_none() && blur.is_none() {
            return None;
        }
        let keyed = layout::animated_transform(segment, time);
        if motion::clip_motion(&project.materials, segment, time, keyed)
            .is_some_and(|m| m.blur > 0.0 && m.blur_radius > 0.0)
        {
            return None;
        }

        let mut frames: Vec<(SourceFrame, f32)> = vec![(quad.frame.clone(), 1.0)];
        if let Some(blend) = blend {
            let request = SourceRequest {
                material_id: &segment.material_id,
                kind: MaterialKind::Video,
                source_time: blend.second,
                segment_id: &segment.id,
                max_size: size,
            };
            // A remade clip blends its remade frames.
            let second = match self.enhanced_frame(&project.materials, segment, blend.second, size)
            {
                Some((frame, _)) => Ok(Some(frame)),
                None => sources.frame(&self.ctx, &request),
            };
            match second {
                Ok(Some(second)) => {
                    frames[0].1 = 1.0 - blend.weight;
                    frames.push((second, blend.weight));
                }
                Ok(None) => {}
                // The earlier frame alone is still a correct picture, one
                // step less smooth; a missing neighbour is not worth a hole.
                Err(error) => tracing::debug!(
                    segment = %segment.id,
                    %error,
                    "frame blending: the next source frame is unavailable"
                ),
            }
        }

        let canvas = (project.canvas.width, project.canvas.height);
        let mut placements: Vec<Option<QuadPlacement>> = vec![Some(quad.placement)];
        if let Some(blur) = blur {
            let moved: Vec<Option<QuadPlacement>> =
                fx::motion_blur::sample_times(time, project.fps, blur, segment)
                    .into_iter()
                    .map(|at| {
                        let resolved =
                            crate::modules::tracking::follow::resolve(project, segment, at);
                        let resolved =
                            crate::modules::analysis::stabilise::resolve(project, resolved, at);
                        place(canvas, &project.materials, &resolved, quad.frame.size(), at)
                    })
                    .collect();
            let first = moved.iter().flatten().next().copied();
            let still = first.is_some_and(|first| {
                moved.iter().all(|p| {
                    p.is_some_and(|p| {
                        p.mvp
                            .iter()
                            .zip(first.mvp.iter())
                            .all(|(a, b)| (a - b).abs() < 1e-4)
                    })
                })
            });
            if !still {
                placements = moved;
            }
        }
        if frames.len() == 1 && placements.len() == 1 {
            return None;
        }

        // A sample where the clip covers nothing still counts: it is the
        // instant the shutter saw nothing there.
        let n = placements.len() as f32;
        let mut quads = Vec::with_capacity(frames.len() * placements.len());
        for (frame, weight) in &frames {
            for placement in placements.iter().flatten() {
                let mut draw = quad.clone();
                draw.frame = frame.clone();
                draw.placement = *placement;
                draw.placement.opacity = placement.opacity * weight / n;
                if !quads.is_empty() {
                    draw.slot = draws.slots as u32;
                    draws.slots += 1;
                }
                quads.push(draw);
            }
        }
        // Every placement missed the canvas: the quad's own slot is still
        // reserved, and drawing nothing is the honest picture.
        Some(quads)
    }

    /// The source frame of `segment`'s video at `source_time` for a temporal
    /// denoise: the remade one when the clip's frames are remade, as for the
    /// frame itself. `None` when there is none; a missing neighbour leaves
    /// the average to the frames that are there, it is not an error.
    fn neighbour_frame(
        &self,
        materials: &MaterialPool,
        sources: &dyn SourceProvider,
        segment: &Segment,
        source_time: Micros,
        size: (u32, u32),
    ) -> Option<SourceFrame> {
        if let Some((frame, _)) = self.enhanced_frame(materials, segment, source_time, size) {
            return Some(frame);
        }
        let request = SourceRequest {
            material_id: &segment.material_id,
            kind: MaterialKind::Video,
            source_time,
            segment_id: &segment.id,
            max_size: size,
        };
        match sources.frame(&self.ctx, &request) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::debug!(
                    segment = %segment.id,
                    %error,
                    "temporal denoise: a neighbouring frame is unavailable"
                );
                None
            }
        }
    }

    /// The remade frame of a video clip with "Remove object" or "Enhance
    /// quality" on at `source_time`, decoded to cover what a render of
    /// `size` shows of it, with the source's display size (which places
    /// it); `None` when the clip has neither or the frame is not made yet.
    fn enhanced_frame(
        &self,
        materials: &MaterialPool,
        segment: &Segment,
        source_time: Micros,
        size: (u32, u32),
    ) -> Option<(SourceFrame, (u32, u32))> {
        let chain = crate::modules::enhance::Chain::of(materials, segment)?;
        let video = materials.video(&segment.material_id)?;
        let period = if video.fps.is_finite() && video.fps > 1.0 {
            ((1_000_000.0 / video.fps).round() as Micros).max(1)
        } else {
            33_333
        };
        let display = (video.width.max(1), video.height.max(1));
        // What the clip covers in this render: fitted into it and zoomed by
        // its scale. A crop or a keyframed zoom shows more of fewer pixels;
        // the next JPEG size up covers most of that.
        let fit = (size.0 as f32 / display.0 as f32).min(size.1 as f32 / display.1 as f32);
        let zoom = segment.transform.scale[0]
            .abs()
            .max(segment.transform.scale[1].abs())
            .max(1.0);
        let want = (
            (display.0 as f32 * fit * zoom).ceil() as u32,
            (display.1 as f32 * fit * zoom).ceil() as u32,
        );
        let frame = self
            .enhanced
            .get(&self.ctx, &video.path, &chain, source_time, period, want)?;
        Some((frame, display))
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
        materials: &MaterialPool,
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
        // An animated sticker loops or holds its last frame; a still is
        // untouched. See `modules::animated`.
        let fetch_time = crate::modules::animated::clip_time(materials, segment, kind, source_time);

        let request = SourceRequest {
            material_id: &segment.material_id,
            kind,
            source_time: fetch_time,
            segment_id: &segment.id,
            max_size: size,
        };

        // A title whose text animator is running is drawn by the motion
        // module, glyph by glyph; at rest it is the provider's cached raster.
        let animated_text = (kind == MaterialKind::Text)
            .then(|| {
                motion::text::animated_text_frame(&self.ctx, materials, segment, time, size, canvas)
            })
            .flatten();
        // A clip whose frames are remade ("Remove object", "Enhance
        // quality") draws the made frame once it is baked, placed by the
        // source's own size (the made frame may be larger, never another
        // shape); until then the decoded one.
        let enhanced = (kind == MaterialKind::Video && animated_text.is_none())
            .then(|| self.enhanced_frame(materials, segment, fetch_time, size))
            .flatten();
        let place_size = enhanced.as_ref().map(|(_, display)| *display);
        let fetched = match (animated_text, enhanced) {
            (Some(frame), _) | (None, Some((frame, _))) => Ok(Some(frame)),
            // A compound clip: its sequence, rendered whole at this instant.
            (None, None) if kind == MaterialKind::Sequence => {
                self.nested_frame(canvas, materials, segment, source_time, size, sources)
            }
            (None, None) => sources.frame(&self.ctx, &request),
        };
        let frame = match fetched {
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

        let Some(placement) = place(
            canvas,
            materials,
            segment,
            place_size.unwrap_or(frame.size()),
            time,
        ) else {
            return Ok(None);
        };

        // The clip's colour adjustment, already reduced to what the shader
        // needs. Identity grades are dropped *here* rather than sent with the
        // flag up: the shader path for an ungraded clip must be exactly the
        // pre-colour one, or every existing project shifts by a code value.
        //
        // The two halves gate independently: `scalars_are_identity` rather
        // than `is_identity`, so a clip with only a LUT does not pay for the
        // grade arithmetic — and, the case the "missing LUT file" contract
        // needs, a clip whose LUT will not resolve and whose scalars are at
        // rest takes no colour path at all.
        let adjust = materials.color_adjust_of(segment);
        let color = adjust
            .filter(|adjust| !adjust.scalars_are_identity())
            .map(|adjust| {
                [
                    adjust.brightness,
                    adjust.contrast,
                    adjust.saturation,
                    adjust.temperature,
                ]
            });
        // The LUT resolves through the cache, which stats the file each time;
        // a missing or malformed file answers `None` here and the clip
        // renders without its look — warned about, never failed on.
        let lut = adjust
            .and_then(|adjust| adjust.lut.as_ref())
            .filter(|lut| lut.intensity > 0.0)
            .and_then(|lut| {
                self.luts
                    .get(&self.ctx, &lut.path)
                    .map(|gpu| (gpu, lut.intensity.min(1.0)))
            });

        // The extended grade. Packed once here; the stages at rest carry no
        // feature bit and the shader skips them.
        let mut grade = adjust
            .map(|adjust| super::grade::GradeBlock::new(&adjust.grade))
            .unwrap_or_default();
        if lut.as_ref().is_some_and(|(lut, _)| lut.one_d) {
            grade.features |= super::grade::feature::LUT_1D;
        }
        if kind == MaterialKind::Sequence {
            grade.features |= super::grade::feature::PREMULTIPLIED;
        }
        // Grain is reseeded per frame from the timeline time, never from a
        // clock or a counter: the preview and the export render the same
        // instant and must draw the same grain.
        grade.detail[3] = ((time / 1000).rem_euclid(1 << 16)) as f32;
        let curves = adjust
            .filter(|adjust| !adjust.grade.curves.is_identity())
            .map(|adjust| self.curves.get(&self.ctx, &adjust.grade.curves));
        if curves.is_some() {
            grade.features |= super::grade::feature::CURVES;
        }

        // Masks and the chroma key, measured in the quad as it is drawn in
        // this render; at rest (or absent) they set no flag.
        let compositing = materials.compositing_of(segment);
        let mut matte = compositing
            .map(|m| {
                let quad = super::matte::quad_pixel_size(&placement.mvp, size);
                super::matte::MatteBlock::new(m, source_time, quad)
            })
            .unwrap_or_default();
        // The clip's matte ("Remove background", or a grade or effects
        // limited to the subject): the baked matte of this source frame,
        // when there is one. A frame not baked yet draws whole and graded
        // whole.
        let setting = compositing
            .and_then(|m| m.background.as_ref())
            .filter(|s| s.is_used());
        let background = setting.and_then(|setting| {
            let video = materials.video(&segment.material_id)?;
            let period = if video.fps.is_finite() && video.fps > 1.0 {
                (1_000_000.0 / video.fps).round() as Micros
            } else {
                33_333
            };
            self.backgrounds
                .get(&self.ctx, &video.path, setting, source_time, period)
        });
        if let (Some(setting), Some(_)) = (setting, &background) {
            use crate::modules::project::compositing::MatteTarget;
            if setting.cut {
                matte.flags[0] |= super::matte::flag::BACKGROUND;
                if setting.invert {
                    matte.flags[0] |= super::matte::flag::BACKGROUND_INVERT;
                }
            }
            match setting.grade {
                MatteTarget::Subject => matte.flags[0] |= super::matte::flag::GRADE_SUBJECT,
                MatteTarget::Background => matte.flags[0] |= super::matte::flag::GRADE_BACKGROUND,
                _ => {}
            }
        }

        let slot = draws.slots as u32;
        draws.slots += 1;
        Ok(Some(QuadDraw {
            frame,
            placement,
            color,
            lut,
            grade,
            curves,
            matte,
            background,
            slot,
        }))
    }

    /// A compound clip's picture: its sequence composited at `source_time` into
    /// a texture of this render's size, handed back as an ordinary source
    /// frame so the clip's transform, grade, masks and effects apply to the
    /// whole composite, as they would to a video.
    ///
    /// The nested render clears to transparent, so a compound clip shows what
    /// is beneath it wherever its own lanes are empty — and holds
    /// *premultiplied* colour, which is why the quad that draws it carries the
    /// `PREMULTIPLIED` feature bit. The texture is not returned to the pool:
    /// the source frame still refers to it, and a pooled texture handed out
    /// again within the same frame would be drawn over.
    ///
    /// The nested view and the finished frame are kept ([`super::nested`]),
    /// keyed by a fingerprint of the compound clip's contents, so the pool is
    /// cloned once per document rather than once per frame, and the same
    /// inside at the same instant is rendered once.
    ///
    /// Recursion is bounded by `sequence::MAX_DEPTH`, counted per thread
    /// because one render runs on one thread from start to end.
    #[allow(clippy::too_many_arguments)]
    fn nested_frame(
        &self,
        canvas: (u32, u32),
        materials: &MaterialPool,
        segment: &Segment,
        source_time: Micros,
        size: (u32, u32),
        sources: &dyn SourceProvider,
    ) -> anyhow::Result<Option<SourceFrame>> {
        thread_local! {
            static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        }
        let depth = DEPTH.get();
        if depth >= crate::modules::sequence::MAX_DEPTH {
            tracing::warn!(segment = %segment.id, "compound clips nest too deep; drawing nothing");
            return Ok(None);
        }
        let id = &segment.material_id;
        let Some(key) = crate::modules::sequence::digest::digest(materials, canvas, id) else {
            return Ok(None);
        };
        let provider = sources.cache_identity();
        if let Some(provider) = provider {
            if let Some(texture) = self.nested.lock().frame(key, provider, source_time, size) {
                return Ok(Some(SourceFrame::from_texture(texture)));
            }
        }
        let cached = self.nested.lock().view(key);
        let view = match cached {
            Some(view) => view,
            None => {
                let Some(view) = crate::modules::sequence::nested_in(materials, canvas, id) else {
                    return Ok(None);
                };
                let view = Arc::new(view);
                self.nested.lock().put_view(key, Arc::clone(&view));
                view
            }
        };
        if source_time < 0 || source_time >= view.duration() {
            return Ok(None);
        }
        DEPTH.set(depth + 1);
        let rendered = self.render_to_texture(&view, source_time, size, sources);
        DEPTH.set(depth);
        let target = rendered.map_err(|e| anyhow::anyhow!("compound clip: {e}"))?;
        let texture = Arc::new(target.texture().clone());
        if let Some(provider) = provider {
            let texel = self.config.format.block_copy_size(None).unwrap_or(4) as u64;
            let bytes = size.0 as u64 * size.1 as u64 * texel;
            self.nested.lock().put_frame(
                key,
                provider,
                source_time,
                size,
                Arc::clone(&texture),
                bytes,
            );
        }
        Ok(Some(SourceFrame::from_texture(texture)))
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
        let bytes_per_pixel = target.format().block_copy_size(None).ok_or_else(|| {
            RenderError::Readback(format!("{:?} is not copyable", target.format()))
        })?;
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

/// Where `segment` (already resolved for follows and stabilisation) is drawn
/// at `time`, for a source picture of `frame_size`. `None` when it covers
/// nothing.
fn place(
    canvas: (u32, u32),
    materials: &MaterialPool,
    segment: &Segment,
    frame_size: (u32, u32),
    time: Micros,
) -> Option<QuadPlacement> {
    let keyed = layout::animated_transform(segment, time);
    // Keyframe-free animation, on top of the keyframes. `None` for a clip
    // without one, which then takes exactly the path it always took.
    let motion = motion::clip_motion(materials, segment, time, keyed);
    let transform = motion.map_or(keyed, |m| m.transform);
    let crop = layout::animated_crop(segment, time);
    let placement = layout::place_quad(canvas, frame_size, &transform, crop)?;
    match motion {
        Some(m) if m.reveal != [0.0, 0.0, 1.0, 1.0] => layout::reveal(placement, m.reveal),
        _ => Some(placement),
    }
}

/// `quad` as it should be drawn: as itself, or — while a blur animation runs —
/// as the incoming side of a blur transition from nothing.
///
/// Through the transition pipeline rather than a blur in the quad shader, so
/// the blur is frame-space (a small clip blurs as much as a full-frame one)
/// and the quad pipeline, which the colour grade owns, is untouched.
#[allow(clippy::too_many_arguments)]
fn blurred(
    project: &Project,
    segment: &Segment,
    time: Micros,
    quad: QuadDraw,
    chain: Vec<FxInstance>,
    mask: Option<FxMask>,
    neighbours: Neighbours,
) -> Draw {
    // Without a blur animation the clip is an ordinary quad, or an effected
    // one when it carries built-in effects (`modules/fx`). A running blur
    // animation draws the clip as a transition side, where a temporal
    // denoise falls back to its spatial pass.
    let plain = |quad: QuadDraw, chain: Vec<FxInstance>, mask: Option<FxMask>| {
        if chain.is_empty() {
            Draw::Quad(quad)
        } else {
            Draw::Effected {
                quad,
                chain,
                mask,
                neighbours,
            }
        }
    };
    let keyed = layout::animated_transform(segment, time);
    let Some(m) = motion::clip_motion(&project.materials, segment, time, keyed) else {
        return plain(quad, chain, mask);
    };
    if m.blur <= 0.0 || m.blur_radius <= 0.0 {
        return plain(quad, chain, mask);
    }
    // The blur animation draws the clip as a transition side, whose effects
    // apply to the whole layer; a matte-limited effect is whole there.
    Draw::Transition {
        params: TransitionParams {
            kind: TransitionKind::Blur,
            progress: (1.0 - m.blur).clamp(0.0, 1.0),
            direction: TransitionDirection::default(),
            softness: m.blur_radius,
            zoom: 0.0,
            color: [0.0; 4],
            library: None,
        },
        from: None,
        to: Some(quad),
        // The clip's effects run on its layer before the blur blends it in.
        from_fx: Vec::new(),
        to_fx: chain,
    }
}

/// The matte draw that limits `segment`'s effects to its subject or the
/// rest, when its matte says so and the frame drawn has one baked. Without a
/// baked matte the effects apply to the whole clip, as a frame with no
/// matte yet is drawn whole.
fn fx_mask(
    materials: &MaterialPool,
    segment: &Segment,
    quad: &QuadDraw,
    draws: &mut DrawList,
) -> Option<FxMask> {
    use crate::modules::project::compositing::MatteTarget;
    let setting = materials.compositing_of(segment)?.background.as_ref()?;
    let background = match setting.effects {
        MatteTarget::Subject => false,
        MatteTarget::Background => true,
        _ => return None,
    };
    quad.background.as_ref()?;
    let mut mask = quad.clone();
    mask.matte.flags = [super::matte::flag::MATTE_OUT, 0, 0, 0];
    mask.slot = draws.slots as u32;
    draws.slots += 1;
    Some(FxMask {
        quad: mask,
        background,
    })
}

/// One textured quad: a segment's frame, where it goes, and which uniform
/// block holds that.
///
/// The slot is explicit rather than positional because a transition contributes
/// two quads that are *not* drawn in the composite pass — they are drawn into
/// layers beforehand — so "the nth draw" and "the nth uniform block" stopped
/// being the same number.
#[derive(Clone)]
struct QuadDraw {
    frame: SourceFrame,
    placement: QuadPlacement,
    /// `(brightness, contrast, saturation, temperature)` when the segment has
    /// a non-identity colour adjustment; `None` renders exactly as before the
    /// colour feature existed.
    color: Option<[f32; 4]>,
    /// The clip's resolved LUT and its intensity. `None` when there is no
    /// LUT, its intensity is 0, or its file is missing or malformed — all of
    /// which render the clip without a look rather than failing.
    lut: Option<(std::sync::Arc<super::lut::GpuLut>, f32)>,
    /// The extended grade, packed; `features` 0 when nothing in it is live.
    grade: super::grade::GradeBlock,
    /// The baked tone curves, when the clip has any.
    curves: Option<std::sync::Arc<super::grade::GpuCurves>>,
    /// Masks and chroma key, packed; no flag when the clip has neither.
    matte: super::matte::MatteBlock,
    /// The clip's baked "Remove background" matte for this frame.
    background: Option<std::sync::Arc<super::background::GpuMatte>>,
    slot: u32,
}

/// One thing the composite pass draws.
///
/// The variants differ in size — an effect clip carries only its chain — and
/// that is fine: a frame has a handful of these, built and dropped per frame.
#[allow(clippy::large_enum_variant)]
enum Draw {
    Quad(QuadDraw),
    /// Two clips blended against the frame. Either side may be `None`, which is
    /// a transparent layer; both being `None` is not built at all.
    Transition {
        params: TransitionParams,
        from: Option<QuadDraw>,
        to: Option<QuadDraw>,
        /// Each side's own effects, run over its layer before the blend.
        from_fx: Vec<FxInstance>,
        to_fx: Vec<FxInstance>,
    },
    /// A clip with effects: drawn into a layer of its own, the effects run
    /// over the layer, and the result composited where the clip would have
    /// been.
    Effected {
        quad: QuadDraw,
        chain: Vec<FxInstance>,
        /// The effects apply only to the matte's subject or the rest.
        mask: Option<FxMask>,
        /// The clip drawn from the source frames before and after this one,
        /// for a temporal denoise in `chain` (`fx::temporal`).
        neighbours: Neighbours,
    },
    /// An effect clip: its effects run over everything composited so far.
    Adjust {
        chain: Vec<FxInstance>,
    },
    /// A clip with a blend mode other than normal: drawn into a layer (its
    /// effects run over it), then laid onto everything composited so far by
    /// the blend pass. `mode` is `BlendMode::code`.
    Blended {
        quad: QuadDraw,
        chain: Vec<FxInstance>,
        mode: u32,
        mask: Option<FxMask>,
        neighbours: Neighbours,
    },
    /// One clip drawn several times and averaged: two source frames mixed
    /// (frame blending) and/or the clip placed along its movement (motion
    /// blur). Each quad's opacity already carries its weight. The average
    /// becomes an ordinary clip layer, which then takes the `Effected` path,
    /// or the `Blended` one when `mode` is set. See `render::accumulate`.
    Accumulated {
        quads: Vec<QuadDraw>,
        chain: Vec<FxInstance>,
        mode: Option<u32>,
        mask: Option<FxMask>,
    },
}

/// The clip as the source frames before and after the one drawn show it,
/// placed like it; empty unless a temporal denoise asks (`fx::temporal`).
type Neighbours = [Option<QuadDraw>; 2];

/// A clip's effects limited by its matte (`BackgroundRemoval::effects`): the
/// matte drawn as weights over the clip's quad (`M_MATTE_OUT`), and which
/// side of it the effects apply to. See [`super::matte_mix`].
#[derive(Clone)]
struct FxMask {
    quad: QuadDraw,
    /// The effects apply to the rest, not the subject.
    background: bool,
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
        self.items.iter().flat_map(|item| -> Vec<&QuadDraw> {
            match item {
                Draw::Quad(quad) => vec![quad],
                Draw::Effected {
                    quad,
                    mask,
                    neighbours,
                    ..
                }
                | Draw::Blended {
                    quad,
                    mask,
                    neighbours,
                    ..
                } => std::iter::once(quad)
                    .chain(mask.as_ref().map(|m| &m.quad))
                    .chain(neighbours.iter().flatten())
                    .collect(),
                Draw::Transition { from, to, .. } => {
                    from.as_ref().into_iter().chain(to.as_ref()).collect()
                }
                Draw::Accumulated { quads, mask, .. } => {
                    quads.iter().chain(mask.as_ref().map(|m| &m.quad)).collect()
                }
                Draw::Adjust { .. } => Vec::new(),
            }
        })
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
        // 0.5 * 255 = 127.5 sits exactly on a rounding boundary: Intel's
        // Vulkan driver writes 128, NVIDIA's 127. Either is the background.
        let got = f.pixel(320, 240);
        let want = [0u8, 64, 128, 255];
        assert!(
            got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1),
            "background {got:?}, expected {want:?} within one step"
        );
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
        assert!(
            (r as i32 - 128).abs() <= 2,
            "half of white over black, got {r}"
        );
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

        assert_eq!(
            frame(&c, &project, 0, &provider).pixel(320, 240),
            [0, 0, 0, 255]
        );
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
        let f = c.render(&project, 0, (101, 37), &provider).expect("render");
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
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
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
        /// Left half one colour, right half another. The asymmetric source
        /// the crop and rotation tests need: a solid colour cannot show
        /// *which* part of the picture ended up *where*.
        SplitRgba([u8; 4], [u8; 4], u32, u32),
        /// The same split as full-range achromatic NV12: left luma, right
        /// luma, chroma at 128 throughout.
        SplitPlanar(u8, u8, u32, u32),
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

        /// [`Self::plane`] with the left half of every row one texel and the
        /// right half another.
        fn split_plane(
            ctx: &RenderContext,
            w: u32,
            h: u32,
            format: wgpu::TextureFormat,
            left: &[u8],
            right: &[u8],
        ) -> Arc<wgpu::Texture> {
            let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("mixed provider split plane"),
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
            let mut data = Vec::with_capacity((w * h) as usize * left.len());
            for _ in 0..h {
                for x in 0..w {
                    data.extend_from_slice(if x < w / 2 { left } else { right });
                }
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
                    bytes_per_row: Some(w * left.len() as u32),
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
                TestSource::SplitRgba(left, right, w, h) => SourceFrame::from_texture(
                    Self::split_plane(ctx, w, h, wgpu::TextureFormat::Rgba8Unorm, &left, &right),
                ),
                TestSource::SplitPlanar(left, right, w, h) => SourceFrame::from_planes(
                    Self::split_plane(ctx, w, h, wgpu::TextureFormat::R8Unorm, &[left], &[right]),
                    Self::plane(
                        ctx,
                        w / 2,
                        h / 2,
                        wgpu::TextureFormat::Rg8Unorm,
                        &[128, 128],
                    ),
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
    fn cut_project(kind: TransitionKind, duration: Micros, size: (u32, u32)) -> Project {
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
        let provider = MixedProvider::default()
            .with("left", RED)
            .with("right", BLUE);

        // Before and after the window there is no transition at all, just the
        // clip that owns the instant.
        assert_eq!(
            frame(&c, &project, 3_000_000, &provider).pixel(320, 240),
            [255, 0, 0, 255]
        );
        assert_eq!(
            frame(&c, &project, 5_000_000, &provider).pixel(320, 240),
            [0, 0, 255, 255]
        );

        // At the very start of the window the outgoing clip is still whole.
        assert_eq!(
            frame(&c, &project, 3_500_000, &provider).pixel(320, 240),
            [255, 0, 0, 255]
        );

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
            let provider = MixedProvider::default()
                .with("left", left)
                .with("right", right);

            let start = frame(&c, &project, 3_500_000, &provider).pixel(320, 240);
            assert!(
                start[0] > 250,
                "{name}: the window opens on white, got {start:?}"
            );
            let end = frame(&c, &project, 4_499_999, &provider).pixel(320, 240);
            assert!(
                end[0] < 5,
                "{name}: the window closes on black, got {end:?}"
            );

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
        let provider = MixedProvider::default()
            .with("left", RED)
            .with("right", BLUE);

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
        let provider = MixedProvider::default()
            .with("left", RED)
            .with("right", BLUE);

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
        let provider = MixedProvider::default()
            .with("left", RED)
            .with("right", BLUE);

        let f = frame(&c, &project, 4_000_000, &provider);
        assert_eq!(
            f.pixel(4, 240),
            [0, 0, 255, 255],
            "the left half has arrived"
        );
        assert_eq!(
            f.pixel(635, 240),
            [255, 0, 0, 255],
            "the right half has not"
        );
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
        let provider = MixedProvider::default()
            .with("left", RED)
            .with("right", BLUE);

        frame(&c, &project, 4_000_000, &provider);
        assert_eq!(provider.calls(), 2, "inside the window, both sides");

        let outside = MixedProvider::default()
            .with("left", RED)
            .with("right", BLUE);
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
        let provider = MixedProvider::default()
            .with("left", RED)
            .with("right", BLUE);

        // The gap shows the background, and the clip on each side is whole.
        assert_eq!(
            frame(&c, &project, 2_500_000, &provider).pixel(320, 240),
            [255, 0, 0, 255]
        );
        assert_eq!(
            frame(&c, &project, 3_500_000, &provider).pixel(320, 240),
            [0, 0, 0, 255]
        );
        assert_eq!(
            frame(&c, &project, 4_100_000, &provider).pixel(320, 240),
            [0, 0, 255, 255]
        );
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
            assert!(
                r > 10 && b > 10,
                "expected both clips at {at} µs, got {:?}",
                preview.pixel(320, 240)
            );
        }
    }

    // -----------------------------------------------------------------------
    // Crop and rotation
    //
    // Both are exercised through a split source — left half one tone, right
    // half another — because a solid colour can prove *that* something drew
    // and never *which part of the picture went where*, and "which part went
    // where" is the entire content of crop and rotation. And both are run on
    // both decode paths: geometry that only holds for software-decoded RGBA
    // is the trap this codebase has already been caught by (see the greyscale
    // incident in CLAUDE.md), and hardware decode is the default.
    // -----------------------------------------------------------------------

    /// Where a probed pixel landed, for the split-source geometry tests.
    #[derive(Debug, PartialEq, Clone, Copy)]
    enum Region {
        /// The tone the *left* half of the source carries.
        Left,
        /// The tone the *right* half of the source carries.
        Right,
        Background,
    }

    /// One clip covering `[0, 4 s)` of a 640x480 canvas, background blue so
    /// that the planar sources' black is distinguishable from it.
    fn split_project(material: &str) -> Project {
        let mut project = project([0.0, 0.0, 1.0, 1.0]);
        add_video(&mut project, material);
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(segment(material, 0, 4_000_000));
        project.tracks.push(track);
        project
    }

    /// A name, a source, and how to read a rendered pixel back into a region.
    type SplitCase = (&'static str, TestSource, fn([u8; 4]) -> Region);

    /// The two split sources and how to read a rendered pixel back into a
    /// [`Region`]. Red|blue for the RGBA path; white|black NV12 for the
    /// hardware path.
    fn split_cases() -> Vec<SplitCase> {
        fn classify_rgba(p: [u8; 4]) -> Region {
            match p {
                [r, _, b, _] if r > 200 && b < 50 => Region::Left,
                [r, g, b, _] if b > 200 && r < 50 && g > 200 => Region::Right,
                [r, g, b, _] if b > 200 && r < 50 && g < 50 => Region::Background,
                other => panic!("unclassifiable pixel {other:?}"),
            }
        }
        fn classify_planar(p: [u8; 4]) -> Region {
            match p {
                [r, g, b, _] if r > 200 && g > 200 && b > 200 => Region::Left,
                [r, g, b, _] if r < 50 && g < 50 && b < 50 => Region::Right,
                [r, _, b, _] if b > 200 && r < 50 => Region::Background,
                other => panic!("unclassifiable pixel {other:?}"),
            }
        }
        vec![
            (
                "software (RGBA)",
                TestSource::SplitRgba([255, 0, 0, 255], [0, 255, 255, 255], 640, 480),
                classify_rgba,
            ),
            (
                "hardware (NV12)",
                TestSource::SplitPlanar(255, 0, 640, 480),
                classify_planar,
            ),
        ]
    }

    /// Cropping to the left half must scale that half to fill the fitted
    /// quad — the right half must not appear anywhere in the frame — and the
    /// fit must follow the *cropped* aspect (320x480 pillarboxed on 4:3), not
    /// the source's.
    #[test]
    fn a_crop_keeps_only_the_kept_region_on_both_decode_paths() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        for (name, source, classify) in split_cases() {
            let mut project = split_project("split");
            project.tracks[0].segments[0].crop = Some(crate::modules::project::document::Crop {
                left: 0.0,
                top: 0.0,
                right: 0.5,
                bottom: 1.0,
            });
            let provider = MixedProvider::default().with("split", source);
            let f = frame(&c, &project, 0, &provider);

            // Cropped source is 320x480; fitted at 1:1 it spans x = 160..480.
            assert_eq!(classify(f.pixel(320, 240)), Region::Left, "{name}: centre");
            assert_eq!(
                classify(f.pixel(200, 240)),
                Region::Left,
                "{name}: left of quad"
            );
            assert_eq!(
                classify(f.pixel(460, 240)),
                Region::Left,
                "{name}: the kept half stretches across the whole quad"
            );
            assert_eq!(
                classify(f.pixel(100, 240)),
                Region::Background,
                "{name}: pillarbox left"
            );
            assert_eq!(
                classify(f.pixel(550, 240)),
                Region::Background,
                "{name}: pillarbox right"
            );
            // Nothing from the discarded half anywhere on the centre row.
            for x in (2..640).step_by(10) {
                assert_ne!(
                    classify(f.pixel(x, 240)),
                    Region::Right,
                    "{name}: discarded half leaked at x={x}"
                );
            }
        }
    }

    /// The quarter turns, from `Transform::rotation` rather than from
    /// container metadata. Clockwise 90 puts the source's left half at the
    /// top; 180 puts it on the right; 270 at the bottom. Pinned against the
    /// same expectations on both decode paths.
    #[test]
    fn quarter_turn_rotations_put_each_half_where_a_clockwise_turn_says() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        for (name, source, classify) in split_cases() {
            for (angle, probes) in [
                // (x, y, expected): the 90° quad is 480 wide, x = 80..560.
                (
                    90.0f32,
                    vec![
                        (320, 120, Region::Left),
                        (320, 360, Region::Right),
                        (30, 240, Region::Background),
                        (610, 240, Region::Background),
                    ],
                ),
                (
                    180.0,
                    vec![(500, 240, Region::Left), (140, 240, Region::Right)],
                ),
                (
                    270.0,
                    vec![
                        (320, 360, Region::Left),
                        (320, 120, Region::Right),
                        (30, 240, Region::Background),
                    ],
                ),
            ] {
                let mut project = split_project("split");
                project.tracks[0].segments[0].transform.rotation = angle;
                let provider = MixedProvider::default().with("split", source);
                let f = frame(&c, &project, 0, &provider);
                for (x, y, expected) in probes {
                    assert_eq!(
                        classify(f.pixel(x, y)),
                        expected,
                        "{name}: at {angle}°, pixel ({x},{y})"
                    );
                }
            }
        }
    }

    /// An arbitrary angle. A square clip at 45° is a diamond `|x|+|y| <= d`
    /// around the centre with `d = side/√2`, so points just inside and just
    /// outside that boundary pin the actual rotation angle rather than only
    /// "it turned by some multiple of 90".
    #[test]
    fn an_arbitrary_rotation_draws_the_expected_diamond_on_both_decode_paths() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        for (name, source, is_clip) in [
            (
                "software (RGBA)",
                TestSource::Rgba([255, 0, 0, 255], 480, 480),
                (|p: [u8; 4]| p[0] > 200 && p[2] < 50) as fn([u8; 4]) -> bool,
            ),
            (
                "hardware (NV12)",
                TestSource::Planar(255, 128, 128, 480, 480),
                |p: [u8; 4]| p[0] > 200 && p[1] > 200,
            ),
        ] {
            let mut project = split_project("square");
            {
                let transform = &mut project.tracks[0].segments[0].transform;
                // 480-square fitted to the 480-tall canvas, halved: a 240px
                // square, whose 45° diamond reaches ±169.7px on the axes.
                transform.scale = [0.5, 0.5];
                transform.rotation = 45.0;
            }
            let provider = MixedProvider::default().with("square", source);
            let f = frame(&c, &project, 0, &provider);

            assert!(is_clip(f.pixel(320, 240)), "{name}: centre is the clip");
            assert!(
                is_clip(f.pixel(440, 240)),
                "{name}: (120,0) is inside the diamond"
            );
            assert!(
                is_clip(f.pixel(320, 90)),
                "{name}: (0,-150) is inside the diamond"
            );
            assert!(
                !is_clip(f.pixel(440, 360)),
                "{name}: (120,120) is outside the diamond — an unrotated \
                 square would still cover it"
            );
            assert!(
                !is_clip(f.pixel(440, 120)),
                "{name}: (120,-120) is outside the diamond"
            );
        }
    }

    /// Crop first, then rotate: the kept half fills the quad and the quad
    /// turns. The order is what `layout::place_quad` documents; if crop were
    /// applied after rotation the discarded half would reappear.
    #[test]
    fn crop_and_rotation_compose_in_document_order() {
        let Some(c) = compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        for (name, source, classify) in split_cases() {
            let mut project = split_project("split");
            {
                let segment = &mut project.tracks[0].segments[0];
                segment.crop = Some(crate::modules::project::document::Crop {
                    left: 0.0,
                    top: 0.0,
                    right: 0.5,
                    bottom: 1.0,
                });
                segment.transform.rotation = 90.0;
            }
            let provider = MixedProvider::default().with("split", source);
            let f = frame(&c, &project, 0, &provider);

            // Cropped 320x480, rotated: a 480x320 quad, y = 80..400.
            assert_eq!(classify(f.pixel(320, 240)), Region::Left, "{name}: centre");
            assert_eq!(
                classify(f.pixel(320, 60)),
                Region::Background,
                "{name}: above the rotated quad"
            );
            for (x, y) in [(320, 100), (320, 380), (120, 240), (520, 240)] {
                assert_ne!(
                    classify(f.pixel(x, y)),
                    Region::Right,
                    "{name}: discarded half leaked at ({x},{y})"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Colour adjustments
    // -----------------------------------------------------------------------

    /// Attach a grade to the only clip of `project`.
    fn grade(project: &mut Project, adjust: [f32; 4]) {
        let material = crate::modules::project::document::ColorAdjustMaterial {
            id: "grade".into(),
            brightness: adjust[0],
            contrast: adjust[1],
            saturation: adjust[2],
            temperature: adjust[3],
            lut: None,
            grade: Default::default(),
        };
        project.materials.color_adjusts.push(material);
        project.tracks[0].segments[0].extras.push("grade".into());
    }

    /// [`grade`], with a LUT file attached as well.
    fn grade_with_lut(
        project: &mut Project,
        adjust: [f32; 4],
        path: &std::path::Path,
        intensity: f32,
    ) {
        grade(project, adjust);
        project.materials.color_adjusts[0].lut = Some(crate::modules::project::document::LutRef {
            path: path.to_string_lossy().into_owned(),
            intensity,
        });
    }

    /// A .cube file on real disk, because the cache is keyed by path + mtime
    /// and a LUT that only existed in memory would test nothing about that.
    fn write_lut(name: &str, text: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("chukcut-lut-tests");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{name}-{}.cube", std::process::id()));
        std::fs::write(&path, text).expect("write LUT fixture");
        path
    }

    /// The reference for `apply_color` in `quad.wgsl`, in encoded space.
    ///
    /// Written out independently rather than shared, for the reason
    /// `nv12_pixel` gives: a change to the shader must fail here, not cancel
    /// out. Takes and returns encoded values because, against an sRGB render
    /// target, the readback byte *is* the encoded value — the target's encode
    /// undoes the shader's final decode.
    fn adjust_encoded(mut c: [f32; 3], adjust: [f32; 4]) -> [f32; 3] {
        let [brightness, contrast, saturation, temperature] = adjust;
        c[0] += temperature * 0.2;
        c[2] -= temperature * 0.2;
        let grey = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        for channel in &mut c {
            *channel = grey + (*channel - grey) * saturation;
            *channel = (*channel - 0.5) * contrast + 0.5;
            *channel += brightness;
            *channel = channel.clamp(0.0, 1.0);
        }
        c
    }

    /// The headline requirement: a document that never touched colour renders
    /// **byte-identical** to one from before the feature existed. An identity
    /// grade takes the ungraded shader path outright (the `color_active`
    /// flag), so this asserts full-frame equality, not closeness — a colour
    /// pass that shifts untouched clips by one code value fails here.
    #[test]
    fn an_identity_grade_renders_byte_identical_to_no_grade_at_all() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // A hardware-decoded colourful source, where any accidental colour
        // maths would have the most to distort.
        let source = TestSource::Planar(173, 90, 200, 640, 480);
        let provider = MixedProvider::default().with("clip", source);

        let mut project = split_project("clip");
        let before = frame(&c, &project, 0, &provider);

        grade(&mut project, [0.0, 1.0, 1.0, 0.0]);
        let with_identity = frame(&c, &project, 0, &provider);
        assert_eq!(
            before.data, with_identity.data,
            "an identity grade changed pixels"
        );

        // And the machinery is actually live: a non-identity grade differs.
        project.materials.color_adjusts[0].brightness = 0.1;
        let brightened = frame(&c, &project, 0, &provider);
        assert_ne!(before.data, brightened.data, "the grade did nothing");
    }

    /// The same grade must produce the same pixels whichever decoder produced
    /// the frame. White and black land on identical values through both
    /// paths, so one expected number covers both — the same trick the
    /// transition agreement test uses.
    #[test]
    fn a_grade_is_the_same_however_the_clip_was_decoded() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let white_sw = TestSource::Rgba([255, 255, 255, 255], 640, 480);
        let white_hw = TestSource::Planar(255, 128, 128, 640, 480);
        let black_sw = TestSource::Rgba([0, 0, 0, 255], 640, 480);
        let black_hw = TestSource::Planar(0, 128, 128, 640, 480);

        // (source, grade, expected RGB) — expectations computed through the
        // reference, spelled as numbers so a broken reference is also caught.
        // White at brightness -0.4 sits at 0.6 encoded = 153; warm white
        // drops only blue, 1 - 0.6*0.2 = 0.88 = 224; black at +0.4 is 102.
        for (name, source, adjust, expected) in [
            (
                "sw darkened",
                white_sw,
                [-0.4, 1.0, 1.0, 0.0],
                [153, 153, 153],
            ),
            (
                "hw darkened",
                white_hw,
                [-0.4, 1.0, 1.0, 0.0],
                [153, 153, 153],
            ),
            ("sw warmed", white_sw, [0.0, 1.0, 1.0, 0.6], [255, 255, 224]),
            ("hw warmed", white_hw, [0.0, 1.0, 1.0, 0.6], [255, 255, 224]),
            (
                "sw lifted black",
                black_sw,
                [0.4, 1.0, 1.0, 0.0],
                [102, 102, 102],
            ),
            (
                "hw lifted black",
                black_hw,
                [0.4, 1.0, 1.0, 0.0],
                [102, 102, 102],
            ),
        ] {
            let mut project = split_project("clip");
            grade(&mut project, adjust);
            let provider = MixedProvider::default().with("clip", source);
            let f = frame(&c, &project, 0, &provider);
            let [r, g, b, a] = f.pixel(320, 240);
            for (channel, actual, wanted) in [
                ("r", r, expected[0]),
                ("g", g, expected[1]),
                ("b", b, expected[2]),
            ] {
                assert!(
                    (actual as i32 - wanted).abs() <= 2,
                    "{name}: {channel} is {actual}, expected {wanted}"
                );
            }
            assert_eq!(a, 255, "{name}: a grade never touches coverage");

            // The reference agrees with the hand-computed numbers above.
            let encoded_in = if matches!(
                source,
                TestSource::Rgba([0, ..], ..) | TestSource::Planar(0, ..)
            ) {
                [0.0, 0.0, 0.0]
            } else {
                [1.0, 1.0, 1.0]
            };
            let reference = adjust_encoded(encoded_in, adjust);
            for i in 0..3 {
                assert!(
                    ((reference[i] * 255.0).round() as i32 - expected[i]).abs() <= 1,
                    "{name}: the reference and the spelled-out expectation disagree"
                );
            }
        }
    }

    /// Mid-tones through the full reference, on the hardware path, where the
    /// arithmetic is exact: a full-range achromatic NV12 sample decodes to
    /// its own code value, so `readback = adjust_encoded(y/255) * 255`.
    #[test]
    fn contrast_brightness_and_saturation_match_the_reference_arithmetic() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // Contrast pushes a light grey away from the pivot; brightness slides
        // it down; both at once compose in the documented order.
        for (name, adjust) in [
            ("contrast", [0.0, 1.5, 1.0, 0.0f32]),
            ("brightness", [-0.2, 1.0, 1.0, 0.0]),
            ("both", [-0.2, 1.5, 1.0, 0.0]),
            ("cooled", [0.0, 1.0, 1.0, -0.7]),
        ] {
            let mut project = split_project("clip");
            grade(&mut project, adjust);
            let provider =
                MixedProvider::default().with("clip", TestSource::Planar(200, 128, 128, 640, 480));
            let f = frame(&c, &project, 0, &provider);
            let expected = adjust_encoded([200.0 / 255.0; 3], adjust);
            let got = f.pixel(320, 240);
            for i in 0..3 {
                let wanted = (expected[i] * 255.0).round() as i32;
                assert!(
                    (got[i] as i32 - wanted).abs() <= 2,
                    "{name}: channel {i} is {}, reference says {wanted}",
                    got[i]
                );
            }
        }

        // Saturation zero turns pure red into its BT.601 grey — 0.299, byte
        // 76 — through the software path, since red needs chroma.
        let mut project = split_project("clip");
        grade(&mut project, [0.0, 1.0, 0.0, 0.0]);
        let provider =
            MixedProvider::default().with("clip", TestSource::Rgba([255, 0, 0, 255], 640, 480));
        let f = frame(&c, &project, 0, &provider);
        let [r, g, b, _] = f.pixel(320, 240);
        for (channel, value) in [("r", r), ("g", g), ("b", b)] {
            assert!(
                (value as i32 - 76).abs() <= 2,
                "{channel} is {value}, expected desaturated red at 76"
            );
        }
    }

    /// Preview and export must show the same grade. Same shape as the
    /// transition agreement test and for the same reason: both call
    /// `render_to_texture`, and this pins that the colour pass went in there
    /// rather than into one of the two callers.
    #[test]
    fn the_preview_and_the_export_agree_on_a_graded_frame() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        for (name, source) in [
            ("software", TestSource::Rgba([255, 255, 255, 255], 640, 480)),
            ("hardware", TestSource::Planar(255, 128, 128, 640, 480)),
        ] {
            let mut project = split_project("clip");
            grade(&mut project, [-0.1, 1.2, 0.8, 0.5]);
            let provider = MixedProvider::default().with("clip", source);

            let preview = c
                .render(&project, 0, (640, 480), &provider)
                .expect("preview render");
            let Ok(export) = c.render_nv12(&project, 0, (640, 480), &provider) else {
                eprintln!("skipping: no RGBA to NV12 compute pass on this device");
                return;
            };

            for (x, y) in [(0, 0), (320, 240), (639, 479), (17, 300)] {
                let expected = preview.pixel(x, y);
                let actual = nv12_pixel(&export, x, y);
                for channel in 0..3 {
                    assert!(
                        (expected[channel] as i32 - actual[channel] as i32).abs() <= 3,
                        "{name}: at ({x},{y}), preview {expected:?} against export {actual:?}"
                    );
                }
            }

            // And the grade really was live in what we compared.
            let [r, _, b, _] = preview.pixel(320, 240);
            assert!(
                r != 255 || b != 255,
                "{name}: the graded frame looks ungraded: {:?}",
                preview.pixel(320, 240)
            );
        }
    }

    // -----------------------------------------------------------------------
    // LUTs
    //
    // Fixture cubes are built by `render::lut::fixtures` so their effect is
    // arithmetic, not a file someone once trusted. Full-range achromatic NV12
    // is the workhorse source again: its encoded value *is* its code value,
    // so a LUT's expected output is a number a reviewer can recompute in
    // their head.
    // -----------------------------------------------------------------------

    /// An identity .cube at full intensity must change nothing — byte for
    /// byte, which is what the hand-rolled trilinear in `sample_lut` exists
    /// for: hardware filtering quantises its weights and fails this.
    #[test]
    fn an_identity_lut_renders_byte_identical_to_no_lut() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let path = write_lut(
            "identity",
            &crate::modules::render::lut::fixtures::identity_cube(2),
        );
        let source = TestSource::Planar(173, 90, 200, 640, 480);
        let provider = MixedProvider::default().with("clip", source);

        let mut project = split_project("clip");
        let before = frame(&c, &project, 0, &provider);

        grade_with_lut(&mut project, [0.0, 1.0, 1.0, 0.0], &path, 1.0);
        let with_lut = frame(&c, &project, 0, &provider);
        assert_eq!(before.data, with_lut.data, "an identity LUT changed pixels");

        // A larger identity cube interpolates between grid points and must
        // still be exact — trilinear over identity data is the identity.
        let path = write_lut(
            "identity17",
            &crate::modules::render::lut::fixtures::identity_cube(17),
        );
        project.materials.color_adjusts[0]
            .lut
            .as_mut()
            .unwrap()
            .path = path.to_string_lossy().into_owned();
        let with_larger = frame(&c, &project, 0, &provider);
        assert_eq!(
            before.data, with_larger.data,
            "a 17-point identity LUT changed pixels"
        );
    }

    /// The analytic fixtures, on both decode paths. Inversion and halving on
    /// achromatic NV12 read straight off in code values; the channel swap
    /// needs colour, which is the RGBA path's clean case.
    #[test]
    fn a_lut_applies_its_analytic_transform_on_both_decode_paths() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let invert = write_lut(
            "invert",
            &crate::modules::render::lut::fixtures::cube_text(2, |r, g, b| {
                [1.0 - r, 1.0 - g, 1.0 - b]
            }),
        );
        let swap = write_lut(
            "swap",
            &crate::modules::render::lut::fixtures::cube_text(2, |r, g, b| [b, g, r]),
        );
        let halve = write_lut(
            "halve",
            &crate::modules::render::lut::fixtures::cube_text(2, |r, g, b| {
                [r * 0.5, g * 0.5, b * 0.5]
            }),
        );

        // (name, source, lut, expected RGB at the centre)
        let cases: Vec<(&str, TestSource, &std::path::Path, [i32; 3])> = vec![
            // 255 - 200 = 55, through the hardware path.
            (
                "invert hw grey",
                TestSource::Planar(200, 128, 128, 640, 480),
                &invert,
                [55, 55, 55],
            ),
            // And through the software path at the extremes.
            (
                "invert sw white",
                TestSource::Rgba([255, 255, 255, 255], 640, 480),
                &invert,
                [0, 0, 0],
            ),
            (
                "swap sw red",
                TestSource::Rgba([255, 0, 0, 255], 640, 480),
                &swap,
                [0, 0, 255],
            ),
            // The 2-point gradient: everything halves, in encoded space.
            (
                "halve hw grey",
                TestSource::Planar(100, 128, 128, 640, 480),
                &halve,
                [50, 50, 50],
            ),
            (
                "halve sw white",
                TestSource::Rgba([255, 255, 255, 255], 640, 480),
                &halve,
                [128, 128, 128],
            ),
        ];
        for (name, source, lut, expected) in cases {
            let mut project = split_project("clip");
            grade_with_lut(&mut project, [0.0, 1.0, 1.0, 0.0], lut, 1.0);
            let provider = MixedProvider::default().with("clip", source);
            let got = frame(&c, &project, 0, &provider).pixel(320, 240);
            for channel in 0..3 {
                assert!(
                    (got[channel] as i32 - expected[channel]).abs() <= 1,
                    "{name}: channel {channel} is {}, expected {}",
                    got[channel],
                    expected[channel]
                );
            }
            assert_eq!(got[3], 255, "{name}: a LUT never touches coverage");
        }
    }

    /// Intensity is a lerp between the input and the look, and 0 takes the
    /// LUT path out entirely — exact code values, not merely "less effect".
    #[test]
    fn lut_intensity_lerps_between_input_and_look() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let invert = write_lut(
            "invert-intensity",
            &crate::modules::render::lut::fixtures::cube_text(2, |r, g, b| {
                [1.0 - r, 1.0 - g, 1.0 - b]
            }),
        );
        let provider =
            MixedProvider::default().with("clip", TestSource::Planar(200, 128, 128, 640, 480));

        // 0.75 * 200 + 0.25 * 55 = 163.75.
        let mut project = split_project("clip");
        grade_with_lut(&mut project, [0.0, 1.0, 1.0, 0.0], &invert, 0.25);
        let quarter = frame(&c, &project, 0, &provider).pixel(320, 240);
        assert!(
            (quarter[0] as i32 - 164).abs() <= 1,
            "quarter intensity gave {quarter:?}"
        );

        // Intensity 0 is not "the LUT, weakly": the resolver drops it, the
        // flag stays 0, and the clip renders on the untouched path.
        let mut project = split_project("clip");
        grade_with_lut(&mut project, [0.0, 1.0, 1.0, 0.0], &invert, 0.0);
        let off = frame(&c, &project, 0, &provider);
        let plain = frame(&c, &split_project("clip"), 0, &provider);
        assert_eq!(
            off.data, plain.data,
            "intensity 0 must be byte-identical to no LUT"
        );
    }

    /// Grade first, look second. Brightness +0.2 then invert on code 100:
    /// (100/255 + 0.2) inverted is 0.408 → 104. The reversed order would give
    /// (1 - 100/255) + 0.2 = 0.808 → 206, so this pins the order, not just
    /// "both ran".
    #[test]
    fn the_lut_applies_after_the_grade() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let invert = write_lut(
            "invert-order",
            &crate::modules::render::lut::fixtures::cube_text(2, |r, g, b| {
                [1.0 - r, 1.0 - g, 1.0 - b]
            }),
        );
        let mut project = split_project("clip");
        grade_with_lut(&mut project, [0.2, 1.0, 1.0, 0.0], &invert, 1.0);
        let provider =
            MixedProvider::default().with("clip", TestSource::Planar(100, 128, 128, 640, 480));
        let got = frame(&c, &project, 0, &provider).pixel(320, 240);
        assert!(
            (got[0] as i32 - 104).abs() <= 1,
            "grade-then-look should give 104, got {got:?}"
        );
    }

    /// The reopen contract: a LUT whose file is gone renders the clip
    /// *unadjusted* — byte-identical to no material when the scalars are at
    /// rest, and byte-identical to the grade alone when they are not. A
    /// malformed file behaves the same, and neither kills the frame.
    #[test]
    fn a_missing_or_malformed_lut_file_renders_the_clip_unadjusted() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let source = TestSource::Planar(173, 90, 200, 640, 480);
        let provider = MixedProvider::default().with("clip", source);
        let missing = std::path::PathBuf::from("/nonexistent/look.cube");
        let garbage = write_lut("garbage", "LUT_3D_SIZE 2\nnot numbers at all\n");

        let plain = frame(&c, &split_project("clip"), 0, &provider);

        for path in [&missing, &garbage] {
            let mut project = split_project("clip");
            grade_with_lut(&mut project, [0.0, 1.0, 1.0, 0.0], path, 1.0);
            // Twice, so the second frame exercises the cached-failure path
            // rather than re-reading the file.
            let first = frame(&c, &project, 0, &provider);
            let second = frame(&c, &project, 0, &provider);
            assert_eq!(
                plain.data, first.data,
                "{path:?}: identity scalars, no look"
            );
            assert_eq!(first.data, second.data);
        }

        // With real scalars, the grade still applies without the look.
        let mut graded_only = split_project("clip");
        grade(&mut graded_only, [0.1, 1.0, 1.0, 0.0]);
        let expected = frame(&c, &graded_only, 0, &provider);

        let mut project = split_project("clip");
        grade_with_lut(&mut project, [0.1, 1.0, 1.0, 0.0], &missing, 1.0);
        let got = frame(&c, &project, 0, &provider);
        assert_eq!(expected.data, got.data, "the grade must survive a lost LUT");
    }

    /// The cache is keyed by mtime: editing the file shows up on the next
    /// frame, with no invalidation call anywhere.
    #[test]
    fn an_edited_lut_file_is_picked_up_on_the_next_frame() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let path = write_lut(
            "edited",
            &crate::modules::render::lut::fixtures::identity_cube(2),
        );
        let provider =
            MixedProvider::default().with("clip", TestSource::Planar(200, 128, 128, 640, 480));
        let mut project = split_project("clip");
        grade_with_lut(&mut project, [0.0, 1.0, 1.0, 0.0], &path, 1.0);

        let before = frame(&c, &project, 0, &provider).pixel(320, 240);
        assert!(
            (before[0] as i32 - 200).abs() <= 1,
            "identity first: {before:?}"
        );

        // A pause so the rewrite cannot land on the same mtime even on a
        // coarse-timestamp filesystem.
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(
            &path,
            crate::modules::render::lut::fixtures::cube_text(2, |r, g, b| {
                [1.0 - r, 1.0 - g, 1.0 - b]
            }),
        )
        .expect("rewrite LUT");

        let after = frame(&c, &project, 0, &provider).pixel(320, 240);
        assert!(
            (after[0] as i32 - 55).abs() <= 1,
            "the edit was not picked up: {after:?}"
        );
    }

    /// Preview and export agree on a graded-and-looked frame, both decode
    /// paths — the same claim the transition and grade parity tests make,
    /// extended over the LUT.
    #[test]
    fn the_preview_and_the_export_agree_on_a_lut_frame() {
        let Some(c) = srgb_compositor() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let invert = write_lut(
            "invert-parity",
            &crate::modules::render::lut::fixtures::cube_text(2, |r, g, b| {
                [1.0 - r, 1.0 - g, 1.0 - b]
            }),
        );
        for (name, source) in [
            ("software", TestSource::Rgba([230, 120, 40, 255], 640, 480)),
            ("hardware", TestSource::Planar(200, 100, 180, 640, 480)),
        ] {
            let mut project = split_project("clip");
            grade_with_lut(&mut project, [-0.1, 1.2, 0.8, 0.3], &invert, 0.6);
            let provider = MixedProvider::default().with("clip", source);

            let preview = c
                .render(&project, 0, (640, 480), &provider)
                .expect("preview render");
            let Ok(export) = c.render_nv12(&project, 0, (640, 480), &provider) else {
                eprintln!("skipping: no RGBA to NV12 compute pass on this device");
                return;
            };

            for (x, y) in [(0, 0), (320, 240), (639, 479), (17, 300)] {
                let expected = preview.pixel(x, y);
                let actual = nv12_pixel(&export, x, y);
                for channel in 0..3 {
                    assert!(
                        (expected[channel] as i32 - actual[channel] as i32).abs() <= 3,
                        "{name}: at ({x},{y}), preview {expected:?} against export {actual:?}"
                    );
                }
            }
        }
    }

    // The extended grade's pixel tests, in their own file for length. An
    // `include!` rather than `#[path]`, because a `#[path]` inside an inline
    // module resolves under `compositor/tests/`, a directory that does not
    // exist.
    mod grade_tests {
        include!("grade_tests.rs");
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
