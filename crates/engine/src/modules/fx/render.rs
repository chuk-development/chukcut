//! The GPU side of built-in effects: one shader module, lazily built
//! pipelines, and a recorder that runs an effect stack over a layer.
//!
//! ## What it takes
//!
//! A full-canvas **layer** (straight alpha, in the compositor's format) and a
//! list of [`FxInstance`]s — effects already resolved to numbers for this
//! instant. It hands back a new layer in a format the caller chooses. It
//! never sees a segment or a document; the compositor decides *what* gets a
//! layer (an effected clip, one side of a transition, or everything beneath
//! an effect clip) and this module only decides what happens to its pixels.
//!
//! ## The chain
//!
//! ```text
//!   layer ─ fs_import ─► premultiplied Rgba16Float ─ effect passes … ─ fs_export ─► layer
//! ```
//!
//! Every effect pass reads and writes premultiplied linear colour in a pooled
//! `Rgba16Float` texture; see the conventions at the top of `fx.wgsl`. All of
//! a frame's passes are recorded into the compositor's one command encoder,
//! so an effect costs no extra submission and no readback.
//!
//! ## Units
//!
//! The document stores what the user set, mostly `0..100`. Everything is
//! turned into pixels of the frame being drawn *here*, as a fraction of the
//! frame's shorter side, so the preview's small frame and the export's large
//! one look the same: a blur of 20 is the same blur at 360p and at 4K.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::{Mutex, MutexGuard};

use crate::modules::project::document::{MaterialPool, Micros, Segment};
use crate::modules::project::effects::EffectMaterial;
use crate::modules::render::texture_pool::{PooledTexture, TextureKey, TexturePool};
use crate::modules::render::RenderContext;

use super::catalog::{self, descriptor, EffectDescriptor, ParamKind};

/// Intermediate format: linear light needs more than eight bits, and a glow
/// adds light above 1.0 that must survive until the export pass.
const WORK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const WORK_USAGE: wgpu::TextureUsages =
    wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::TEXTURE_BINDING);

/// Parameters an instance can carry, the most any catalog entry has.
const MAX_PARAMS: usize = 8;

/// One effect, resolved for one instant: numbers in catalog order, colours,
/// the clip's clock and seed, and where the clip sits on the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxInstance {
    pub desc: &'static EffectDescriptor,
    /// `numbers[i]` is `desc.params[i]` when it is a number or a choice.
    pub numbers: [f32; MAX_PARAMS],
    /// `colors[i]` is `desc.params[i]` when it is a colour.
    pub colors: [[f32; 4]; MAX_PARAMS],
    /// The clip's source time, the clock animated effects run on.
    pub time: Micros,
    pub seed: u32,
    /// The clip's quad as the compositor placed it (its model-view-projection
    /// matrix, column major), for effects drawn in the clip's own frame. `None`
    /// on an effect clip, where "the clip" is the whole canvas.
    pub placement: Option<[f32; 16]>,
}

impl FxInstance {
    /// Resolve `effect` at `source_time`. `None` for an effect that is
    /// switched off, unknown to this build, or at rest — an instance that
    /// would leave every pixel as it was is not worth a layer.
    pub fn resolve(
        effect: &EffectMaterial,
        source_time: Micros,
        placement: Option<[f32; 16]>,
    ) -> Option<Self> {
        if !effect.enabled {
            return None;
        }
        let desc = descriptor(&effect.kind)?;
        let mut numbers = [0.0; MAX_PARAMS];
        let mut colors = [[0.0; 4]; MAX_PARAMS];
        for (i, spec) in desc.params.iter().enumerate().take(MAX_PARAMS) {
            match spec.kind {
                ParamKind::Color { default } => colors[i] = effect.color(spec.id, default),
                _ => {
                    numbers[i] =
                        spec.clamp(effect.number_at(spec.id, source_time, spec.default_number()))
                }
            }
        }
        let instance = Self {
            desc,
            numbers,
            colors,
            time: source_time,
            seed: effect.seed & 0x00ff_ffff,
            placement,
        };
        (!instance.at_rest()).then_some(instance)
    }

    pub fn get(&self, id: &str) -> f32 {
        self.desc
            .params
            .iter()
            .position(|p| p.id == id)
            .map_or(0.0, |i| self.numbers[i])
    }

    pub fn color(&self, id: &str) -> [f32; 4] {
        self.desc
            .params
            .iter()
            .position(|p| p.id == id)
            .map_or([0.0; 4], |i| self.colors[i])
    }

    fn seconds(&self) -> f32 {
        self.time as f32 / 1_000_000.0
    }

    /// Whether this instance would change no pixel.
    pub fn at_rest(&self) -> bool {
        use catalog::*;
        match self.desc.id {
            GAUSSIAN_BLUR => self.get("radius") <= 0.0,
            ZOOM_BLUR => self.get("strength") <= 0.0,
            GLOW => self.get("intensity") <= 0.0,
            LIGHT_SWEEP => self.get("intensity") <= 0.0,
            SHAKE => {
                self.get("amplitude") <= 0.0
                    && self.get("rotation") <= 0.0
                    && self.get("zoom") <= 0.0
            }
            RGB_SPLIT => self.get("amount") <= 0.0,
            GLITCH => self.get("intensity") <= 0.0,
            VHS => self.get("intensity") <= 0.0 && self.get("noise") <= 0.0,
            PIXELATE => self.get("size") <= 0.0,
            GRAIN => self.get("amount") <= 0.0,
            HALATION | BLOOM => self.get("amount") <= 0.0,
            GATE_WEAVE => self.get("amount") <= 0.0,
            LETTERBOX => self.get("opacity") <= 0.0,
            FRAME => {
                self.get("radius") <= 0.0 && self.get("border") <= 0.0 && self.get("shadow") <= 0.0
            }
            _ => false,
        }
    }
}

/// The effects `segment` applies at `source_time`, resolved, in order.
pub fn chain_for(
    materials: &MaterialPool,
    segment: &Segment,
    source_time: Micros,
    placement: Option<[f32; 16]>,
) -> Vec<FxInstance> {
    materials
        .effects_of(segment)
        .into_iter()
        .filter_map(|e| FxInstance::resolve(e, source_time, placement))
        .collect()
}

// ---------------------------------------------------------------------------
// Motion that the CPU computes, so tests can predict the picture
// ---------------------------------------------------------------------------

fn pcg(v: u32) -> u32 {
    let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

/// Smooth noise in `-1..1`: random values at whole numbers of `x`,
/// smoothstepped between. `channel` picks an independent sequence.
fn value_noise(seed: u32, channel: u32, x: f32) -> f32 {
    let i = x.floor();
    let f = x - i;
    let at = |n: i64| {
        let h = pcg((n as u32) ^ pcg(seed.wrapping_add(channel.wrapping_mul(0x9e37_79b9))));
        (h & 0xffff) as f32 / 65535.0 * 2.0 - 1.0
    };
    let (a, b) = (at(i as i64), at(i as i64 + 1));
    let s = f * f * (3.0 - 2.0 * f);
    a + (b - a) * s
}

/// The offset (pixels, y down), rotation (degrees, clockwise) and zoom of a
/// shake at `time` seconds, for a frame whose shorter side is `short` pixels.
///
/// Two octaves of value noise per axis, so the jolt has a beat at
/// `frequency` and a finer tremble on top. Seeded, and a pure function of the
/// clip's clock: the preview and the export shake the same way.
pub fn shake_at(instance: &FxInstance, short: f32) -> ([f32; 2], f32, f32) {
    let t = instance.seconds();
    let f = instance.get("frequency").max(0.1);
    let seed = instance.seed;
    let wobble =
        |c: u32| value_noise(seed, c, t * f) * 0.75 + value_noise(seed, c + 7, t * f * 2.3) * 0.25;
    let amp = instance.get("amplitude") / 100.0 * 0.05 * short;
    let rot = instance.get("rotation") / 100.0 * 4.0;
    let zoom = 1.0 + instance.get("zoom") / 100.0 * 0.25;
    ([wobble(0) * amp, wobble(1) * amp], wobble(2) * rot, zoom)
}

/// The same for gate weave: a slow, small drift, mostly vertical.
pub fn weave_at(instance: &FxInstance, short: f32) -> ([f32; 2], f32, f32) {
    let t = instance.seconds();
    let f = 0.5 + instance.get("speed") / 100.0 * 3.5;
    let seed = instance.seed;
    let amount = instance.get("amount") / 100.0;
    let px = amount * 0.004 * short;
    (
        [
            value_noise(seed, 11, t * f) * px * 0.5,
            value_noise(seed, 12, t * f) * px,
        ],
        value_noise(seed, 13, t * f) * amount * 0.15,
        1.0,
    )
}

/// Where the light sweep's band is, in half diagonals from the centre: it
/// enters at one corner and leaves at the other once per period.
pub fn sweep_position(instance: &FxInstance) -> f32 {
    let period = instance.get("period").max(0.05);
    let phase = (instance.seconds() / period).rem_euclid(1.0);
    -1.25 + 2.5 * phase
}

/// The Gaussian sigma, in pixels, for a blur radius of `0..100`.
pub fn blur_sigma(radius: f32, short: f32) -> f32 {
    radius / 100.0 * 0.04 * short
}

/// Taps per side and the stride between them for a blur of `sigma` pixels:
/// three sigma covered, at most 48 taps a side, every tap on a whole texel.
pub fn blur_taps(sigma: f32) -> (i32, i32) {
    let extent = (3.0 * sigma).ceil().max(1.0) as i32;
    let stride = ((extent as f32) / 48.0).ceil().max(1.0) as i32;
    ((extent + stride - 1) / stride, stride)
}

/// Pixelate's block size in whole pixels.
pub fn pixel_block(size: f32, short: f32) -> f32 {
    (2.0 + size / 100.0 * 0.08 * short).round()
}

/// RGB split's offset in pixels.
pub fn split_offset(instance: &FxInstance, short: f32) -> [f32; 2] {
    let d = instance.get("amount") / 100.0 * 0.02 * short;
    let a = instance.get("angle").to_radians();
    [a.cos() * d, a.sin() * d]
}

/// Letterbox bars: (height, width) as fractions of the frame, per side.
pub fn letterbox_bars(aspect: f32, frame: (u32, u32)) -> (f32, f32) {
    let f = frame.0.max(1) as f32 / frame.1.max(1) as f32;
    let a = aspect.max(0.01);
    if a > f {
        ((1.0 - f / a) * 0.5, 0.0)
    } else {
        (0.0, (1.0 - a / f) * 0.5)
    }
}

/// The clip's rectangle in pixels of a `size` frame: centre and the two half
/// axes, from its placement matrix. The whole frame without one.
pub fn clip_rect(placement: Option<[f32; 16]>, size: (u32, u32)) -> ([f32; 2], [f32; 2], [f32; 2]) {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let Some(m) = placement else {
        return ([w * 0.5, h * 0.5], [w * 0.5, 0.0], [0.0, h * 0.5]);
    };
    let m = glam::Mat4::from_cols_array(&m);
    let c = m * glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
    let ax = m * glam::Vec4::new(0.5, 0.0, 0.0, 0.0);
    let ay = m * glam::Vec4::new(0.0, 0.5, 0.0, 0.0);
    (
        [(c.x + 1.0) * 0.5 * w, (1.0 - c.y) * 0.5 * h],
        [ax.x * 0.5 * w, -ax.y * 0.5 * h],
        [ay.x * 0.5 * w, -ay.y * 0.5 * h],
    )
}

// ---------------------------------------------------------------------------
// The renderer
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FxUniform {
    out_size: [f32; 4],
    in_size: [f32; 4],
    frame: [f32; 4],
    p: [[f32; 4]; 6],
}

// `#[repr(C)]`, all `f32` arrays, no padding. By hand for the reason
// `render/compositor.rs` gives.
unsafe impl bytemuck::Zeroable for FxUniform {}
unsafe impl bytemuck::Pod for FxUniform {}

/// How a pass's output meets what is already in its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blend {
    /// Written through: every pass that fills a target of its own.
    Replace,
    /// Straight-alpha source-over: an effected clip landing on the frame.
    Over,
}

type PipelineCache = HashMap<(&'static str, wgpu::TextureFormat, Blend), Arc<wgpu::RenderPipeline>>;

/// The shader module, the layouts and the pipelines, built per entry point
/// and target format on first use.
pub struct FxRenderer {
    module: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Bound as the second input of a pass that has only one.
    placeholder: wgpu::TextureView,
    pipelines: Mutex<PipelineCache>,
    uniforms: Mutex<Scratch>,
    stride: u64,
}

#[derive(Default)]
struct Scratch {
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
}

impl FxRenderer {
    pub fn new(ctx: &RenderContext) -> Self {
        let device = ctx.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chukcut fx shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/fx.wgsl").into()),
        });
        let uniform_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chukcut fx uniforms"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<FxUniform>() as u64
                        ),
                    },
                    count: None,
                }],
            });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut fx inputs"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chukcut fx pipeline layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("chukcut fx sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let placeholder = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("chukcut fx placeholder"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORK_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let align = ctx.limits().min_uniform_buffer_offset_alignment.max(1) as u64;
        let stride = (std::mem::size_of::<FxUniform>() as u64).div_ceil(align) * align;
        Self {
            module,
            pipeline_layout,
            uniform_layout,
            texture_layout,
            sampler,
            placeholder,
            pipelines: Mutex::new(HashMap::new()),
            uniforms: Mutex::new(Scratch::default()),
            stride,
        }
    }

    /// The pipeline for `entry` writing `format`, built the first time it is
    /// asked for. One entry point is one small compile; building all of them
    /// up front would put every effect's compile on the first frame of any.
    fn pipeline(
        &self,
        device: &wgpu::Device,
        entry: &'static str,
        format: wgpu::TextureFormat,
        blend: Blend,
    ) -> Arc<wgpu::RenderPipeline> {
        let mut cache = self.pipelines.lock();
        Arc::clone(cache.entry((entry, format, blend)).or_insert_with(|| {
            Arc::new(
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &self.module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &self.module,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: match blend {
                                Blend::Replace => None,
                                Blend::Over => Some(wgpu::BlendState::ALPHA_BLENDING),
                            },
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                }),
            )
        }))
    }

    /// Start a frame's worth of effect passes. Holds the uniform buffer until
    /// the returned frame is finished, so two threads rendering frames at once
    /// cannot write each other's parameters.
    pub fn begin<'a>(&'a self, ctx: &'a RenderContext, pool: &'a TexturePool) -> FxFrame<'a> {
        let uniforms = self.uniforms.lock();
        let mut frame = FxFrame {
            fx: self,
            ctx,
            pool,
            uniforms,
            group: None,
            capacity: 0,
            next: 0,
            temps: Vec::new(),
        };
        frame.grow(64);
        frame
    }

    fn bind_inputs(
        &self,
        device: &wgpu::Device,
        a: &wgpu::TextureView,
        b: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut fx inputs"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b.unwrap_or(&self.placeholder)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}

impl std::fmt::Debug for FxRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FxRenderer")
            .field("pipelines", &self.pipelines.lock().len())
            .finish()
    }
}

/// A texture a chain has written, with its size.
struct Work {
    texture: PooledTexture,
}

impl Work {
    fn size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }
}

/// A prepared draw of a finished layer into a pass the caller opened: see
/// [`FxFrame::prepare_over`].
pub struct OverDraw {
    pipeline: Arc<wgpu::RenderPipeline>,
    uniform_group: wgpu::BindGroup,
    offset: u32,
    inputs: wgpu::BindGroup,
}

impl OverDraw {
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_group, &[self.offset]);
        pass.set_bind_group(1, &self.inputs, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// One frame's effect passes. See [`FxRenderer::begin`].
pub struct FxFrame<'a> {
    fx: &'a FxRenderer,
    ctx: &'a RenderContext,
    pool: &'a TexturePool,
    uniforms: MutexGuard<'a, Scratch>,
    group: Option<wgpu::BindGroup>,
    capacity: u32,
    next: u32,
    /// Intermediate textures, given back to the pool by [`Self::finish`].
    temps: Vec<PooledTexture>,
}

impl<'a> FxFrame<'a> {
    /// A fresh, larger uniform buffer for the passes still to come. Passes
    /// already recorded keep the buffer they were bound to: the command
    /// encoder holds it alive, and each block was written to it already.
    fn grow(&mut self, slots: u32) {
        let device = self.ctx.device();
        let size = self.fx.stride * slots as u64;
        if self.uniforms.capacity < size || self.uniforms.buffer.is_none() || self.capacity > 0 {
            self.uniforms.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chukcut fx uniforms"),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.uniforms.capacity = size;
        }
        let buffer = self.uniforms.buffer.as_ref().expect("just ensured");
        self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut fx uniform group"),
            layout: &self.fx.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<FxUniform>() as u64),
                }),
            }],
        }));
        self.capacity = (self.uniforms.capacity / self.fx.stride) as u32;
        self.next = 0;
    }

    /// Write a uniform block and return the bind group and offset it is at.
    fn block(&mut self, block: FxUniform) -> (wgpu::BindGroup, u32) {
        if self.next >= self.capacity {
            self.grow(self.capacity.max(32) * 2);
        }
        let slot = self.next;
        self.next += 1;
        let buffer = self.uniforms.buffer.as_ref().expect("grown");
        self.ctx.queue().write_buffer(
            buffer,
            slot as u64 * self.fx.stride,
            bytemuck::bytes_of(&block),
        );
        (
            self.group.clone().expect("grown"),
            slot * self.fx.stride as u32,
        )
    }

    fn work(&mut self, size: (u32, u32)) -> PooledTexture {
        self.pool.acquire(
            self.ctx.device(),
            TextureKey::new(size.0.max(1), size.1.max(1), WORK_FORMAT, WORK_USAGE),
        )
    }

    /// Record one fullscreen pass of `entry` from `inputs` into `target`.
    #[allow(clippy::too_many_arguments)]
    fn pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        entry: &'static str,
        input: &wgpu::TextureView,
        input_size: (u32, u32),
        second: Option<&wgpu::TextureView>,
        target: &wgpu::TextureView,
        target_format: wgpu::TextureFormat,
        target_size: (u32, u32),
        frame: [f32; 4],
        p: [[f32; 4]; 6],
    ) {
        let device = self.ctx.device();
        let pipeline = self
            .fx
            .pipeline(device, entry, target_format, Blend::Replace);
        let inv = |v: u32| 1.0 / v.max(1) as f32;
        let (group, offset) = self.block(FxUniform {
            out_size: [
                target_size.0 as f32,
                target_size.1 as f32,
                inv(target_size.0),
                inv(target_size.1),
            ],
            in_size: [
                input_size.0 as f32,
                input_size.1 as f32,
                inv(input_size.0),
                inv(input_size.1),
            ],
            frame,
            p,
        });
        let inputs = self.fx.bind_inputs(device, input, second);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(entry),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[offset]);
        pass.set_bind_group(1, &inputs, &[]);
        pass.draw(0..3, 0..1);
    }

    /// One effect pass from `input` into a new working texture of `size`.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        entry: &'static str,
        input: &Work,
        second: Option<&Work>,
        size: (u32, u32),
        frame: [f32; 4],
        p: [[f32; 4]; 6],
    ) -> Work {
        let out = self.work(size);
        self.pass(
            encoder,
            entry,
            input.texture.view(),
            input.size(),
            second.map(|w| w.texture.view()),
            out.view(),
            WORK_FORMAT,
            size,
            frame,
            p,
        );
        Work { texture: out }
    }

    fn retire(&mut self, work: Work) {
        self.temps.push(work.texture);
    }

    /// Run `chain` over `input` (straight alpha, `size`) and return the result
    /// in a fresh texture of `out_key` (straight alpha). The caller owns the
    /// returned texture; intermediates go back to the pool in [`Self::finish`].
    pub fn apply(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        size: (u32, u32),
        chain: &[FxInstance],
        out_key: TextureKey,
    ) -> PooledTexture {
        let imported = self.work(size);
        self.pass(
            encoder,
            "fs_import",
            input,
            size,
            None,
            imported.view(),
            WORK_FORMAT,
            size,
            [size.0 as f32, size.1 as f32, 0.0, 0.0],
            [[0.0; 4]; 6],
        );
        let mut current = Work { texture: imported };
        for instance in chain {
            current = self.record(encoder, instance, current, size);
        }
        let out = self.pool.acquire(self.ctx.device(), out_key);
        self.pass(
            encoder,
            "fs_export",
            current.texture.view(),
            size,
            None,
            out.view(),
            out_key.format,
            (out_key.width, out_key.height),
            [size.0 as f32, size.1 as f32, 0.0, 0.0],
            [[0.0; 4]; 6],
        );
        self.retire(current);
        out
    }

    /// The draw of a finished straight-alpha layer over whatever a pass the
    /// caller opens later has drawn so far. Prepared ahead because the bind
    /// groups have to exist before that pass begins.
    pub fn prepare_over(
        &mut self,
        layer: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: (u32, u32),
    ) -> OverDraw {
        let device = self.ctx.device();
        let pipeline = self.fx.pipeline(device, "fs_copy", format, Blend::Over);
        let (uniform_group, offset) = self.block(FxUniform {
            frame: [size.0 as f32, size.1 as f32, 0.0, 0.0],
            ..Default::default()
        });
        let inputs = self.fx.bind_inputs(device, layer, None);
        OverDraw {
            pipeline,
            uniform_group,
            offset,
            inputs,
        }
    }

    /// Lay `layer` (a clip, straight alpha) onto `base` (the frame so far)
    /// with blend mode `mode` (`BlendMode::code`), into a fresh texture of
    /// `out_key`, which the caller owns. Both inputs are in `out_key.format`.
    pub fn blend(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        base: &wgpu::TextureView,
        layer: &wgpu::TextureView,
        size: (u32, u32),
        mode: u32,
        out_key: TextureKey,
    ) -> PooledTexture {
        let out = self.pool.acquire(self.ctx.device(), out_key);
        let srgb = if out_key.format.is_srgb() { 1.0 } else { 0.0 };
        let mut p = [[0.0; 4]; 6];
        p[0] = [mode as f32, srgb, 0.0, 0.0];
        self.pass(
            encoder,
            "fs_blend",
            base,
            size,
            Some(layer),
            out.view(),
            out_key.format,
            (out_key.width, out_key.height),
            [size.0 as f32, size.1 as f32, 0.0, 0.0],
            p,
        );
        out
    }

    /// Give every intermediate texture back. Call after the encoder has been
    /// submitted.
    pub fn finish(self) {
        for texture in self.temps {
            self.pool.release(texture);
        }
    }

    /// The passes of one effect.
    fn record(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        fx: &FxInstance,
        input: Work,
        size: (u32, u32),
    ) -> Work {
        use catalog::*;
        let short = size.0.min(size.1) as f32;
        let seed = (fx.seed & 0x00ff_ffff) as f32;
        let frame = [size.0 as f32, size.1 as f32, fx.seconds(), seed];
        let mut p = [[0.0f32; 4]; 6];
        let out = match fx.desc.id {
            GAUSSIAN_BLUR => {
                let sigma = blur_sigma(fx.get("radius"), short);
                return self.blur(encoder, input, sigma, frame);
            }
            ZOOM_BLUR => {
                let strength = fx.get("strength") / 100.0 * 0.5;
                p[0] = [
                    0.5 + fx.get("center_x") / 200.0,
                    0.5 - fx.get("center_y") / 200.0,
                    strength,
                    (12.0 + strength * 80.0).round(),
                ];
                self.step(encoder, "fs_zoom_blur", &input, None, size, frame, p)
            }
            GLOW => {
                let threshold = srgb_to_linear(fx.get("threshold") / 100.0);
                let sigma = fx.get("radius") / 100.0 * 0.05 * short + 2.0;
                let tint = fx.color("tint");
                let amount = fx.get("intensity") / 100.0 * 2.0;
                return self.add_light(
                    encoder,
                    input,
                    [threshold, 0.08, fx.get("source"), 0.0],
                    sigma,
                    amount,
                    tint,
                    frame,
                );
            }
            BLOOM => {
                let threshold = srgb_to_linear(fx.get("threshold") / 100.0);
                let sigma = fx.get("radius") / 100.0 * 0.08 * short + 2.0;
                let amount = fx.get("amount") / 100.0 * 1.5;
                return self.add_light(
                    encoder,
                    input,
                    [threshold, 0.25, 0.0, 0.0],
                    sigma,
                    amount,
                    [1.0; 4],
                    frame,
                );
            }
            HALATION => {
                let threshold = srgb_to_linear(fx.get("threshold") / 100.0);
                let sigma = fx.get("radius") / 100.0 * 0.025 * short + 1.5;
                let amount = fx.get("amount") / 100.0 * 1.5;
                return self.add_light(
                    encoder,
                    input,
                    [threshold, 0.1, 0.0, 0.0],
                    sigma,
                    amount,
                    fx.color("color"),
                    frame,
                );
            }
            LIGHT_SWEEP => {
                let angle = fx.get("angle").to_radians();
                let c = fx.color("color");
                p[0] = [
                    sweep_position(fx),
                    fx.get("width") / 100.0 * 0.6,
                    angle.cos(),
                    angle.sin(),
                ];
                p[1] = [c[0], c[1], c[2], fx.get("intensity") / 100.0 * 1.5];
                self.step(encoder, "fs_light_sweep", &input, None, size, frame, p)
            }
            SHAKE | GATE_WEAVE => {
                let (offset, degrees, zoom) = if fx.desc.id == SHAKE {
                    shake_at(fx, short)
                } else {
                    weave_at(fx, short)
                };
                let r = degrees.to_radians();
                p[0] = [r.cos(), r.sin(), 1.0 / zoom, 0.0];
                p[1] = [offset[0], offset[1], 0.0, 0.0];
                self.step(encoder, "fs_transform", &input, None, size, frame, p)
            }
            RGB_SPLIT => {
                let o = split_offset(fx, short);
                p[0] = [o[0], o[1], 0.0, 0.0];
                self.step(encoder, "fs_rgb_split", &input, None, size, frame, p)
            }
            GLITCH => {
                let tick = (fx.seconds() * fx.get("speed")).floor().max(0.0);
                p[0] = [
                    fx.get("intensity") / 100.0,
                    4.0 + fx.get("block_size") / 100.0 * 0.12 * short,
                    tick,
                    fx.get("color_shift") / 100.0 * 0.02 * short,
                ];
                self.step(encoder, "fs_glitch", &input, None, size, frame, p)
            }
            VHS => {
                let t = fx.seconds();
                let band = (t * 0.15 + (fx.seed % 1000) as f32 / 1000.0).rem_euclid(1.0);
                p[0] = [
                    fx.get("intensity") / 100.0,
                    fx.get("noise") / 100.0,
                    fx.get("jitter") / 100.0 * 0.006 * short,
                    fx.get("scanlines") / 100.0,
                ];
                p[1] = [
                    (fx.get("bleed") / 100.0 * 0.012 * short).round(),
                    (t * 30.0).floor(),
                    band * 1.2 - 0.1,
                    0.0,
                ];
                self.step(encoder, "fs_vhs", &input, None, size, frame, p)
            }
            PIXELATE => {
                p[0] = [pixel_block(fx.get("size"), short), 0.0, 0.0, 0.0];
                self.step(encoder, "fs_pixelate", &input, None, size, frame, p)
            }
            MIRROR => {
                p[0] = [fx.get("mode"), 0.0, 0.0, 0.0];
                self.step(encoder, "fs_mirror", &input, None, size, frame, p)
            }
            KALEIDOSCOPE => {
                p[0] = [
                    fx.get("segments").round().max(2.0),
                    fx.get("rotation").to_radians(),
                    100.0 / fx.get("zoom").max(1.0),
                    0.0,
                ];
                self.step(encoder, "fs_kaleidoscope", &input, None, size, frame, p)
            }
            GRAIN => {
                // Grain size is set at 1080p and scales with the frame, so a
                // small preview does not show grain four times coarser.
                let cell = (fx.get("size") * short / 1080.0).max(1.0);
                let ms = ((fx.time / 1000).rem_euclid(1 << 16)) as f32;
                p[0] = [fx.get("amount") / 100.0, cell, ms, 0.0];
                self.step(encoder, "fs_grain", &input, None, size, frame, p)
            }
            LETTERBOX => {
                let (bar_h, bar_w) = letterbox_bars(fx.get("aspect"), size);
                p[0] = [bar_h, bar_w, fx.get("opacity") / 100.0, 0.0];
                p[1] = fx.color("color");
                self.step(encoder, "fs_letterbox", &input, None, size, frame, p)
            }
            FRAME => {
                let (c, ax, ay) = clip_rect(fx.placement, size);
                let half = (ax[0].hypot(ax[1])).min(ay[0].hypot(ay[1]));
                let angle = fx.get("shadow_angle").to_radians();
                let distance = fx.get("shadow_distance") / 100.0 * 0.05 * short;
                let mut shadow = fx.color("shadow_color");
                shadow[3] *= fx.get("shadow") / 100.0;
                p[0] = [c[0], c[1], ax[0], ax[1]];
                p[1] = [
                    ay[0],
                    ay[1],
                    fx.get("radius") / 100.0 * half,
                    fx.get("border") / 100.0 * 0.03 * short,
                ];
                p[2] = fx.color("border_color");
                p[3] = shadow;
                p[4] = [
                    angle.cos() * distance,
                    angle.sin() * distance,
                    fx.get("shadow_blur") / 100.0 * 0.06 * short,
                    0.0,
                ];
                self.step(encoder, "fs_frame", &input, None, size, frame, p)
            }
            _ => return input,
        };
        self.retire(input);
        out
    }

    fn blur(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        input: Work,
        sigma: f32,
        frame: [f32; 4],
    ) -> Work {
        if sigma < 0.3 {
            return input;
        }
        let size = input.size();
        let (taps, stride) = blur_taps(sigma);
        let params = |dir: [f32; 2]| {
            let mut p = [[0.0f32; 4]; 6];
            p[0] = [dir[0], dir[1], sigma, stride as f32];
            p[1] = [taps as f32, 0.0, 0.0, 0.0];
            p
        };
        let across = self.step(
            encoder,
            "fs_blur",
            &input,
            None,
            size,
            frame,
            params([1.0, 0.0]),
        );
        self.retire(input);
        let down = self.step(
            encoder,
            "fs_blur",
            &across,
            None,
            size,
            frame,
            params([0.0, 1.0]),
        );
        self.retire(across);
        down
    }

    /// Glow, bloom and halation: the bright part, at half resolution, blurred
    /// and added back tinted.
    #[allow(clippy::too_many_arguments)]
    fn add_light(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        input: Work,
        bright: [f32; 4],
        sigma: f32,
        amount: f32,
        tint: [f32; 4],
        frame: [f32; 4],
    ) -> Work {
        let size = input.size();
        let half = (size.0.div_ceil(2), size.1.div_ceil(2));
        let mut p = [[0.0f32; 4]; 6];
        p[0] = bright;
        let small = self.step(encoder, "fs_bright", &input, None, half, frame, p);
        let blurred = self.blur(encoder, small, sigma * 0.5, frame);
        let mut p = [[0.0f32; 4]; 6];
        p[0] = [amount, 0.0, 0.0, 0.0];
        p[1] = [tint[0], tint[1], tint[2], 1.0];
        let out = self.step(
            encoder,
            "fs_add_light",
            &input,
            Some(&blurred),
            size,
            frame,
            p,
        );
        self.retire(blurred);
        self.retire(input);
        out
    }
}

fn srgb_to_linear(encoded: f32) -> f32 {
    let e = encoded.clamp(0.0, 1.0);
    if e <= 0.04045 {
        e / 12.92
    } else {
        ((e + 0.055) / 1.055).powf(2.4)
    }
}
