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
    // Which stages of the extended grade are live, as `F_*` bits below. The
    // same flag discipline as `color_active`: a stage whose controls are at
    // rest is not run with identity values, it is not run.
    features: u32,
    // `2^exposure`, tint, highlights, shadows.
    tone: vec4<f32>,
    // Whites, blacks, vibrance, fade.
    tone2: vec4<f32>,
    // Sharpen, clarity, grain, grain seed.
    detail: vec4<f32>,
    // Vignette amount, midpoint, feather; w unused.
    vignette: vec4<f32>,
    // Colour wheels, already reduced to per-channel factors on the CPU
    // (`render::grade::wheel_factors`): additive lift, multiplicative gain,
    // gamma *exponent*, additive offset. w unused.
    lift: vec4<f32>,
    gain: vec4<f32>,
    gamma: vec4<f32>,
    offset: vec4<f32>,
    // Per HSL band (`project::grade::HSL_BANDS` order): hue, saturation,
    // luminance; w unused.
    hsl: array<vec4<f32>, 8>,
};

// The bits of `features`. `render::grade::feature` spells the same numbers
// and a Rust test reads this file to check they agree.
const F_EXPOSURE: u32 = 1u;
const F_TINT: u32 = 2u;
const F_TONE: u32 = 4u;
const F_WHEELS: u32 = 8u;
const F_HSL: u32 = 16u;
const F_VIBRANCE: u32 = 32u;
const F_CURVES: u32 = 64u;
const F_FADE: u32 = 128u;
const F_SHARPEN: u32 = 256u;
const F_CLARITY: u32 = 512u;
const F_VIGNETTE: u32 = 1024u;
const F_GRAIN: u32 = 2048u;
const F_LUT_1D: u32 = 4096u;
// The stages that run in encoded space, between `linear_to_srgb` and its
// inverse. Any of them (or the original grade, or a LUT) opens that bracket.
const F_ENCODED: u32 = 766u;

// Strengths, mirrored in `render::grade` (and checked there).
const TINT_STRENGTH: f32 = 0.2;
const LEVELS_STRENGTH: f32 = 0.25;
const TONE_STRENGTH: f32 = 0.3;
const HSL_HUE_DEGREES: f32 = 30.0;
const HSL_LUMA_STRENGTH: f32 = 0.5;
const FADE_LIFT: f32 = 0.25;
const FADE_COMPRESS: f32 = 0.35;
const SHARPEN_STRENGTH: f32 = 2.0;
const CLARITY_STRENGTH: f32 = 1.5;
const CLARITY_RADIUS: f32 = 0.012;
const GRAIN_STRENGTH: f32 = 0.25;
const CURVE_SIZE: u32 = 1024u;
const LUMA601: vec3<f32> = vec3<f32>(0.299, 0.587, 0.114);

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
// The clip's tone curves, `CURVE_SIZE` x 1 `Rgba32Float`: red, green, blue in
// rgb and master in a (`render::grade::bake_curves`). A 1x1 placeholder unless
// `F_CURVES` is set.
@group(1) @binding(4) var curve_texture: texture_2d<f32>;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Position on the quad as displayed, 0..1 each way: where the vignette
    // is measured from. Not `uv`, which is cropped and turned into the
    // texture's stored orientation.
    @location(1) local: vec2<f32>,
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

// One channel of a 1D LUT, folded into rows of the 3D texture's first slice
// (`render::lut::upload`). Linear between entries, by hand for the same
// exactness reason `sample_lut` gives.
fn lut_1d_channel(x: f32, ch: u32) -> f32 {
    let n = u32(quad.lut_hi.w);
    let p = clamp((x - quad.lut_lo[ch]) / (quad.lut_hi[ch] - quad.lut_lo[ch]), 0.0, 1.0)
        * f32(n - 1u);
    let i = min(u32(floor(p)), n - 2u);
    let t = p - f32(i);
    let row = textureDimensions(lut_texture).x;
    let j = i + 1u;
    let a = textureLoad(lut_texture, vec3<i32>(i32(i % row), i32(i / row), 0), 0)[ch];
    let b = textureLoad(lut_texture, vec3<i32>(i32(j % row), i32(j / row), 0), 0)[ch];
    return mix(a, b, t);
}

fn sample_lut_1d(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(lut_1d_channel(c.r, 0u), lut_1d_channel(c.g, 1u), lut_1d_channel(c.b, 2u));
}

// One lookup in the baked curve row, linear between entries.
fn curve_at(x: f32, ch: u32) -> f32 {
    let p = clamp(x, 0.0, 1.0) * f32(CURVE_SIZE - 1u);
    let i = min(u32(floor(p)), CURVE_SIZE - 2u);
    let t = p - f32(i);
    let a = textureLoad(curve_texture, vec2<i32>(i32(i), 0), 0)[ch];
    let b = textureLoad(curve_texture, vec2<i32>(i32(i) + 1, 0), 0)[ch];
    return mix(a, b, t);
}

// The source in linear light at any texture coordinate, either decode path.
// `textureSampleLevel` rather than `textureSample`: the neighbourhood stages
// call this from branches, where implicit derivatives are not allowed.
fn source_linear(uv: vec2<f32>) -> vec3<f32> {
    if (quad.planar == 1u) {
        let y = textureSampleLevel(source_texture, source_sampler, uv, 0.0).r;
        let cbcr = textureSampleLevel(chroma_texture, source_sampler, uv, 0.0).rg;
        return srgb_to_linear(yuv_to_rgb(y, cbcr, quad.matrix, quad.range));
    }
    return textureSampleLevel(source_texture, source_sampler, uv, 0.0).rgb;
}

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let d = mx - mn;
    var h = 0.0;
    if (d > 0.0) {
        if (mx == c.r) {
            h = (c.g - c.b) / d;
            if (h < 0.0) {
                h = h + 6.0;
            }
        } else if (mx == c.g) {
            h = (c.b - c.r) / d + 2.0;
        } else {
            h = (c.r - c.g) / d + 4.0;
        }
        h = h / 6.0;
    }
    var s = 0.0;
    if (mx > 0.0) {
        s = d / mx;
    }
    return vec3<f32>(h, s, mx);
}

fn hsv_to_rgb(hsv: vec3<f32>) -> vec3<f32> {
    let h6 = fract(hsv.x) * 6.0;
    let k = vec3<f32>(
        clamp(abs(h6 - 3.0) - 1.0, 0.0, 1.0),
        clamp(2.0 - abs(h6 - 2.0), 0.0, 1.0),
        clamp(2.0 - abs(h6 - 4.0), 0.0, 1.0),
    );
    return hsv.z * mix(vec3<f32>(1.0), k, hsv.y);
}

// Per-hue adjustment: the pixel's hue falls between two band centres and
// takes their settings blended by distance, so the bands partition the
// circle with no seams. Saturation and luminance changes scale with the
// pixel's own saturation — a grey has no hue and is left alone.
fn hsl_adjust(c: vec3<f32>) -> vec3<f32> {
    var centres = array<f32, 9>(0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0, 360.0);
    let hsv = rgb_to_hsv(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
    let degrees = fract(hsv.x) * 360.0;
    var a = 0u;
    var t = 0.0;
    for (var i = 0u; i < 8u; i = i + 1u) {
        if (degrees >= centres[i] && degrees < centres[i + 1u]) {
            a = i;
            t = (degrees - centres[i]) / (centres[i + 1u] - centres[i]);
        }
    }
    let adj = mix(quad.hsl[a].xyz, quad.hsl[(a + 1u) % 8u].xyz, t);
    let h = hsv.x + adj.x * HSL_HUE_DEGREES / 360.0;
    let s = clamp(hsv.y * (1.0 + adj.y), 0.0, 1.0);
    let v = hsv.z * (1.0 + adj.z * HSL_LUMA_STRENGTH * hsv.y);
    return hsv_to_rgb(vec3<f32>(h, s, v));
}

// A 32-bit integer hash (PCG), for grain that is identical on every GPU and
// every run — the preview and the export must draw the same grain.
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
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
    out.local = in.uv;
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
    // layer and an ordinary quad get it the same way. Every stage is behind a
    // flag rather than run with identity values — see `color_active` on the
    // uniform. The order is documented in `render/grade.rs`; the numbered
    // comments below are its steps.
    var rgb = texel.rgb;
    var clarity_detail = 0.0;
    if ((quad.features & (F_SHARPEN | F_CLARITY)) != 0u) {
        let texel_size = 1.0 / vec2<f32>(textureDimensions(source_texture));
        // 1. Sharpen: unsharp mask over the four direct neighbours, in light.
        if ((quad.features & F_SHARPEN) != 0u) {
            let blur = (source_linear(in.uv + vec2<f32>(texel_size.x, 0.0))
                + source_linear(in.uv - vec2<f32>(texel_size.x, 0.0))
                + source_linear(in.uv + vec2<f32>(0.0, texel_size.y))
                + source_linear(in.uv - vec2<f32>(0.0, texel_size.y))) * 0.25;
            rgb = max(rgb + (rgb - blur) * quad.detail.x * SHARPEN_STRENGTH, vec3<f32>(0.0));
        }
        // 2. Clarity, measured: encoded luma against a ring of twelve taps,
        // alternating between the full radius and half of it.
        if ((quad.features & F_CLARITY) != 0u) {
            let dims = vec2<f32>(textureDimensions(source_texture));
            let radius = CLARITY_RADIUS * min(dims.x, dims.y) * texel_size;
            var sum = 0.0;
            for (var i = 0u; i < 12u; i = i + 1u) {
                let angle = f32(i) * 0.5235988;
                let r = select(radius, radius * 0.5, (i & 1u) == 1u);
                let tap = source_linear(in.uv + vec2<f32>(cos(angle), sin(angle)) * r);
                sum = sum + dot(linear_to_srgb(tap), LUMA601);
            }
            clarity_detail = dot(linear_to_srgb(texel.rgb), LUMA601) - sum / 12.0;
        }
    }
    // 3. Exposure, in light.
    if ((quad.features & F_EXPOSURE) != 0u) {
        rgb = rgb * quad.tone.x;
    }
    if (quad.color_active == 1u || quad.lut_active == 1u || (quad.features & F_ENCODED) != 0u) {
        var c = linear_to_srgb(rgb);
        // 4. Clarity, applied: the measured detail, weighted to midtones.
        if ((quad.features & F_CLARITY) != 0u) {
            let l = dot(c, LUMA601);
            c = c + vec3<f32>(quad.detail.y * CLARITY_STRENGTH * clarity_detail * 4.0 * l * (1.0 - l));
        }
        // 5. Tint: magenta up is green down.
        if ((quad.features & F_TINT) != 0u) {
            c = c + vec3<f32>(0.5, -1.0, 0.5) * quad.tone.y * TINT_STRENGTH;
        }
        // 6. The original four sliders.
        if (quad.color_active == 1u) {
            c = grade(c, quad.color);
        }
        if ((quad.features & F_TONE) != 0u) {
            // 7. Whites and blacks: a levels remap of the two end points.
            let white = 1.0 - LEVELS_STRENGTH * quad.tone2.x;
            let black = -LEVELS_STRENGTH * quad.tone2.y;
            c = (c - vec3<f32>(black)) / (white - black);
            // 8. Highlights and shadows: lifts masked by luma.
            let l = dot(c, LUMA601);
            let ws = 1.0 - smoothstep(0.0, 0.6, l);
            let wh = smoothstep(0.4, 1.0, l);
            c = c + vec3<f32>(TONE_STRENGTH * (quad.tone.w * ws + quad.tone.z * wh));
        }
        // 9. Colour wheels.
        if ((quad.features & F_WHEELS) != 0u) {
            c = c + quad.lift.rgb * (vec3<f32>(1.0) - c);
            c = c * quad.gain.rgb;
            c = pow(max(c, vec3<f32>(0.0)), quad.gamma.rgb);
            c = c + quad.offset.rgb;
        }
        // 10. HSL.
        if ((quad.features & F_HSL) != 0u) {
            c = hsl_adjust(c);
        }
        // 11. Vibrance: saturation scaled by how unsaturated the pixel is.
        if ((quad.features & F_VIBRANCE) != 0u) {
            let sat = clamp(max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b)), 0.0, 1.0);
            let grey = dot(c, LUMA601);
            c = vec3<f32>(grey) + (c - vec3<f32>(grey)) * (1.0 + quad.tone2.z * (1.0 - sat));
        }
        // 12. Curves: master first, then each channel's own.
        if ((quad.features & F_CURVES) != 0u) {
            c = vec3<f32>(
                curve_at(curve_at(c.r, 3u), 0u),
                curve_at(curve_at(c.g, 3u), 1u),
                curve_at(curve_at(c.b, 3u), 2u),
            );
        }
        // 13. The look. Grade first, look second: the LUT sees the corrected
        // image, which is how looks are authored.
        if (quad.lut_active == 1u) {
            // The samplers clamp into the LUT's own domain, so a grade
            // overshoot is handled there rather than pre-clamped here.
            var looked: vec3<f32>;
            if ((quad.features & F_LUT_1D) != 0u) {
                looked = sample_lut_1d(c);
            } else {
                looked = sample_lut(c);
            }
            // Intensity is a plain lerp between the (graded) input and the
            // LUT's answer, still in encoded space.
            c = mix(c, looked, quad.lut_lo.w);
        }
        // 14. Fade.
        if ((quad.features & F_FADE) != 0u) {
            c = c * (1.0 - FADE_COMPRESS * quad.tone2.w) + vec3<f32>(FADE_LIFT * quad.tone2.w);
        }
        rgb = srgb_to_linear(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    // 15. Vignette, in light: positive takes light away towards the edges,
    // negative adds it.
    if ((quad.features & F_VIGNETTE) != 0u) {
        let d = length((in.local - vec2<f32>(0.5)) * 2.0) / sqrt(2.0);
        let half_width = max(quad.vignette.z, 0.001) * 0.5;
        let mask = smoothstep(quad.vignette.y - half_width, quad.vignette.y + half_width, d);
        let amount = quad.vignette.x;
        if (amount > 0.0) {
            rgb = rgb * (1.0 - amount * mask);
        } else {
            rgb = rgb + max(vec3<f32>(1.0) - rgb, vec3<f32>(0.0)) * (-amount * mask);
        }
    }
    // 16. Grain: triangular noise per output pixel, reseeded per frame.
    if ((quad.features & F_GRAIN) != 0u) {
        let p = vec2<u32>(in.clip_position.xy);
        let h1 = pcg(p.x + pcg(p.y + pcg(u32(quad.detail.w))));
        let h2 = pcg(h1);
        let n = f32(h1 & 65535u) / 65535.0 + f32(h2 & 65535u) / 65535.0 - 1.0;
        rgb = max(rgb * (1.0 + n * quad.detail.z * GRAIN_STRENGTH), vec3<f32>(0.0));
    }
    texel = vec4<f32>(rgb, texel.a);
    // Straight alpha: opacity scales coverage, not colour. The pipeline's
    // blend state does the source-over multiply.
    return vec4<f32>(texel.rgb, texel.a * quad.opacity);
}
