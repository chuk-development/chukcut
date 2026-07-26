// The colour conversions, both directions, in one place.
//
// This file is not a shader module of its own: it has no entry point and no
// bindings. It is prepended to `nv12.wgsl` (which needs the forward direction,
// RGB to YUV, for the encoder) and to `quad.wgsl` (which needs the inverse,
// because a hardware-decoded frame arrives as two NV12 planes and the
// compositor is the first thing that can turn them back into colour).
//
// They live together on purpose. A forward and an inverse that disagree about a
// matrix or a range is the classic way an editor develops a slight, consistent
// saturation shift between what it shows and what it writes, and the two are
// only ever noticed side by side.
//
// ## Ranges
//
// "Limited" (`tv`, MPEG) puts luma in 16..235 and chroma in 16..240; "full"
// (`pc`, JPEG) uses 0..255 for both. Every YUV routine in FFmpeg defaults to
// limited and JPEG is always full, which is the trap `vaapi-jpeg-preview.md`
// records: a full-range picture read as limited has grey blacks.
//
// ## Matrices
//
// Only the luma weights differ between BT.601, BT.709 and BT.2020, so the
// matrices are derived from `Kr`/`Kb` rather than written out. Deriving them is
// both shorter and harder to typo than four hand-copied constants each.

// ---------------------------------------------------------------------------
// Forward: RGB to YUV. The encoder's direction.
// ---------------------------------------------------------------------------

// BT.601, limited range — what swscale produces by default for an untagged RGB
// source, and what `encoder.rs` tags the stream as with `color_range: MPEG`.
// The coefficients are the ones in the standard; the 219/224 scales are the
// limited-range excursions.
fn luma(c: vec3<f32>) -> f32 {
    return 16.0 + 219.0 * (0.299 * c.r + 0.587 * c.g + 0.114 * c.b);
}

fn chroma(c: vec3<f32>) -> vec2<f32> {
    let cb = -0.168736 * c.r - 0.331264 * c.g + 0.5 * c.b;
    let cr = 0.5 * c.r - 0.418688 * c.g - 0.081312 * c.b;
    return vec2<f32>(128.0 + 224.0 * cb, 128.0 + 224.0 * cr);
}

// ---------------------------------------------------------------------------
// Inverse: YUV to RGB. The decoder's direction.
// ---------------------------------------------------------------------------

// Matrix selectors. These are the numbers `render::source::YuvMatrix` casts to;
// changing one without the other silently reinterprets every hardware-decoded
// frame, so they are asserted equal in a Rust test rather than trusted.
const MATRIX_BT601: u32 = 0u;
const MATRIX_BT709: u32 = 1u;
const MATRIX_BT2020: u32 = 2u;

const RANGE_LIMITED: u32 = 0u;
const RANGE_FULL: u32 = 1u;

/// Luma weights `(Kr, Kg, Kb)` for a matrix selector.
fn luma_weights(matrix: u32) -> vec3<f32> {
    if (matrix == MATRIX_BT601) {
        return vec3<f32>(0.299, 0.587, 0.114);
    }
    if (matrix == MATRIX_BT2020) {
        return vec3<f32>(0.2627, 0.6780, 0.0593);
    }
    return vec3<f32>(0.2126, 0.7152, 0.0722);
}

/// One NV12 sample pair back to non-linear R'G'B' in 0..1.
///
/// `y` is the luma texture's sample and `cbcr` the chroma texture's, both
/// already normalised to 0..1 by the sampler — so a code value of 16 arrives as
/// 16/255 and the limited-range rescale below is written in those units.
///
/// The result is *gamma-encoded* RGB, which is what the software path's swscale
/// pass also produces and what `provider::upload_rgba` then hands to the
/// sampler as `Rgba8UnormSrgb`. Linearising it is the caller's job, and the
/// caller must do it: the compositor blends in linear light.
fn yuv_to_rgb(y: f32, cbcr: vec2<f32>, matrix: u32, range: u32) -> vec3<f32> {
    var yy = y;
    var cb = cbcr.x - 0.5;
    var cr = cbcr.y - 0.5;
    if (range == RANGE_LIMITED) {
        yy = (y - 16.0 / 255.0) * (255.0 / 219.0);
        cb = (cbcr.x - 128.0 / 255.0) * (255.0 / 224.0);
        cr = (cbcr.y - 128.0 / 255.0) * (255.0 / 224.0);
    }

    let k = luma_weights(matrix);
    let r = yy + 2.0 * (1.0 - k.x) * cr;
    let b = yy + 2.0 * (1.0 - k.z) * cb;
    let g = (yy - k.x * r - k.z * b) / k.y;
    return clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
}

/// sRGB electro-optical transfer function, per channel.
///
/// Needed because the RGBA path binds its texture as `Rgba8UnormSrgb` and the
/// sampler linearises for free, while an `R8Unorm` luma plane gets no such
/// treatment. Without this the two paths would composite the same picture at
/// two different brightnesses — the hardware one visibly lighter — which is
/// exactly the kind of difference that survives review because each looks
/// plausible on its own.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}
