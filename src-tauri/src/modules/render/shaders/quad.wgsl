// The only shader the compositor has: one textured, cropped, faded quad.
//
// Everything geometric arrives pre-baked in `mvp`, built on the CPU by
// `layout::place_quad`. The shader does not know about canvas size, aspect
// ratio, normalized positions or degrees — putting that maths in one testable
// Rust function rather than in WGSL is what makes it possible to assert on it
// without a GPU.
//
// Two things it does know about, and both exist because of hardware decode:
//
// - **A source may be two textures rather than one.** A frame decoded on the
//   GPU is an NV12 surface — an `R8` luma plane and a half-size `GR88` chroma
//   plane — imported straight from the decoder's memory. Sampling and
//   converting them here is what makes that import worth doing; the
//   alternative is a colour-conversion pass on the CPU, which costs more than
//   the decode did. `docs/research/hardware-decode.md` has the numbers.
// - **A source may be sideways.** Container rotation is normally applied by the
//   decoder, which means touching every pixel. On the mapped path nothing
//   touches pixels, so the angle arrives here and is folded into the texture
//   coordinate — free, because the lookup was happening anyway.
//
// `yuv.wgsl` is prepended to this file and supplies `yuv_to_rgb` and
// `srgb_to_linear`.

struct QuadUniform {
    // Unit quad (+/-0.5) to clip space.
    mvp: mat4x4<f32>,
    // Source rectangle in UV space as (u0, v0, u1, v1). The whole texture is
    // (0, 0, 1, 1); a crop narrows it. Expressed in *display* orientation, so a
    // crop means the same thing whichever way the file is rotated.
    crop: vec4<f32>,
    // Straight-alpha multiplier for the segment.
    opacity: f32,
    // 0: `source_texture` is RGBA and is sampled directly.
    // 1: `source_texture` is luma and `chroma_texture` is interleaved CbCr.
    planar: u32,
    // `render::source::YuvMatrix` and `YuvRange`, as their discriminants.
    // Ignored when `planar` is 0.
    matrix: u32,
    range: u32,
    // Quarter turns clockwise to apply to the texture coordinate: 0, 1, 2, 3.
    turns: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> quad: QuadUniform;

@group(1) @binding(0) var source_texture: texture_2d<f32>;
@group(1) @binding(1) var source_sampler: sampler;
// Always bound. On the RGBA path it is a 1x1 placeholder that nothing samples —
// WebGPU has no optional bindings, and one pipeline with a dead texture is
// cheaper than two pipelines and a second bind group layout.
@group(1) @binding(2) var chroma_texture: texture_2d<f32>;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

/// Display-space UV to the stored texture's UV, for a clockwise display
/// rotation of `turns` quarter turns.
///
/// The mapping is the same one `decoder::rotate_rgba` performs on bytes, read
/// backwards: that function fills destination pixel (x, y) from a source pixel,
/// and this returns exactly that source pixel's coordinate. They are two
/// spellings of one convention and they have to agree, or a rotated clip is
/// upright on the software path and mirrored on the hardware one.
fn turn_uv(uv: vec2<f32>, turns: u32) -> vec2<f32> {
    if (turns == 1u) {
        return vec2<f32>(uv.y, 1.0 - uv.x);
    }
    if (turns == 2u) {
        return vec2<f32>(1.0 - uv.x, 1.0 - uv.y);
    }
    if (turns == 3u) {
        return vec2<f32>(1.0 - uv.y, uv.x);
    }
    return uv;
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = quad.mvp * vec4<f32>(in.position, 0.0, 1.0);
    // The quad's own UVs run 0..1; remap them into the crop rectangle so the
    // kept region of the source stretches across the whole quad, then turn the
    // result into the orientation the texture is actually stored in.
    let cropped = quad.crop.xy + in.uv * (quad.crop.zw - quad.crop.xy);
    out.uv = turn_uv(cropped, quad.turns);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var texel: vec4<f32>;
    if (quad.planar == 1u) {
        // Two samples, both filtered, both from memory the decoder wrote and
        // nothing copied. The chroma texture is half size in each axis, so the
        // same UV addresses the matching sample without any scaling here.
        let y = textureSample(source_texture, source_sampler, in.uv).r;
        // The luma coordinate, unshifted, on a half-size texture: bilinear
        // filtering then places the chroma sample at the centre of each 2x2
        // luma block.
        //
        // That is *not* where 4:2:0 chroma formally lives — H.264 sites it on
        // the left column of the pair — and correcting for it was tried: adding
        // a quarter of a chroma texel to `u` made the agreement with the
        // software path worse, 1.33 against 1.28 mean channel difference, and
        // pushed the worst-case pixel from 177 to 255. swscale evidently
        // interpolates the same centred way we do, and matching the reference
        // implementation matters more here than matching the specification.
        let cbcr = textureSample(chroma_texture, source_sampler, in.uv).rg;
        let rgb = yuv_to_rgb(y, cbcr, quad.matrix, quad.range);
        // The RGBA path's texture is `Rgba8UnormSrgb`, so its sampler
        // linearises before the blend. This path's planes are `R8Unorm` and
        // `Rg8Unorm`, which linearise nothing, so it is done here — otherwise
        // the two paths composite the same frame at two brightnesses.
        texel = vec4<f32>(srgb_to_linear(rgb), 1.0);
    } else {
        texel = textureSample(source_texture, source_sampler, in.uv);
    }
    // Straight alpha: opacity scales coverage, not colour. The pipeline's
    // blend state does the source-over multiply.
    return vec4<f32>(texel.rgb, texel.a * quad.opacity);
}
