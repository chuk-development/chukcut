//! Effects limited by a clip's matte: "blur only the background", "glow
//! only on the person".
//!
//! The effects run over the clip's whole layer, as always, so a blur of the
//! background sees the full picture and leaves no dark halo where the
//! subject was cut out of it. Then this pass mixes the layer before the
//! effects and the layer after them by the matte, drawn into a third layer
//! over the clip's quad (`quad.wgsl`, `M_MATTE_OUT`: the subject's weight
//! where the clip is, nothing elsewhere):
//!
//! ```text
//!   out = mix(before, after, w)    w = matte            (subject)
//!                                  w = 1 − matte        (background;
//!                                      1 outside the clip, so an effect's
//!                                      spill past the clip's edge stays)
//! ```
//!
//! The mix is of premultiplied colour, so an effect that changes coverage
//! (a glow beyond the subject's edge) fades with it instead of fringing,
//! and the result is straight alpha like every clip layer.

use super::context::RenderContext;

const SHADER: &str = r#"
@group(0) @binding(0) var before: texture_2d<f32>;
@group(0) @binding(1) var after: texture_2d<f32>;
@group(0) @binding(2) var weights: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn premultiplied(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn mixed(position: vec4<f32>, background: bool) -> vec4<f32> {
    let at = vec2<i32>(position.xy);
    let a = premultiplied(textureLoad(before, at, 0));
    let b = premultiplied(textureLoad(after, at, 0));
    // Colour times alpha: the matte where one draw wrote it (alpha 1), the
    // share-weighted sum where several were averaged (`render::accumulate`
    // leaves the mean in colour and the coverage in alpha), 0 where nothing
    // was drawn.
    let texel = textureLoad(weights, at, 0);
    var w = clamp(texel.r * texel.a, 0.0, 1.0);
    if (background) {
        w = 1.0 - w;
    }
    let c = mix(a, b, w);
    if (c.a <= 0.00001) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(c.rgb / c.a, min(c.a, 1.0));
}

@fragment
fn fs_subject(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return mixed(position, false);
}

@fragment
fn fs_background(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return mixed(position, true);
}
"#;

/// The pass that mixes a clip's layer with its effected layer by its matte.
pub struct MatteMix {
    subject: wgpu::RenderPipeline,
    background: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl MatteMix {
    pub fn new(ctx: &RenderContext, format: wgpu::TextureFormat) -> Self {
        let device = ctx.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chukcut matte mix"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut matte mix input"),
            entries: &[texture(0), texture(1), texture(2)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chukcut matte mix layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("chukcut matte mix"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            subject: pipeline("fs_subject"),
            background: pipeline("fs_background"),
            layout,
        }
    }

    /// Record `out = mix(before, after, w)` by `weights` (see the module
    /// docs); `background` uses `1 − matte`.
    #[allow(clippy::too_many_arguments)]
    pub fn mix(
        &self,
        ctx: &RenderContext,
        encoder: &mut wgpu::CommandEncoder,
        before: &wgpu::TextureView,
        after: &wgpu::TextureView,
        weights: &wgpu::TextureView,
        background: bool,
        out: &wgpu::TextureView,
    ) {
        let group = ctx.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut matte mix input"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(before),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(after),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(weights),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chukcut matte mix"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: out,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(if background {
            &self.background
        } else {
            &self.subject
        });
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
