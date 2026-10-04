//! The extended grade on the GPU: uniform packing, curve tables, and the CPU
//! reference the shader is tested against.
//!
//! ## Order of operations
//!
//! `quad.wgsl` applies a clip's colour in this order. Each stage runs only
//! when its controls are away from rest (a bit in `features`, or the older
//! `color_active` / `lut_active` flags), so an ungraded clip takes exactly
//! the shader path it took before any of this existed.
//!
//! **Linear light, on the decoded source** — the stages that model light or
//! need the neighbourhood of the pixel:
//!
//! 1. *Sharpen* — an unsharp mask over the four direct neighbours.
//! 2. *Clarity* — measured here: the pixel's encoded luma against the mean
//!    of a ring of twelve taps (radius [`CLARITY_RADIUS`] of the short side).
//!    The difference is applied in step 4.
//! 3. *Exposure* — multiply by `2^stops`. Exposure is a statement about
//!    light, so it is the one tonal control that must be linear: +1 stop
//!    doubles the light, which in encoded space is a curve, not a factor.
//!
//! **Gamma-encoded (sRGB) space** — the perceptual controls. Sliders that
//! pivot on mid grey or weight by tone mean what users expect only on the
//! encoded image, and `.cube` looks are authored against it:
//!
//! 4. *Clarity* — adds the step-2 detail, weighted to the midtones.
//! 5. *Tint* — green/magenta shift.
//! 6. *The original grade* — temperature, saturation, contrast, brightness
//!    (unchanged, so old projects render as they did).
//! 7. *Whites and blacks* — a levels remap of the white and black points.
//! 8. *Highlights and shadows* — luma-masked lifts.
//! 9. *Colour wheels* — lift, gain, gamma, offset (ASC CDL order, with lift
//!    in front as the Resolve-style "shadows" wheel).
//! 10. *HSL* — per-hue hue, saturation and luminance over eight bands.
//! 11. *Vibrance* — saturation weighted towards the unsaturated.
//! 12. *Curves* — master, then red, green and blue.
//! 13. *LUT* — the 3D or 1D look, blended by its intensity.
//! 14. *Fade* — lifts black and lowers white: the look on top of the look.
//!
//! **Linear light again, after decoding** — the stages that model the lens
//! and the film:
//!
//! 15. *Vignette* — multiplies light towards the edges (or lifts it).
//! 16. *Grain* — multiplicative noise, per output pixel, reseeded per frame.
//!
//! ## Why the CPU mirrors it
//!
//! [`reference_encoded`] spells the per-pixel stages out again in Rust. The
//! tests render through the GPU and compare against it, so a change to
//! either side fails a test instead of silently agreeing with itself.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use super::context::RenderContext;
use crate::modules::project::grade::{CurveChannel, Curves, Grade, Wheel, WheelKind, HSL_BANDS};

// Strengths: how far a control at `±1` moves the image. Arbitrary in the way
// every editor's slider ranges are arbitrary, chosen so the extremes are
// strong but not broken. What is not arbitrary is that each lives here once
// and the shader receives it through `quad.wgsl` constants of the same names.

/// Tint `+1` lowers green by this much in encoded units (and raises red and
/// blue by half of it), the twin of `TEMPERATURE_STRENGTH` in `quad.wgsl`.
pub const TINT_STRENGTH: f32 = 0.2;
/// Whites/blacks `±1` move the white or black point by this much.
pub const LEVELS_STRENGTH: f32 = 0.25;
/// Highlights/shadows `±1` add up to this much to the masked tones.
pub const TONE_STRENGTH: f32 = 0.3;
/// HSL hue `±1` turns a band by this many degrees.
pub const HSL_HUE_DEGREES: f32 = 30.0;
/// HSL luminance `±1` scales a fully saturated colour's value by `1 ± this`.
pub const HSL_LUMA_STRENGTH: f32 = 0.5;
/// Wheel strengths, per wheel, for a puck on the rim or luma at `±1`.
pub const LIFT_STRENGTH: f32 = 0.3;
pub const GAIN_STRENGTH: f32 = 0.5;
pub const GAMMA_STRENGTH: f32 = 0.5;
pub const OFFSET_STRENGTH: f32 = 0.2;
/// Fade `1` lifts black to this...
pub const FADE_LIFT: f32 = 0.25;
/// ...and scales the range by `1 - FADE_COMPRESS`, so white lands at 0.9.
pub const FADE_COMPRESS: f32 = 0.35;
/// Sharpen `1` adds this many times the difference from the local mean.
pub const SHARPEN_STRENGTH: f32 = 2.0;
/// Clarity `±1` adds this many times the midtone detail.
pub const CLARITY_STRENGTH: f32 = 1.5;
/// Clarity's ring radius as a fraction of the source's short side. A fraction
/// rather than pixels so the preview, rendered small, and the export, rendered
/// large, measure the same structure.
pub const CLARITY_RADIUS: f32 = 0.012;
/// Grain `1` multiplies the light by up to `1 ± this`.
pub const GRAIN_STRENGTH: f32 = 0.25;

/// Width of the baked curve table. 1024 entries joined linearly stay within
/// a hundredth of a code value of the exact cubic for any curve a user can
/// draw — the test `baked_curves_match_the_exact_cubic` pins it.
pub const CURVE_SIZE: u32 = 1024;

/// The bits of `QuadUniform::features`. Shared with `quad.wgsl`, which spells
/// the same numbers; `the_feature_bits_match_the_shader` reads the shader
/// source and checks.
pub mod feature {
    pub const EXPOSURE: u32 = 1;
    pub const TINT: u32 = 1 << 1;
    pub const TONE: u32 = 1 << 2;
    pub const WHEELS: u32 = 1 << 3;
    pub const HSL: u32 = 1 << 4;
    pub const VIBRANCE: u32 = 1 << 5;
    pub const CURVES: u32 = 1 << 6;
    pub const FADE: u32 = 1 << 7;
    pub const SHARPEN: u32 = 1 << 8;
    pub const CLARITY: u32 = 1 << 9;
    pub const VIGNETTE: u32 = 1 << 10;
    pub const GRAIN: u32 = 1 << 11;
    /// Not a control: the bound LUT is a 1D table rather than a cube.
    pub const LUT_1D: u32 = 1 << 12;
    /// Not a control: the source holds premultiplied colour — a compound
    /// clip's nested render — and is divided by its alpha before anything else.
    pub const PREMULTIPLIED: u32 = 1 << 13;
}

/// The extended grade, packed the way the uniform block wants it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GradeBlock {
    pub features: u32,
    /// `2^exposure`, tint, highlights, shadows.
    pub tone: [f32; 4],
    /// Whites, blacks, vibrance, fade.
    pub tone2: [f32; 4],
    /// Sharpen, clarity, grain, grain seed.
    pub detail: [f32; 4],
    /// Amount, midpoint, feather, unused.
    pub vignette: [f32; 4],
    /// Per-channel wheel factors, already reduced from the pucks: see
    /// [`wheel_factors`].
    pub lift: [f32; 4],
    pub gain: [f32; 4],
    pub gamma: [f32; 4],
    pub offset: [f32; 4],
    /// Per band: hue, saturation, luminance, unused.
    pub hsl: [[f32; 4]; 8],
}

impl GradeBlock {
    /// Pack `grade`. The feature bits are set only for stages away from rest;
    /// the curve bit is set by the caller once the table is bound, and the
    /// grain seed is the caller's too (it changes per frame).
    pub fn new(grade: &Grade) -> Self {
        use feature::*;
        let mut f = 0;
        let mut on = |bit: u32, cond: bool| {
            if cond {
                f |= bit;
            }
        };
        on(EXPOSURE, grade.exposure != 0.0);
        on(TINT, grade.tint != 0.0);
        on(
            TONE,
            grade.highlights != 0.0
                || grade.shadows != 0.0
                || grade.whites != 0.0
                || grade.blacks != 0.0,
        );
        on(
            WHEELS,
            WheelKind::ALL
                .iter()
                .any(|&k| !grade.wheels.get(k).is_identity()),
        );
        on(HSL, grade.hsl != Default::default());
        on(VIBRANCE, grade.vibrance != 0.0);
        on(FADE, grade.fade != 0.0);
        on(SHARPEN, grade.sharpen != 0.0);
        on(CLARITY, grade.clarity != 0.0);
        on(VIGNETTE, grade.vignette.amount != 0.0);
        on(GRAIN, grade.grain != 0.0);

        let [lift, gain, gamma, offset] = wheel_factors(grade);
        let mut hsl = [[0.0; 4]; 8];
        for (out, band) in hsl.iter_mut().zip(grade.hsl.bands.iter()) {
            *out = [band.hue, band.saturation, band.luminance, 0.0];
        }
        Self {
            features: f,
            tone: [
                grade.exposure.exp2(),
                grade.tint,
                grade.highlights,
                grade.shadows,
            ],
            tone2: [grade.whites, grade.blacks, grade.vibrance, grade.fade],
            detail: [grade.sharpen, grade.clarity, grade.grain, 0.0],
            vignette: [
                grade.vignette.amount,
                grade.vignette.midpoint,
                grade.vignette.feather,
                0.0,
            ],
            lift: with_w(lift),
            gain: with_w(gain),
            gamma: with_w(gamma),
            offset: with_w(offset),
            hsl,
        }
    }

    /// The block for a clip with no extended grade: every factor at its
    /// identity and no feature bit set.
    pub fn identity() -> Self {
        Self::new(&Grade::default())
    }
}

fn with_w(v: [f32; 3]) -> [f32; 4] {
    [v[0], v[1], v[2], 0.0]
}

/// The direction a wheel's puck pushes, as an RGB offset that sums to zero
/// (so it moves colour, not brightness), scaled by how far the puck is from
/// the centre, plus the wheel's luma on every channel.
///
/// The hue under the puck is the fully saturated colour at the puck's angle;
/// subtracting its mean leaves a pure chroma direction — red is
/// `(2/3, -1/3, -1/3)`.
pub fn wheel_vector(wheel: &Wheel) -> [f32; 3] {
    let r = (wheel.x * wheel.x + wheel.y * wheel.y).sqrt().min(1.0);
    let mut out = [wheel.luma; 3];
    if r > 0.0 {
        let hue = wheel.y.atan2(wheel.x).to_degrees().rem_euclid(360.0) / 360.0;
        let rgb = hsv_to_rgb([hue, 1.0, 1.0]);
        let mean = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
        for c in 0..3 {
            out[c] += (rgb[c] - mean) * r;
        }
    }
    out
}

/// `[lift, gain, gamma exponent, offset]`, per channel, for the shader's
///
/// ```text
/// c = c + lift * (1 - c);  c = c * gain;  c = pow(c, gamma);  c = c + offset
/// ```
pub fn wheel_factors(grade: &Grade) -> [[f32; 3]; 4] {
    let lift = wheel_vector(&grade.wheels.lift).map(|w| w * LIFT_STRENGTH);
    let gain = wheel_vector(&grade.wheels.gain).map(|w| (1.0 + w * GAIN_STRENGTH).max(0.0));
    // A positive push brightens the midtones, so it lowers the exponent;
    // the floor keeps a pinned puck from dividing by zero.
    let gamma =
        wheel_vector(&grade.wheels.gamma).map(|w| 1.0 / (1.0 + w * GAMMA_STRENGTH).max(0.1));
    let offset = wheel_vector(&grade.wheels.offset).map(|w| w * OFFSET_STRENGTH);
    [lift, gain, gamma, offset]
}

// ---------------------------------------------------------------------------
// Curves
// ---------------------------------------------------------------------------

/// A monotone cubic (Fritsch–Carlson) through `points`, evaluated at `x`.
///
/// Monotone because a tone curve that overshoots between two points inverts
/// tones there, which no user ever means. Flat beyond the first and last
/// point. An empty list is the identity.
pub fn eval_curve(points: &[[f32; 2]], x: f32) -> f32 {
    match points.len() {
        0 => return x,
        1 => return points[0][1],
        _ => {}
    }
    let n = points.len();
    if x <= points[0][0] {
        return points[0][1];
    }
    if x >= points[n - 1][0] {
        return points[n - 1][1];
    }
    let tangents = curve_tangents(points);
    let k = points
        .windows(2)
        .position(|w| x >= w[0][0] && x <= w[1][0])
        .unwrap_or(n - 2);
    let (x0, y0) = (points[k][0], points[k][1]);
    let (x1, y1) = (points[k + 1][0], points[k + 1][1]);
    let h = x1 - x0;
    if h <= 0.0 {
        return y1;
    }
    let t = (x - x0) / h;
    let (t2, t3) = (t * t, t * t * t);
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    (h00 * y0 + h10 * h * tangents[k] + h01 * y1 + h11 * h * tangents[k + 1]).clamp(0.0, 1.0)
}

/// Fritsch–Carlson tangents: secant averages, zeroed at local extrema and
/// scaled back wherever they would overshoot.
fn curve_tangents(points: &[[f32; 2]]) -> Vec<f32> {
    let n = points.len();
    let secant: Vec<f32> = points
        .windows(2)
        .map(|w| {
            let dx = w[1][0] - w[0][0];
            if dx > 0.0 {
                (w[1][1] - w[0][1]) / dx
            } else {
                0.0
            }
        })
        .collect();
    let mut m = vec![0f32; n];
    m[0] = secant[0];
    m[n - 1] = secant[n - 2];
    for i in 1..n - 1 {
        m[i] = if secant[i - 1] * secant[i] <= 0.0 {
            0.0
        } else {
            (secant[i - 1] + secant[i]) * 0.5
        };
    }
    for i in 0..n - 1 {
        if secant[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let a = m[i] / secant[i];
        let b = m[i + 1] / secant[i];
        let s = a * a + b * b;
        if s > 9.0 {
            let tau = 3.0 / s.sqrt();
            m[i] = tau * a * secant[i];
            m[i + 1] = tau * b * secant[i];
        }
    }
    m
}

/// The four curves baked into one `CURVE_SIZE`-wide RGBA row: red, green,
/// blue in `rgb`, master in `a` — the layout `curve_at` in `quad.wgsl` reads.
pub fn bake_curves(curves: &Curves) -> Vec<[f32; 4]> {
    let last = (CURVE_SIZE - 1) as f32;
    (0..CURVE_SIZE)
        .map(|i| {
            let x = i as f32 / last;
            [
                eval_curve(&curves.red, x),
                eval_curve(&curves.green, x),
                eval_curve(&curves.blue, x),
                eval_curve(&curves.master, x),
            ]
        })
        .collect()
}

/// One lookup in a baked table, the arithmetic `curve_at` mirrors.
pub fn table_at(table: &[[f32; 4]], x: f32, channel: usize) -> f32 {
    let n = table.len();
    let p = x.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (p.floor() as usize).min(n - 2);
    let t = p - i as f32;
    table[i][channel] + (table[i + 1][channel] - table[i][channel]) * t
}

/// A baked curve table on the device.
pub struct GpuCurves {
    pub view: wgpu::TextureView,
}

/// Curve point lists → uploaded tables.
///
/// Keyed by the points' bits rather than a material id: the inspector shows
/// a drag by rendering a *copy* of the project, whose material has no
/// committed id yet, and every one of those frames should reuse the table
/// of the frame before when the curve did not move. Bounded, because every
/// drag step mints a new key; clearing the whole map past the bound costs
/// one re-upload per live curve, which is a few kilobytes.
#[derive(Default)]
pub struct CurveCache {
    entries: Mutex<HashMap<Vec<u32>, Arc<GpuCurves>>>,
}

const CURVE_CACHE_LIMIT: usize = 64;

impl CurveCache {
    pub fn get(&self, ctx: &RenderContext, curves: &Curves) -> Arc<GpuCurves> {
        let key = curve_key(curves);
        let mut entries = self.entries.lock();
        if let Some(hit) = entries.get(&key) {
            return Arc::clone(hit);
        }
        if entries.len() >= CURVE_CACHE_LIMIT {
            entries.clear();
        }
        let table = bake_curves(curves);
        let gpu = Arc::new(upload_curves(ctx, &table));
        entries.insert(key, Arc::clone(&gpu));
        gpu
    }
}

fn curve_key(curves: &Curves) -> Vec<u32> {
    let mut key = Vec::new();
    for channel in CurveChannel::ALL {
        let points = curves.get(channel);
        key.push(points.len() as u32);
        for p in points {
            key.push(p[0].to_bits());
            key.push(p[1].to_bits());
        }
    }
    key
}

fn upload_curves(ctx: &RenderContext, table: &[[f32; 4]]) -> GpuCurves {
    let width = table.len() as u32;
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("chukcut curves"),
        size: wgpu::Extent3d {
            width,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(table),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 16),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    GpuCurves {
        view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
    }
}

// ---------------------------------------------------------------------------
// The CPU reference
// ---------------------------------------------------------------------------

/// HSV with every component in `0..1`, the conversion `quad.wgsl` uses.
pub fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    let d = mx - mn;
    let mut h = 0.0;
    if d > 0.0 {
        h = if mx == c[0] {
            let h = (c[1] - c[2]) / d;
            if h < 0.0 {
                h + 6.0
            } else {
                h
            }
        } else if mx == c[1] {
            (c[2] - c[0]) / d + 2.0
        } else {
            (c[0] - c[1]) / d + 4.0
        };
        h /= 6.0;
    }
    let s = if mx > 0.0 { d / mx } else { 0.0 };
    [h, s, mx]
}

pub fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let h6 = hsv[0].rem_euclid(1.0) * 6.0;
    let r = ((h6 - 3.0).abs() - 1.0).clamp(0.0, 1.0);
    let g = (2.0 - (h6 - 2.0).abs()).clamp(0.0, 1.0);
    let b = (2.0 - (h6 - 4.0).abs()).clamp(0.0, 1.0);
    [r, g, b].map(|k| hsv[2] * (1.0 + (k - 1.0) * hsv[1]))
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn luma601(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// The two bands whose centres bracket `hue` (`0..1`), and how far between
/// them it sits — the weights `hsl_adjust` in `quad.wgsl` blends with.
pub fn hsl_bands_for(hue: f32) -> (usize, usize, f32) {
    let degrees = hue.rem_euclid(1.0) * 360.0;
    for i in 0..8 {
        let lo = HSL_BANDS[i].1;
        let hi = if i == 7 { 360.0 } else { HSL_BANDS[i + 1].1 };
        if degrees >= lo && degrees < hi {
            return (i, (i + 1) % 8, (degrees - lo) / (hi - lo));
        }
    }
    (0, 1, 0.0)
}

/// A stand-in for the LUT stage in [`reference_encoded`]: the lookup and its
/// intensity.
pub type Look<'a> = (&'a dyn Fn([f32; 3]) -> [f32; 3], f32);

/// The encoded-space stages (4–14 in the module docs, minus clarity, which
/// needs neighbours) applied to one encoded pixel, the arithmetic the shader
/// runs. `color` is the original four scalars when they are active; `look`
/// is a function standing in for the LUT stage, with its intensity.
pub fn reference_encoded(
    mut c: [f32; 3],
    color: Option<[f32; 4]>,
    grade: &Grade,
    look: Option<Look<'_>>,
) -> [f32; 3] {
    let block = GradeBlock::new(grade);
    let f = block.features;
    use feature::*;

    if f & TINT != 0 {
        let t = grade.tint * TINT_STRENGTH;
        c = [c[0] + 0.5 * t, c[1] - t, c[2] + 0.5 * t];
    }
    if let Some([brightness, contrast, saturation, temperature]) = color {
        c[0] += temperature * 0.2;
        c[2] -= temperature * 0.2;
        let grey = luma601(c);
        c = c.map(|v| ((grey + (v - grey) * saturation) - 0.5) * contrast + 0.5 + brightness);
    }
    if f & TONE != 0 {
        let white = 1.0 - LEVELS_STRENGTH * grade.whites;
        let black = -LEVELS_STRENGTH * grade.blacks;
        c = c.map(|v| (v - black) / (white - black));
        let l = luma601(c);
        let ws = 1.0 - smoothstep(0.0, 0.6, l);
        let wh = smoothstep(0.4, 1.0, l);
        let lift = TONE_STRENGTH * (grade.shadows * ws + grade.highlights * wh);
        c = c.map(|v| v + lift);
    }
    if f & WHEELS != 0 {
        let [lift, gain, gamma, offset] = wheel_factors(grade);
        for i in 0..3 {
            let mut v = c[i];
            v += lift[i] * (1.0 - v);
            v *= gain[i];
            v = v.max(0.0).powf(gamma[i]);
            v += offset[i];
            c[i] = v;
        }
    }
    if f & HSL != 0 {
        let hsv = rgb_to_hsv(c.map(|v| v.clamp(0.0, 1.0)));
        let (a, b, t) = hsl_bands_for(hsv[0]);
        let mix = |k: usize| {
            let pa = [
                grade.hsl.bands[a].hue,
                grade.hsl.bands[a].saturation,
                grade.hsl.bands[a].luminance,
            ][k];
            let pb = [
                grade.hsl.bands[b].hue,
                grade.hsl.bands[b].saturation,
                grade.hsl.bands[b].luminance,
            ][k];
            pa + (pb - pa) * t
        };
        let h = hsv[0] + mix(0) * HSL_HUE_DEGREES / 360.0;
        let s = (hsv[1] * (1.0 + mix(1))).clamp(0.0, 1.0);
        let v = hsv[2] * (1.0 + mix(2) * HSL_LUMA_STRENGTH * hsv[1]);
        c = hsv_to_rgb([h, s, v]);
    }
    if f & VIBRANCE != 0 {
        let mx = c[0].max(c[1]).max(c[2]);
        let mn = c[0].min(c[1]).min(c[2]);
        let amount = 1.0 + grade.vibrance * (1.0 - (mx - mn).clamp(0.0, 1.0));
        let grey = luma601(c);
        c = c.map(|v| grey + (v - grey) * amount);
    }
    if !grade.curves.is_identity() {
        let table = bake_curves(&grade.curves);
        for (i, v) in c.iter_mut().enumerate() {
            *v = table_at(&table, table_at(&table, *v, 3), i);
        }
    }
    if let Some((lut, intensity)) = look {
        let looked = lut(c);
        for i in 0..3 {
            c[i] += (looked[i] - c[i]) * intensity;
        }
    }
    if f & FADE != 0 {
        c = c.map(|v| v * (1.0 - FADE_COMPRESS * grade.fade) + FADE_LIFT * grade.fade);
    }
    c.map(|v| v.clamp(0.0, 1.0))
}

/// The vignette's mask at a position in the quad (`0..1` each way): `0` at
/// the untouched centre, `1` where the full amount applies.
pub fn vignette_mask(local: [f32; 2], midpoint: f32, feather: f32) -> f32 {
    let dx = (local[0] - 0.5) * 2.0;
    let dy = (local[1] - 0.5) * 2.0;
    let d = (dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2;
    let half = feather.max(0.001) * 0.5;
    smoothstep(midpoint - half, midpoint + half, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::grade::HslBand;

    #[test]
    fn the_feature_bits_match_the_shader() {
        let shader = include_str!("shaders/quad.wgsl");
        for (name, bit) in [
            ("F_EXPOSURE", feature::EXPOSURE),
            ("F_TINT", feature::TINT),
            ("F_TONE", feature::TONE),
            ("F_WHEELS", feature::WHEELS),
            ("F_HSL", feature::HSL),
            ("F_VIBRANCE", feature::VIBRANCE),
            ("F_CURVES", feature::CURVES),
            ("F_FADE", feature::FADE),
            ("F_SHARPEN", feature::SHARPEN),
            ("F_CLARITY", feature::CLARITY),
            ("F_VIGNETTE", feature::VIGNETTE),
            ("F_GRAIN", feature::GRAIN),
            ("F_LUT_1D", feature::LUT_1D),
        ] {
            let needle = format!("const {name}: u32 = {bit}u;");
            assert!(shader.contains(&needle), "quad.wgsl lacks `{needle}`");
        }
        for (name, value) in [
            ("TINT_STRENGTH", TINT_STRENGTH),
            ("LEVELS_STRENGTH", LEVELS_STRENGTH),
            ("TONE_STRENGTH", TONE_STRENGTH),
            ("HSL_HUE_DEGREES", HSL_HUE_DEGREES),
            ("HSL_LUMA_STRENGTH", HSL_LUMA_STRENGTH),
            ("FADE_LIFT", FADE_LIFT),
            ("FADE_COMPRESS", FADE_COMPRESS),
            ("SHARPEN_STRENGTH", SHARPEN_STRENGTH),
            ("CLARITY_STRENGTH", CLARITY_STRENGTH),
            ("CLARITY_RADIUS", CLARITY_RADIUS),
            ("GRAIN_STRENGTH", GRAIN_STRENGTH),
        ] {
            let needle = format!("const {name}: f32 = {value:?};");
            assert!(shader.contains(&needle), "quad.wgsl lacks `{needle}`");
        }
        assert!(shader.contains(&format!("const CURVE_SIZE: u32 = {CURVE_SIZE}u;")));
    }

    #[test]
    fn an_identity_grade_sets_no_feature() {
        assert_eq!(GradeBlock::identity().features, 0);
        let factors = wheel_factors(&Grade::default());
        assert_eq!(factors, [[0.0; 3], [1.0; 3], [1.0; 3], [0.0; 3]]);
    }

    #[test]
    fn a_curve_through_its_points_never_overshoots() {
        // An S-curve: through every point exactly, monotone between them.
        let points = [
            [0.0, 0.0],
            [0.25, 0.15],
            [0.5, 0.5],
            [0.75, 0.85],
            [1.0, 1.0],
        ];
        for p in points {
            assert!((eval_curve(&points, p[0]) - p[1]).abs() < 1e-6);
        }
        let mut last = -1.0;
        for i in 0..=1000 {
            let y = eval_curve(&points, i as f32 / 1000.0);
            assert!(y >= last - 1e-6, "dipped at {i}: {y} after {last}");
            last = y;
        }
        // A plateau stays flat: no bump between two equal points.
        let plateau = [[0.0, 0.0], [0.3, 0.6], [0.7, 0.6], [1.0, 1.0]];
        for i in 30..=70 {
            let y = eval_curve(&plateau, i as f32 / 100.0);
            assert!((y - 0.6).abs() < 1e-6, "plateau moved at {i}: {y}");
        }
        // Flat outside the end points.
        let inner = [[0.2, 0.3], [0.8, 0.7]];
        assert_eq!(eval_curve(&inner, 0.0), 0.3);
        assert_eq!(eval_curve(&inner, 1.0), 0.7);
    }

    #[test]
    fn baked_curves_match_the_exact_cubic() {
        let curves = Curves {
            master: vec![[0.0, 0.05], [0.3, 0.2], [0.6, 0.75], [1.0, 0.95]],
            red: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            ..Default::default()
        };
        let table = bake_curves(&curves);
        let mut worst = 0f32;
        for i in 0..=4000 {
            let x = i as f32 / 4000.0;
            let exact = eval_curve(&curves.master, x);
            worst = worst.max((table_at(&table, x, 3) - exact).abs());
            let exact = eval_curve(&curves.red, x);
            worst = worst.max((table_at(&table, x, 0) - exact).abs());
        }
        assert!(worst < 0.01 / 255.0, "worst baked error {worst}");
        // The identity channels are the identity.
        for i in 0..=100 {
            let x = i as f32 / 100.0;
            assert!((table_at(&table, x, 2) - x).abs() < 1e-6);
        }
    }

    #[test]
    fn hsl_bands_partition_the_hue_circle() {
        assert_eq!(hsl_bands_for(0.0), (0, 1, 0.0));
        let (a, b, t) = hsl_bands_for(90.0 / 360.0);
        assert_eq!((a, b), (2, 3));
        assert!((t - 0.5).abs() < 1e-6);
        let (a, b, t) = hsl_bands_for(330.0 / 360.0);
        assert_eq!((a, b), (7, 0));
        assert!((t - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hsv_round_trips() {
        for c in [
            [0.2, 0.5, 0.9],
            [1.0, 0.0, 0.0],
            [0.3, 0.3, 0.3],
            [0.9, 0.8, 0.1],
        ] {
            let back = hsv_to_rgb(rgb_to_hsv(c));
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-5, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn the_reference_does_what_each_control_says() {
        let grey = [0.5, 0.5, 0.5];
        let run = |g: &Grade, c: [f32; 3]| reference_encoded(c, None, g, None);

        // Identity in, identity out.
        assert_eq!(run(&Grade::default(), [0.2, 0.4, 0.6]), [0.2, 0.4, 0.6]);

        let g = Grade {
            tint: 1.0,
            ..Default::default()
        };
        let out = run(&g, grey);
        assert!(out[1] < 0.5 && out[0] > 0.5, "tint +1 is magenta: {out:?}");

        let g = Grade {
            shadows: 1.0,
            ..Default::default()
        };
        assert!(run(&g, [0.1; 3])[0] > 0.1 + 0.2, "shadows lift the darks");
        assert!(run(&g, [0.95; 3])[0] < 0.96, "and leave the brights");

        let g = Grade {
            blacks: -1.0,
            ..Default::default()
        };
        assert_eq!(run(&g, [0.2; 3])[0], 0.0, "crushed blacks clip");

        // Greys carry no hue: HSL leaves them exactly alone.
        let mut g = Grade::default();
        g.hsl.bands = [HslBand {
            hue: 1.0,
            saturation: 1.0,
            luminance: 1.0,
        }; 8];
        let out = run(&g, grey);
        for v in out {
            assert!((v - 0.5).abs() < 1e-6, "{out:?}");
        }
        // But pure red turned by +30 degrees is orange.
        let mut g = Grade::default();
        g.hsl.bands[0].hue = 1.0;
        let out = run(&g, [1.0, 0.0, 0.0]);
        assert!((out[1] - 0.5).abs() < 1e-5 && out[0] == 1.0, "{out:?}");

        // A red lift wheel tints the shadows red and leaves white white.
        let mut g = Grade::default();
        g.wheels.lift.x = 1.0;
        let dark = run(&g, [0.0; 3]);
        assert!(dark[0] > dark[1] && dark[0] > dark[2], "{dark:?}");
        assert_eq!(run(&g, [1.0; 3]), [1.0; 3]);

        let g = Grade {
            fade: 1.0,
            ..Default::default()
        };
        let black = run(&g, [0.0; 3]);
        assert!((black[0] - FADE_LIFT).abs() < 1e-6);

        let g = Grade {
            vibrance: 1.0,
            ..Default::default()
        };
        let dull = run(&g, [0.5, 0.45, 0.4]);
        let vivid = run(&g, [1.0, 0.0, 0.0]);
        assert!(
            dull[0] - dull[2] > 0.1 + 0.05,
            "a dull colour gains: {dull:?}"
        );
        assert_eq!(vivid, [1.0, 0.0, 0.0], "a saturated one is spared");
    }

    #[test]
    fn the_vignette_mask_runs_from_centre_to_corner() {
        assert_eq!(vignette_mask([0.5, 0.5], 0.5, 0.5), 0.0);
        assert!((vignette_mask([0.0, 0.0], 0.5, 0.5) - 1.0).abs() < 1e-6);
        let edge = vignette_mask([0.0, 0.5], 0.5, 0.5);
        assert!(edge > 0.0 && edge < 1.0, "{edge}");
    }
}
