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

// Matrix and range selectors. These are the numbers `render::source::YuvMatrix`
// and `YuvRange` cast to; changing one without the other silently reinterprets
// every hardware-decoded frame, so they are asserted equal in a Rust test rather
// than trusted. They sit above both directions because both directions switch on
// them.
const MATRIX_BT601: u32 = 0u;
const MATRIX_BT709: u32 = 1u;
const MATRIX_BT2020: u32 = 2u;

const RANGE_LIMITED: u32 = 0u;
const RANGE_FULL: u32 = 1u;

// ---------------------------------------------------------------------------
// Forward: RGB to YUV. The encoder's direction.
// ---------------------------------------------------------------------------

// The matrix is a parameter. A video encoder is handed whatever the export
// tags its stream as — BT.709 for anything HD, which is what every player
// assumes of an untagged HD stream too — and the preview's JPEG encoder is
// handed BT.601, because JFIF *is* BT.601 and a JPEG file carries no tag to
// say otherwise. Hard-coding BT.601 here was the export's long-standing hue
// shift: saturated reds and greens off by up to ten code values in every
// player that read the file as BT.709. `export::colour` picks the matrix.
//
// The range picks the excursion the result is scaled into.
//
// **Limited** (219/224 about 16/128) is what a video encoder wants, and what
// `export::encoder` tags the stream as unless the user asked for full range.
//
// **Full** (255 about 0/128) is what a *JPEG* encoder wants — a JPEG file
// carries no range tag and every decoder reads it as 0..255. Handing the JPEG
// encoder limited-range samples produces grey blacks and no white: a valid file
// that reads as the editor having washed the footage out, scoring about 27 dB
// against the software encoder instead of 37. That is the trap
// `docs/research/vaapi-jpeg-preview.md` records, and it is why this is a
// parameter rather than a constant.
fn luma_in(c: vec3<f32>, matrix: u32, range: u32) -> f32 {
    let y = dot(luma_weights(matrix), c);
    if (range == RANGE_FULL) {
        return 255.0 * y;
    }
    return 16.0 + 219.0 * y;
}

// Cb and Cr from the luma weights: `Cb = (B - Y) / (2 (1 - Kb))` and the same
// for red. For BT.601 that is exactly the -0.168736/-0.331264/0.5 row the
// standard prints.
fn chroma_in(c: vec3<f32>, matrix: u32, range: u32) -> vec2<f32> {
    let k = luma_weights(matrix);
    let y = dot(k, c);
    let cb = (c.b - y) / (2.0 * (1.0 - k.z));
    let cr = (c.r - y) / (2.0 * (1.0 - k.x));
    let excursion = select(224.0, 255.0, range == RANGE_FULL);
    return vec2<f32>(128.0 + excursion * cb, 128.0 + excursion * cr);
}

/// Luma weights `(Kr, Kg, Kb)` for a matrix selector. Both directions use it.
fn luma_weights(matrix: u32) -> vec3<f32> {
    if (matrix == MATRIX_BT601) {
        return vec3<f32>(0.299, 0.587, 0.114);
    }
    if (matrix == MATRIX_BT2020) {
        return vec3<f32>(0.2627, 0.6780, 0.0593);
    }
    return vec3<f32>(0.2126, 0.7152, 0.0722);
}

// ---------------------------------------------------------------------------
// Inverse: YUV to RGB. The decoder's direction.
// ---------------------------------------------------------------------------

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

/// The inverse of `srgb_to_linear`, per channel.
///
/// Here because the colour adjustments in `quad.wgsl` are applied in
/// *gamma-encoded* space — brightness and contrast pivoting on encoded mid
/// grey is what every editor's sliders mean, and applying the same offsets in
/// linear light crushes shadows and blows highlights — so the quad shader has
/// to encode, adjust, and decode again. Living next to its inverse keeps the
/// two from drifting apart, the argument this whole file makes.
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let clamped = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = clamped * 12.92;
    let hi = 1.055 * pow(clamped, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, clamped <= vec3<f32>(0.0031308));
}

// ---------------------------------------------------------------------------
// HDR sources: PQ and HLG, BT.2020, 10-bit planes.
// ---------------------------------------------------------------------------
//
// The compositor works in SDR: BT.709 primaries, linear light, 1.0 is SDR
// white. A phone's HDR clip is none of those — its samples are PQ or HLG
// encoded, its primaries BT.2020 — and reading it as SDR is what made "phone
// HDR looks grey": a PQ signal decoded with the sRGB curve puts diffuse white
// at about 58 % and lifts every shadow. So a planar source carries its
// transfer and primaries along with its matrix, and `yuv_to_linear` takes it
// all the way to the working space in one place, for the hardware and the
// software decode alike (decision 0034).
//
// The steps, in order: YUV to non-linear R'G'B' (the matrix), the transfer's
// EOTF to display light in nits (PQ directly; HLG through its OOTF for a
// 1000-nit display), nits to SDR-relative light with HDR reference white
// (203 nits, ITU-R BT.2408) as 1.0, BT.2020 to BT.709 primaries, and the
// BT.2390 EETF on the brightest channel to fold highlights above SDR white
// back into range. Everything below the knee (about 0.45 of SDR white, i.e.
// all of the midtones) comes through unchanged, which is what makes an HDR
// clip cut next to an SDR one look like the same scene.

// The planar source's colour word, as `render::source::SourceFrame::colour_word`
// packs it: the matrix in the low byte (the selectors above), the transfer in
// the next, the primaries in the third, and a flag for 16-bit samples. The
// numbers are asserted equal to the Rust ones in a test.
const TRANSFER_SDR: u32 = 0u;
const TRANSFER_PQ: u32 = 1u;
const TRANSFER_HLG: u32 = 2u;
const PRIMARIES_BT709: u32 = 0u;
const PRIMARIES_BT2020: u32 = 1u;
const COLOUR_TRANSFER_SHIFT: u32 = 8u;
const COLOUR_PRIMARIES_SHIFT: u32 = 16u;
const COLOUR_DEEP_SAMPLES: u32 = 16777216u;

// HDR reference white, in nits: what SDR's 1.0 becomes. ITU-R BT.2408.
const REFERENCE_WHITE_NITS: f32 = 203.0;
// The mastering peak the tone map assumes. 1000 nits is what HLG is defined
// against and what almost every phone and consumer camera masters PQ to.
const SOURCE_PEAK_NITS: f32 = 1000.0;

/// SMPTE ST 2084 (PQ) EOTF: signal 0..1 to light, as a fraction of 10 000 nits.
fn pq_eotf(e: vec3<f32>) -> vec3<f32> {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let p = pow(clamp(e, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / m2));
    let num = max(p - vec3<f32>(c1), vec3<f32>(0.0));
    return pow(num / (vec3<f32>(c2) - c3 * p), vec3<f32>(1.0 / m1));
}

/// The inverse of `pq_eotf` for one value: light as a fraction of 10 000 nits
/// to signal. The tone map works in this space.
fn pq_inverse(y: f32) -> f32 {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let p = pow(clamp(y, 0.0, 1.0), m1);
    return pow((c1 + c2 * p) / (1.0 + c3 * p), m2);
}

fn pq_eotf1(e: f32) -> f32 {
    return pq_eotf(vec3<f32>(e)).x;
}

/// ARIB STD-B67 (HLG) inverse OETF: signal to scene light, 0..1.
fn hlg_scene(e: vec3<f32>) -> vec3<f32> {
    let a = 0.17883277;
    let b = 0.28466892;
    let c = 0.55991073;
    let lo = e * e / 3.0;
    let hi = (exp((e - vec3<f32>(c)) / a) + vec3<f32>(b)) / 12.0;
    return select(hi, lo, e <= vec3<f32>(0.5));
}

/// HLG to display light in nits: the inverse OETF, then the BT.2100 OOTF for
/// a display of `SOURCE_PEAK_NITS` (system gamma 1.2 at 1000 nits). The scene
/// luminance uses BT.2020's weights, because HLG's primaries are BT.2020.
fn hlg_display_nits(e: vec3<f32>) -> vec3<f32> {
    let scene = hlg_scene(clamp(e, vec3<f32>(0.0), vec3<f32>(1.0)));
    let ys = dot(vec3<f32>(0.2627, 0.6780, 0.0593), scene);
    let gamma = 1.2;
    return SOURCE_PEAK_NITS * pow(max(ys, 1e-6), gamma - 1.0) * scene;
}

/// Linear BT.2020 to linear BT.709. Out-of-gamut colours come out negative in
/// some channel; the caller clips them.
fn bt2020_to_bt709(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        1.660491 * c.r - 0.587641 * c.g - 0.072850 * c.b,
        -0.124551 * c.r + 1.132900 * c.g - 0.008349 * c.b,
        -0.018151 * c.r - 0.100579 * c.g + 1.118730 * c.b,
    );
}

/// ITU-R BT.2390 EETF for one value of SDR-relative light (1.0 = reference
/// white): maps 0..`SOURCE_PEAK_NITS` onto 0..reference white, with a
/// Hermite knee in PQ space so everything below the knee is untouched.
fn eetf(light: f32) -> f32 {
    let source_peak = pq_inverse(SOURCE_PEAK_NITS / 10000.0);
    let target_peak = pq_inverse(REFERENCE_WHITE_NITS / 10000.0) / source_peak;
    let knee = 1.5 * target_peak - 0.5;
    let e = min(pq_inverse(light * REFERENCE_WHITE_NITS / 10000.0) / source_peak, 1.0);
    if (e <= knee) {
        return light;
    }
    let t = (e - knee) / (1.0 - knee);
    let t2 = t * t;
    let t3 = t2 * t;
    let mapped = (2.0 * t3 - 3.0 * t2 + 1.0) * knee
        + (t3 - 2.0 * t2 + t) * (1.0 - knee)
        + (-2.0 * t3 + 3.0 * t2) * target_peak;
    return pq_eotf1(mapped * source_peak) * 10000.0 / REFERENCE_WHITE_NITS;
}

/// Tone-map SDR-relative linear BT.709 light into 0..1. On the brightest
/// channel, with the others scaled by the same ratio, so a highlight keeps
/// its hue instead of bleaching towards white channel by channel.
fn tone_map(c: vec3<f32>) -> vec3<f32> {
    let clipped = max(c, vec3<f32>(0.0));
    let peak = max(clipped.r, max(clipped.g, clipped.b));
    if (peak <= 0.0) {
        return vec3<f32>(0.0);
    }
    return clamp(clipped * (eetf(peak) / peak), vec3<f32>(0.0), vec3<f32>(1.0));
}

/// One planar sample pair to linear BT.709 light, 1.0 = SDR white: the whole
/// source conversion, for every transfer.
///
/// `colour` is the packed word described above. 16-bit samples (P010, or
/// 10-bit software decodes uploaded as P010) carry ten significant bits in the
/// top of sixteen, so a code value that is `c` in 8-bit units samples as
/// `256 c / 65535`, not `c / 255`; the factor below puts it back on the scale
/// `yuv_to_rgb`'s limited-range constants are written in.
fn yuv_to_linear(y: f32, cbcr: vec2<f32>, colour: u32, range: u32) -> vec3<f32> {
    var scale = 1.0;
    if ((colour & COLOUR_DEEP_SAMPLES) != 0u) {
        scale = 65535.0 / 65280.0;
    }
    let matrix = colour & 255u;
    let transfer = (colour >> COLOUR_TRANSFER_SHIFT) & 255u;
    let primaries = (colour >> COLOUR_PRIMARIES_SHIFT) & 255u;
    let encoded = yuv_to_rgb(y * scale, cbcr * scale, matrix, range);

    if (transfer == TRANSFER_SDR) {
        let light = srgb_to_linear(encoded);
        if (primaries == PRIMARIES_BT2020) {
            return clamp(bt2020_to_bt709(light), vec3<f32>(0.0), vec3<f32>(1.0));
        }
        return light;
    }

    var nits: vec3<f32>;
    if (transfer == TRANSFER_PQ) {
        nits = pq_eotf(encoded) * 10000.0;
    } else {
        nits = hlg_display_nits(encoded);
    }
    var light = nits / REFERENCE_WHITE_NITS;
    if (primaries == PRIMARIES_BT2020) {
        light = bt2020_to_bt709(light);
    }
    return tone_map(light);
}
