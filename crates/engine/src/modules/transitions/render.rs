//! The GPU side: five pipelines over one shader module.
//!
//! ## What this takes and what it does not
//!
//! It takes **two textures and a progress value**. It does not know how those
//! textures were produced, and deliberately so: turning a segment into pixels
//! at a canvas position is the compositor's whole job, and a transition that
//! reached into it would have to duplicate transforms, crops, keyframes and
//! source lookup. So the contract is a full-canvas layer each side, and this
//! module blends them.
//!
//! That means a frame containing a transition costs two extra render targets,
//! for the two layers. The alternative — blending the two clips inside the quad
//! pass, in quad space — was rejected because a wipe, a slide and a zoom are all
//! defined against the *frame*, not against the clip: a wipe across a clip that
//! has been scaled to a third of the canvas and rotated is not a wipe, and
//! nobody would recognise it as one.
//!
//! ## Two ways in
//!
//! - [`TransitionPipeline::draw`] records into a render pass the caller already
//!   opened. This is what the compositor uses, so the transition lands at its
//!   own position in the painter's order, in the same pass, with the same
//!   attachment and the same blend state as every other layer. Nothing about
//!   the compositor's single-pass structure has to change.
//! - [`TransitionPipeline::blend_to_texture`] opens its own pass and submits.
//!   For tests and for anything that wants the blended result on its own.

use parking_lot::Mutex;

use crate::modules::project::document::{TransitionDirection, TransitionKind, TransitionMaterial};
use crate::modules::render::RenderContext;

use super::resolve::TransitionInstant;

/// How many fragment entry points the shader has, and therefore how many
/// pipelines there are.
const KIND_COUNT: usize = 5;

/// Entry point per kind, in [`kind_index`] order.
const ENTRY_POINTS: [&str; KIND_COUNT] =
    ["fs_dissolve", "fs_dip", "fs_wipe", "fs_slide", "fs_zoom"];

fn kind_index(kind: TransitionKind) -> usize {
    match kind {
        TransitionKind::Dissolve => 0,
        TransitionKind::DipToColor => 1,
        TransitionKind::Wipe => 2,
        TransitionKind::Slide => 3,
        TransitionKind::Zoom => 4,
    }
}

/// Everything the shader needs that is not a texture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitionParams {
    pub kind: TransitionKind,
    /// Already eased, already clamped to `0..1`.
    pub progress: f32,
    pub direction: TransitionDirection,
    pub softness: f32,
    pub zoom: f32,
    pub color: [f32; 4],
}

impl TransitionParams {
    pub fn new(material: &TransitionMaterial, progress: f32) -> Self {
        Self {
            kind: material.kind,
            progress: progress.clamp(0.0, 1.0),
            direction: material.direction,
            softness: material.softness,
            zoom: material.zoom,
            color: material.color,
        }
    }
}

impl From<&TransitionInstant<'_>> for TransitionParams {
    fn from(instant: &TransitionInstant<'_>) -> Self {
        Self::new(instant.material, instant.progress)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TransitionUniform {
    progress: f32,
    direction: u32,
    softness: f32,
    zoom: f32,
    color: [f32; 4],
}

// `#[repr(C)]`, scalars then a `vec4`, no padding: plain old data. Written by
// hand rather than derived so this module does not depend on `bytemuck`'s
// `derive` feature being switched on somewhere else in the dependency graph —
// the same reasoning as `render/compositor.rs`.
unsafe impl bytemuck::Zeroable for TransitionUniform {}
unsafe impl bytemuck::Pod for TransitionUniform {}

impl From<&TransitionParams> for TransitionUniform {
    fn from(params: &TransitionParams) -> Self {
        Self {
            progress: params.progress.clamp(0.0, 1.0),
            direction: params.direction.shader_index(),
            softness: params.softness.max(0.0),
            zoom: params.zoom.max(0.0),
            color: params.color,
        }
    }
}

/// The pipelines, the sampler and the uniform scratch buffer.
///
/// Build one per render target format and keep it: it holds no per-frame state
/// beyond a growable uniform buffer, and `draw` takes `&self` so the exporter's
/// worker and the preview can share it exactly as they share the compositor.
pub struct TransitionPipeline {
    pipelines: Vec<wgpu::RenderPipeline>,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: Mutex<Scratch>,
    uniform_stride: u32,
    format: wgpu::TextureFormat,
}

#[derive(Default)]
struct Scratch {
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
}

impl Scratch {
    fn ensure(&mut self, device: &wgpu::Device, size: u64) -> &wgpu::Buffer {
        if self.capacity < size || self.buffer.is_none() {
            let size = size.next_power_of_two().max(256);
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chukcut transition uniforms"),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.capacity = size;
        }
        self.buffer.as_ref().expect("just ensured")
    }
}

impl TransitionPipeline {
    /// Build every pipeline for `format`.
    ///
    /// All five up front, from one shader module, because the alternative —
    /// compiling a kind the first time it is scrubbed over — puts a shader
    /// compile inside a playback frame, and a stutter on the first frame of a
    /// transition is exactly what a user reads as "transitions are slow".
    /// Whether to build *any* of it is the caller's lazy decision; a project
    /// with no transitions should never construct this.
    pub fn new(ctx: &RenderContext, format: wgpu::TextureFormat) -> Self {
        let device = ctx.device();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chukcut transition shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/transition.wgsl").into()),
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut transition uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<TransitionUniform>() as u64,
                    ),
                },
                count: None,
            }],
        });

        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
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
            label: Some("chukcut transition layers"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chukcut transition pipeline layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)],
            immediate_size: 0,
        });

        let pipelines = ENTRY_POINTS
            .iter()
            .map(|entry| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("chukcut transition pipeline"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        // No vertex buffer: the fullscreen triangle is built
                        // from `vertex_index` alone.
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
                        module: &shader,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            // Straight-alpha source-over, the same state the
                            // quad pipeline uses, so a transition composites
                            // onto the tracks beneath it like any other layer.
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .collect();

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("chukcut transition sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let align = ctx.limits().min_uniform_buffer_offset_alignment.max(1) as u64;
        let uniform_stride =
            (std::mem::size_of::<TransitionUniform>() as u64).div_ceil(align) * align;

        Self {
            pipelines,
            uniform_layout,
            texture_layout,
            sampler,
            uniforms: Mutex::new(Scratch::default()),
            uniform_stride: uniform_stride as u32,
            format,
        }
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Bind two layers for a draw. Cheap; wgpu bind groups are reference
    /// counted, so the caller may drop this as soon as the draw is recorded.
    pub fn bind_layers(
        &self,
        ctx: &RenderContext,
        from: &wgpu::TextureView,
        to: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        ctx.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut transition layers"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(from),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(to),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Record one transition into an open render pass.
    ///
    /// `slot` distinguishes the transitions of a single frame from one another.
    /// Their uniforms share one buffer addressed by dynamic offset — the same
    /// arrangement the quad pass uses for its per-draw blocks — so two
    /// transitions recorded into the same pass do not overwrite each other's
    /// parameters. Pass `0` when there is only one.
    pub fn draw(
        &self,
        ctx: &RenderContext,
        pass: &mut wgpu::RenderPass<'_>,
        slot: u32,
        params: &TransitionParams,
        layers: &wgpu::BindGroup,
    ) {
        let stride = self.uniform_stride as u64;
        let mut scratch = self.uniforms.lock();
        let buffer = scratch.ensure(ctx.device(), stride * (slot as u64 + 1));

        let block = TransitionUniform::from(params);
        ctx.queue()
            .write_buffer(buffer, slot as u64 * stride, bytemuck::bytes_of(&block));

        let uniform_group = ctx.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut transition uniform bind group"),
            layout: &self.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<TransitionUniform>() as u64),
                }),
            }],
        });
        drop(scratch);

        pass.set_pipeline(&self.pipelines[kind_index(params.kind)]);
        pass.set_bind_group(0, &uniform_group, &[slot * self.uniform_stride]);
        pass.set_bind_group(1, layers, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Blend two layers into `target` on their own, clearing it first.
    ///
    /// Self-contained: it opens an encoder, records one pass and submits. For
    /// tests, and for any caller that wants the blended frame as a texture of
    /// its own rather than composited into something larger.
    pub fn blend_to_texture(
        &self,
        ctx: &RenderContext,
        params: &TransitionParams,
        from: &wgpu::TextureView,
        to: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) {
        let layers = self.bind_layers(ctx, from, to);
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("chukcut transition blend"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("chukcut transition"),
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
            self.draw(ctx, &mut pass, 0, params, &layers);
        }
        ctx.queue().submit(Some(encoder.finish()));
    }
}

impl std::fmt::Debug for TransitionPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransitionPipeline")
            .field("format", &self.format)
            .field("pipelines", &self.pipelines.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::render::test_context;
    use std::sync::Arc;

    const SIZE: u32 = 8;
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// A flat-coloured texture to use as a layer. `Rgba8Unorm` rather than the
    /// compositor's sRGB format so the bytes that come back are the bytes that
    /// went in, and an assertion failure means the blend is wrong rather than
    /// that a colour space is.
    fn solid(ctx: &RenderContext, color: [u8; 4]) -> (Arc<wgpu::Texture>, wgpu::TextureView) {
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("transition test layer"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let pixels: Vec<u8> = std::iter::repeat_n(color, (SIZE * SIZE) as usize)
            .flatten()
            .collect();
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: Some(SIZE),
            },
            wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (Arc::new(texture), view)
    }

    fn target(ctx: &RenderContext) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("transition test target"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    /// Read the target back. `SIZE * 4` is 32 bytes a row, under the 256-byte
    /// copy alignment, so the buffer is padded and the rows are unpacked here.
    fn read(ctx: &RenderContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
        const PADDED: u32 = 256;
        let buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("transition test readback"),
            size: (PADDED * SIZE) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device().create_command_encoder(&Default::default());
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
                    bytes_per_row: Some(PADDED),
                    rows_per_image: Some(SIZE),
                },
            },
            wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue().submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();

        let view = slice.get_mapped_range().unwrap();
        let mut out = Vec::with_capacity((SIZE * SIZE) as usize);
        for row in 0..SIZE as usize {
            for column in 0..SIZE as usize {
                let at = row * PADDED as usize + column * 4;
                out.push([view[at], view[at + 1], view[at + 2], view[at + 3]]);
            }
        }
        drop(view);
        buffer.unmap();
        out
    }

    fn blend(
        ctx: &RenderContext,
        pipeline: &TransitionPipeline,
        params: TransitionParams,
    ) -> Vec<[u8; 4]> {
        let (_from_texture, from) = solid(ctx, [255, 0, 0, 255]);
        let (_to_texture, to) = solid(ctx, [0, 0, 255, 255]);
        let (texture, view) = target(ctx);
        pipeline.blend_to_texture(ctx, &params, &from, &to, &view);
        read(ctx, &texture)
    }

    fn params(kind: TransitionKind, progress: f32) -> TransitionParams {
        TransitionParams {
            kind,
            progress,
            direction: TransitionDirection::Right,
            softness: 0.0,
            zoom: 0.35,
            color: [0.0, 1.0, 0.0, 1.0],
        }
    }

    /// Skip rather than fail on a machine with no adapter, the way the rest of
    /// the GPU tests in this repository do.
    macro_rules! gpu {
        () => {
            match test_context() {
                Some(ctx) => ctx,
                None => return,
            }
        };
    }

    #[test]
    fn every_kind_builds_a_pipeline() {
        let ctx = gpu!();
        let pipeline = TransitionPipeline::new(&ctx, FORMAT);
        assert_eq!(pipeline.pipelines.len(), KIND_COUNT);
        assert_eq!(ENTRY_POINTS.len(), KIND_COUNT);
    }

    #[test]
    fn dissolve_is_the_outgoing_clip_at_zero_and_the_incoming_one_at_one() {
        let ctx = gpu!();
        let pipeline = TransitionPipeline::new(&ctx, FORMAT);

        let start = blend(&ctx, &pipeline, params(TransitionKind::Dissolve, 0.0));
        assert_eq!(start[0], [255, 0, 0, 255]);

        let end = blend(&ctx, &pipeline, params(TransitionKind::Dissolve, 1.0));
        assert_eq!(end[0], [0, 0, 255, 255]);

        let middle = blend(&ctx, &pipeline, params(TransitionKind::Dissolve, 0.5));
        assert!((middle[0][0] as i32 - 128).abs() <= 2, "{:?}", middle[0]);
        assert!((middle[0][2] as i32 - 128).abs() <= 2, "{:?}", middle[0]);
        assert_eq!(middle[0][3], 255);
    }

    #[test]
    fn dip_reaches_the_colour_in_the_middle_and_neither_clip_shows_through() {
        let ctx = gpu!();
        let pipeline = TransitionPipeline::new(&ctx, FORMAT);

        let middle = blend(&ctx, &pipeline, params(TransitionKind::DipToColor, 0.5));
        assert_eq!(middle[0], [0, 255, 0, 255]);

        // At the ends the colour is gone entirely.
        assert_eq!(
            blend(&ctx, &pipeline, params(TransitionKind::DipToColor, 0.0))[0],
            [255, 0, 0, 255]
        );
        assert_eq!(
            blend(&ctx, &pipeline, params(TransitionKind::DipToColor, 1.0))[0],
            [0, 0, 255, 255]
        );
    }

    #[test]
    fn a_wipe_moves_the_edge_across_the_frame() {
        let ctx = gpu!();
        let pipeline = TransitionPipeline::new(&ctx, FORMAT);

        // Halfway through a rightward wipe the left half is the incoming clip
        // and the right half is still the outgoing one.
        let half = blend(&ctx, &pipeline, params(TransitionKind::Wipe, 0.5));
        assert_eq!(half[0], [0, 0, 255, 255], "left edge should be incoming");
        assert_eq!(
            half[(SIZE - 1) as usize],
            [255, 0, 0, 255],
            "right edge should still be outgoing"
        );

        // And at the boundaries it covers the frame completely, either way.
        let start = blend(&ctx, &pipeline, params(TransitionKind::Wipe, 0.0));
        assert!(start.iter().all(|p| *p == [255, 0, 0, 255]));
        let end = blend(&ctx, &pipeline, params(TransitionKind::Wipe, 1.0));
        assert!(end.iter().all(|p| *p == [0, 0, 255, 255]));
    }

    #[test]
    fn a_wipe_goes_the_other_way_when_told_to() {
        let ctx = gpu!();
        let pipeline = TransitionPipeline::new(&ctx, FORMAT);
        let mut left = params(TransitionKind::Wipe, 0.5);
        left.direction = TransitionDirection::Left;

        let half = blend(&ctx, &pipeline, left);
        assert_eq!(
            half[0],
            [255, 0, 0, 255],
            "left edge should still be outgoing"
        );
        assert_eq!(half[(SIZE - 1) as usize], [0, 0, 255, 255]);
    }

    #[test]
    fn a_slide_pushes_one_layer_off_as_the_other_arrives() {
        let ctx = gpu!();
        let pipeline = TransitionPipeline::new(&ctx, FORMAT);

        // Pushing rightwards: the incoming clip enters from the left.
        let half = blend(&ctx, &pipeline, params(TransitionKind::Slide, 0.5));
        assert_eq!(half[0], [0, 0, 255, 255]);
        assert_eq!(half[(SIZE - 1) as usize], [255, 0, 0, 255]);

        // Nothing is ever transparent in the middle of a push — the two layers
        // between them always cover the frame.
        assert!(half.iter().all(|p| p[3] == 255), "a gap opened up");

        let start = blend(&ctx, &pipeline, params(TransitionKind::Slide, 0.0));
        assert!(start.iter().all(|p| *p == [255, 0, 0, 255]));
    }

    #[test]
    fn zoom_crossfades_and_keeps_the_frame_covered() {
        let ctx = gpu!();
        let pipeline = TransitionPipeline::new(&ctx, FORMAT);

        assert_eq!(
            blend(&ctx, &pipeline, params(TransitionKind::Zoom, 0.0))[0],
            [255, 0, 0, 255]
        );
        let middle = blend(&ctx, &pipeline, params(TransitionKind::Zoom, 0.5));
        // Scaling up samples inside a flat texture, so the colours are the flat
        // colours and only the mix matters.
        assert!((middle[0][0] as i32 - 128).abs() <= 2, "{:?}", middle[0]);
        assert!(middle.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn two_transitions_in_one_pass_keep_their_own_parameters() {
        // The dynamic-offset arrangement exists for exactly this: two
        // transitions recorded into one pass must not share a uniform block.
        // Drawn one over the other with source-over blending, the second wins
        // where it is opaque, so a wrong offset shows up as the second draw
        // taking the first one's progress.
        let ctx = gpu!();
        let pipeline = TransitionPipeline::new(&ctx, FORMAT);

        let (_a, from) = solid(&ctx, [255, 0, 0, 255]);
        let (_b, to) = solid(&ctx, [0, 0, 255, 255]);
        let (texture, view) = target(&ctx);
        let layers = pipeline.bind_layers(&ctx, &from, &to);

        let mut encoder = ctx.device().create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pipeline.draw(
                &ctx,
                &mut pass,
                0,
                &params(TransitionKind::Dissolve, 0.0),
                &layers,
            );
            pipeline.draw(
                &ctx,
                &mut pass,
                1,
                &params(TransitionKind::Dissolve, 1.0),
                &layers,
            );
        }
        ctx.queue().submit(Some(encoder.finish()));

        // The second draw is fully opaque incoming-clip blue, so it covers the
        // first. If both had read slot 0 the answer would be red.
        assert_eq!(read(&ctx, &texture)[0], [0, 0, 255, 255]);
    }
}
