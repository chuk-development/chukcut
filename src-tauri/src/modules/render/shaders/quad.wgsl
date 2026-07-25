// The only shader the compositor has: one textured, cropped, faded quad.
//
// Everything geometric arrives pre-baked in `mvp`, built on the CPU by
// `layout::place_quad`. The shader does not know about canvas size, aspect
// ratio, normalized positions or degrees — putting that maths in one testable
// Rust function rather than in WGSL is what makes it possible to assert on it
// without a GPU.

struct QuadUniform {
    // Unit quad (+/-0.5) to clip space.
    mvp: mat4x4<f32>,
    // Source rectangle in UV space as (u0, v0, u1, v1). The whole texture is
    // (0, 0, 1, 1); a crop narrows it.
    crop: vec4<f32>,
    // Straight-alpha multiplier for the segment.
    opacity: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> quad: QuadUniform;

@group(1) @binding(0) var source_texture: texture_2d<f32>;
@group(1) @binding(1) var source_sampler: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = quad.mvp * vec4<f32>(in.position, 0.0, 1.0);
    // The quad's own UVs run 0..1; remap them into the crop rectangle so the
    // kept region of the source stretches across the whole quad.
    out.uv = quad.crop.xy + in.uv * (quad.crop.zw - quad.crop.xy);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(source_texture, source_sampler, in.uv);
    // Straight alpha: opacity scales coverage, not colour. The pipeline's
    // blend state does the source-over multiply.
    return vec4<f32>(texel.rgb, texel.a * quad.opacity);
}
