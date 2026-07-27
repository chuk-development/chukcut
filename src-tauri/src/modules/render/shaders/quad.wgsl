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
    // Colour adjustments as (brightness, contrast, saturation, temperature).
    // Read only when `color_active` is 1; see `apply_color` below.
    color: vec4<f32>,
    // The LUT's DOMAIN_MIN in xyz and its intensity (0..1 blend) in w.
    // Read only when `lut_active` is 1.
    lut_lo: vec4<f32>,
    // The LUT's DOMAIN_MAX in xyz and its edge size N (as a float) in w.
    lut_hi: vec4<f32>,
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
    // 1 when `color` holds a non-identity adjustment. A flag rather than
    // trusting identity arithmetic to be exact: the adjustment round-trips
    // through the sRGB transfer function, and a clip nobody graded must render
    // byte-identical to a build without the feature. The compositor only sets
    // this for a material whose *scalars* are non-identity — a clip with only
    // a LUT does not pay for the grade arithmetic.
    color_active: u32,
    // 1 when `lut_texture` holds this clip's LUT. Gated the same way and for
    // the same reason as `color_active` — and it is also what makes "LUT file
    // missing" render the clip untouched rather than through a placeholder.
    lut_active: u32,
    _pad0: u32,
};

@group(0) @binding(0) var<uniform> quad: QuadUniform;

@group(1) @binding(0) var source_texture: texture_2d<f32>;
@group(1) @binding(1) var source_sampler: sampler;
// Always bound. On the RGBA path it is a 1x1 placeholder that nothing samples —
// WebGPU has no optional bindings, and one pipeline with a dead texture is
// cheaper than two pipelines and a second bind group layout.
@group(1) @binding(2) var chroma_texture: texture_2d<f32>;
// The clip's 3D LUT, `Rgba32Float`, read with `textureLoad` only — see
// `sample_lut` for why filtering is done by hand. A 1x1x1 placeholder when
// `lut_active` is 0, for the same no-optional-bindings reason as the chroma
// plane above.
@group(1) @binding(3) var lut_texture: texture_3d<f32>;

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

// How far temperature 1.0 pushes red up and blue down, in encoded units.
//
// Arbitrary in the way every editor's temperature slider is arbitrary — there
// is no standard for what "+1 warm" means — chosen so the extremes are clearly
// wrong-looking without clipping a mid-grey image. What is not arbitrary is
// that it is a constant here and nowhere else: the Rust test reference
// (`compositor.rs::tests::adjusted`) mirrors this exact pipeline, and the
// preview and the export agree because they run this one shader.
const TEMPERATURE_STRENGTH: f32 = 0.2;

// The scalar colour adjustments, in gamma-encoded space, encoded in and
// encoded out.
//
// The sliders are defined on the *encoded* image, because that is what
// brightness and contrast mean in every editor: contrast pivots on encoded
// mid grey, brightness offsets encoded values. Applying the same numbers in
// linear light looks wrong in a way users cannot name. The caller owns the
// encode/decode bracket so that a LUT can chain after this in the same
// encoded space without a wasted round trip.
//
// Order: temperature, saturation, contrast, brightness.
fn grade(encoded: vec3<f32>, adjust: vec4<f32>) -> vec3<f32> {
    // Temperature: opposing red/blue shift, green untouched.
    var c = encoded + vec3<f32>(adjust.w, 0.0, -adjust.w) * TEMPERATURE_STRENGTH;
    // Saturation: mix against the BT.601 luma grey. The 601 weights rather
    // than 709 because this operates on encoded values where neither is
    // "correct", and 601 is what the desaturate everyone is used to uses.
    let grey = dot(c, vec3<f32>(0.299, 0.587, 0.114));
    c = mix(vec3<f32>(grey), c, adjust.z);
    // Contrast: scale the distance from encoded mid grey.
    c = (c - vec3<f32>(0.5)) * adjust.y + vec3<f32>(0.5);
    // Brightness: plain offset.
    return c + vec3<f32>(adjust.x);
}

// One trilinear lookup in the clip's 3D LUT, in the same encoded space as
// `grade` — a .cube is authored against the display image.
//
// Filtered *by hand* from eight `textureLoad`s rather than by the sampler,
// because hardware trilinear quantises its interpolation weights (8-bit
// subtexel precision is typical) and that puts up to a code value of error
// into every lookup — including an identity LUT, which must render
// byte-identically to no LUT at all. `textureLoad` is also legal in the
// non-uniform control flow this is called from, where `textureSample`'s
// implicit derivatives are not.
fn sample_lut(encoded: vec3<f32>) -> vec3<f32> {
    let n = quad.lut_hi.w;
    let coord = clamp(
        (encoded - quad.lut_lo.xyz) / (quad.lut_hi.xyz - quad.lut_lo.xyz),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    ) * (n - 1.0);
    // An input of exactly 1.0 floors to the last texel; clamping the base to
    // n-2 with t = 1.0 reads it through the lerp instead of out of bounds.
    let base = min(vec3<u32>(floor(coord)), vec3<u32>(u32(n) - 2u));
    let t = coord - vec3<f32>(base);
    let i = vec3<i32>(base);

    let c000 = textureLoad(lut_texture, i, 0).rgb;
    let c100 = textureLoad(lut_texture, i + vec3<i32>(1, 0, 0), 0).rgb;
    let c010 = textureLoad(lut_texture, i + vec3<i32>(0, 1, 0), 0).rgb;
    let c110 = textureLoad(lut_texture, i + vec3<i32>(1, 1, 0), 0).rgb;
    let c001 = textureLoad(lut_texture, i + vec3<i32>(0, 0, 1), 0).rgb;
    let c101 = textureLoad(lut_texture, i + vec3<i32>(1, 0, 1), 0).rgb;
    let c011 = textureLoad(lut_texture, i + vec3<i32>(0, 1, 1), 0).rgb;
    let c111 = textureLoad(lut_texture, i + vec3<i32>(1, 1, 1), 0).rgb;

    let c00 = mix(c000, c100, t.x);
    let c10 = mix(c010, c110, t.x);
    let c01 = mix(c001, c101, t.x);
    let c11 = mix(c011, c111, t.x);
    return mix(mix(c00, c10, t.y), mix(c01, c11, t.y), t.z);
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
    // Per-clip colour, after decode and before compositing, so a transition
    // layer and an ordinary quad get it the same way. Behind flags rather
    // than run with identity values — see `color_active` on the uniform.
    // Grade first, look second: the LUT sees the corrected image, which is
    // how looks are authored (correct, then grade with the look on top).
    if (quad.color_active == 1u || quad.lut_active == 1u) {
        var c = linear_to_srgb(texel.rgb);
        if (quad.color_active == 1u) {
            c = grade(c, quad.color);
        }
        if (quad.lut_active == 1u) {
            // `sample_lut` clamps into the LUT's own domain, so a grade
            // overshoot is handled there rather than pre-clamped here.
            let looked = sample_lut(c);
            // Intensity is a plain lerp between the (graded) input and the
            // LUT's answer, still in encoded space.
            c = mix(c, looked, quad.lut_lo.w);
        }
        texel = vec4<f32>(srgb_to_linear(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))), texel.a);
    }
    // Straight alpha: opacity scales coverage, not colour. The pipeline's
    // blend state does the source-over multiply.
    return vec4<f32>(texel.rgb, texel.a * quad.opacity);
}
