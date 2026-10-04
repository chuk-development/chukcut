//! Averaging several draws of one clip: frame blending and motion blur.
//!
//! Both make one picture out of several — two source frames weighted by
//! where between them the clip is (`speed::blend`), or one frame placed at
//! several instants of its movement (`fx::motion_blur`). The compositor draws
//! every one of them with the quad pipeline's premultiplied fragment into an
//! `Rgba16Float` layer with additive blending, each with its opacity scaled
//! by its weight, so the layer holds the premultiplied weighted mean.
//! [`Resolver`] then turns that into the straight-alpha layer, in the
//! compositor's own format, that every other clip layer is — so the clip's
//! effects, its blend mode and the "over" pass treat it like any clip.
//!
//! The sum is taken in 16-bit float on purpose: summing eight draws at 1/8
//! each in the 8-bit target would round every addend to 1/255 and step the
//! result by up to four code values.

use super::context::RenderContext;

/// The float format the draws are summed in.
pub const ACCUMULATION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Additive, both channels: the fragment is already premultiplied and
/// weighted, so the sum is the mean.
pub const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

const SHADER: &str = r#"
@group(0) @binding(0) var sum: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle that covers the target.
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// Premultiplied mean in, straight alpha out: what the layer pipeline writes.
@fragment
fn fs_resolve(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(sum, vec2<i32>(position.xy), 0);
    if (c.a <= 0.00001) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(c.rgb / c.a, min(c.a, 1.0));
}
"#;

/// The pass that turns the summed layer into an ordinary clip layer.
pub struct Resolver {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl Resolver {
    pub fn new(ctx: &RenderContext, format: wgpu::TextureFormat) -> Self {
        let device = ctx.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chukcut accumulate resolve"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut accumulate input"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chukcut accumulate resolve layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chukcut accumulate resolve"),
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
                entry_point: Some("fs_resolve"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, layout }
    }

    /// Record the resolve of `sum` into `out` (cleared first).
    pub fn resolve(
        &self,
        ctx: &RenderContext,
        encoder: &mut wgpu::CommandEncoder,
        sum: &wgpu::TextureView,
        out: &wgpu::TextureView,
    ) {
        let group = ctx.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut accumulate input"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(sum),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chukcut accumulate resolve"),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
