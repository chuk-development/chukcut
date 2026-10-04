//! Auto adjust: exposure, white balance and contrast from the picture's own
//! statistics.
//!
//! No model. `docs/research/ml-features.md` §3.14 explains why: the classic
//! measurements — the log-average luminance for exposure, the near-neutral
//! pixels for white balance, robust percentiles for the black and white
//! points — are what a colourist reads off the scopes, and a network adds
//! little to them. The answer is written into the clip's ordinary grade
//! controls (exposure, temperature, tint, whites, blacks, vibrance), so the
//! user sees what auto adjust did and can move any of it.
//!
//! Each step measures the picture *as graded so far* through
//! [`pixels::Simulation`], the CPU twin of the shader. The user's own other
//! controls (contrast, a LUT, curves, HSL) stay and are part of what is
//! measured, so auto adjust balances the picture the viewer actually sees.

use serde::Serialize;

use super::pixels::{lab, luminance, quantile, Samples};
use crate::modules::inspector::edit::GradeEdit;
use crate::modules::render::lut::Cube;

/// The log-average luminance a well-exposed picture has: 18 % grey.
const TARGET_KEY: f32 = 0.18;
/// How much of the measured exposure error is corrected. Less than all of
/// it: a dark scene is often meant to be dark, and the user can push further.
const EXPOSURE_SHARE: f32 = 0.7;
const MAX_EXPOSURE: f32 = 2.0;
/// How much of a colour cast is removed. A little warmth at golden hour is
/// the point of the shot; removing all of it looks clinical.
const BALANCE_SHARE: f32 = 0.85;
/// Where the darkest 0.5 % and the brightest 0.5 % of the picture land.
const BLACK_TARGET: f32 = 0.02;
const WHITE_TARGET: f32 = 0.97;
/// The most a levels stretch may expand the tonal range: 1 / this.
const MIN_SPAN: f32 = 0.6;
/// Average L*a*b* chroma under which the picture gets a little vibrance.
const DULL_CHROMA: f32 = 16.0;

/// What auto adjust measured, before and after.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Measure {
    /// Log-average luminance, linear (`0.18` is a mid-grey key).
    pub key: f32,
    /// Encoded luma of the darkest and brightest 0.5 %.
    pub black: f32,
    pub white: f32,
    /// Mean a* and b* of the near-neutral pixels: the colour cast.
    pub cast: [f32; 2],
    /// Mean L*a*b* chroma.
    pub chroma: f32,
}

impl Measure {
    pub fn of(samples: &Samples) -> Self {
        Self::measured(samples, None)
    }

    /// [`Self::of`] with the cast taken on the pixels `pool` names, so a
    /// before and an after compare the same pixels.
    fn on_pool(samples: &Samples, pool: &[usize]) -> Self {
        Self::measured(samples, Some(pool))
    }

    fn measured(samples: &Samples, pool: Option<&[usize]>) -> Self {
        let n = samples.pixels.len().max(1) as f32;
        let key = (samples
            .pixels
            .iter()
            .map(|&c| (luminance(c) + 1e-4).ln())
            .sum::<f32>()
            / n)
            .exp();
        let mut lumas: Vec<f32> = samples.pixels.iter().map(|&c| luma601(c)).collect();
        let black = quantile(&mut lumas, 0.005);
        let white = quantile(&mut lumas, 0.995);
        let labs: Vec<[f32; 3]> = samples.pixels.iter().map(|&c| lab(c)).collect();
        let chroma = labs.iter().map(|l| l[1].hypot(l[2])).sum::<f32>() / n;
        Self {
            key,
            black,
            white,
            cast: match pool {
                Some(pool) => cast_on(&labs, pool),
                None => cast_of(&labs),
            },
            chroma,
        }
    }
}

fn luma601(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// The pixels a cast is measured on: the most neutral third of those that
/// are neither black nor clipped — probably grey in the world. Chosen once,
/// on the picture before any correction, and kept while the balance is
/// fitted: a pool that changed with every step would make the measured cast
/// jump and the fit oscillate. The second value is how much to trust it: a
/// picture whose most neutral third is still clearly coloured (a red wall)
/// has no greys to measure, and half the correction keeps the wall red.
fn neutral_pool(labs: &[[f32; 3]]) -> (Vec<usize>, f32) {
    let mut candidates: Vec<(usize, f32)> = labs
        .iter()
        .enumerate()
        .filter(|(_, l)| l[0] > 15.0 && l[0] < 95.0)
        .map(|(i, l)| (i, l[1].hypot(l[2])))
        .collect();
    if candidates.is_empty() {
        return (Vec::new(), 0.0);
    }
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
    let keep = (candidates.len() / 3).max(1);
    let limit = candidates[keep - 1].1;
    let trust = if limit < 20.0 { 1.0 } else { 0.5 };
    (candidates[..keep].iter().map(|&(i, _)| i).collect(), trust)
}

/// Mean a*/b* over `pool`.
fn cast_on(labs: &[[f32; 3]], pool: &[usize]) -> [f32; 2] {
    if pool.is_empty() {
        return [0.0, 0.0];
    }
    let n = pool.len() as f32;
    [
        pool.iter().map(|&i| labs[i][1]).sum::<f32>() / n,
        pool.iter().map(|&i| labs[i][2]).sum::<f32>() / n,
    ]
}

fn cast_of(labs: &[[f32; 3]]) -> [f32; 2] {
    let (pool, _) = neutral_pool(labs);
    cast_on(labs, &pool)
}

/// The controls auto adjust owns, in document units.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct AutoControls {
    pub exposure: f32,
    pub temperature: f32,
    pub tint: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
}

impl AutoControls {
    /// `base` with these controls written in.
    pub fn applied_to(&self, base: &GradeEdit) -> GradeEdit {
        let mut edit = base.clone();
        edit.grade.exposure = self.exposure;
        edit.temperature = self.temperature;
        edit.grade.tint = self.tint;
        edit.grade.whites = self.whites;
        edit.grade.blacks = self.blacks;
        edit.grade.vibrance = self.vibrance;
        edit
    }

    fn scaled(self, amount: f32) -> Self {
        let k = amount.clamp(0.0, 1.0);
        Self {
            exposure: self.exposure * k,
            temperature: self.temperature * k,
            tint: self.tint * k,
            whites: self.whites * k,
            blacks: self.blacks * k,
            vibrance: self.vibrance * k,
        }
    }
}

/// What [`auto_adjust`] made and what it measured.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AutoResult {
    pub controls: AutoControls,
    pub edit: GradeEdit,
    pub before: Measure,
    pub after: Measure,
}

/// Balance `samples` (the clip's source pixels): `base` is the clip's grade
/// now, whose other controls stay; `lut` its parsed look, if it has one;
/// `amount` (`0..1`) how far towards the full correction to go.
pub fn auto_adjust(
    samples: &Samples,
    base: &GradeEdit,
    lut: Option<&Cube>,
    amount: f32,
) -> AutoResult {
    let start = AutoControls::default().applied_to(base);
    let graded = samples.graded(&start, lut);
    let labs: Vec<[f32; 3]> = graded.pixels.iter().map(|&p| lab(p)).collect();
    let (pool, trust) = neutral_pool(&labs);
    let before = Measure::on_pool(&graded, &pool);
    let mut c = AutoControls::default();

    // 1. Exposure, in light: move the log-average towards mid grey, but not
    //    so far that the brightest 2 % clip.
    if before.key > 0.0 {
        let mut e = (TARGET_KEY / before.key).log2() * EXPOSURE_SHARE;
        if e > 0.0 {
            let mut lights: Vec<f32> = samples.pixels.iter().map(|&p| luminance(p)).collect();
            let bright = quantile(&mut lights, 0.98).max(1e-4);
            e = e.min((1.0 / bright).log2().max(0.0));
        }
        c.exposure = e.clamp(-MAX_EXPOSURE, MAX_EXPOSURE);
    }

    // 2. White balance: temperature and tint against the cast, by Newton
    //    steps on the simulated picture (the two controls are not
    //    orthogonal in a*/b*, and exposure moved the cast a little).
    let labs_of = |c: &AutoControls| -> Vec<[f32; 3]> {
        samples
            .graded(&c.applied_to(base), lut)
            .pixels
            .iter()
            .map(|&p| lab(p))
            .collect()
    };
    let cast_with = |c: &AutoControls| cast_on(&labs_of(c), &pool);
    let first = cast_with(&AutoControls::default());
    let goal = first.map(|v| v * (1.0 - BALANCE_SHARE * trust));
    // Without real greys the cast is a guess: never push far on a guess.
    let limit = if trust >= 1.0 { 0.8 } else { 0.4 };
    let balance = |c: &mut AutoControls| {
        for _ in 0..6 {
            let now = cast_with(c);
            let r = [now[0] - goal[0], now[1] - goal[1]];
            if r[0].abs() < 0.3 && r[1].abs() < 0.3 {
                break;
            }
            let h = 0.05;
            let probe = |dt: f32, dn: f32| {
                let mut p = *c;
                p.temperature += dt;
                p.tint += dn;
                cast_with(&p)
            };
            let ct = probe(h, 0.0);
            let cn = probe(0.0, h);
            // Jacobian columns: d(a, b)/d(temperature), d(a, b)/d(tint).
            let j = [
                [(ct[0] - now[0]) / h, (cn[0] - now[0]) / h],
                [(ct[1] - now[1]) / h, (cn[1] - now[1]) / h],
            ];
            let det = j[0][0] * j[1][1] - j[0][1] * j[1][0];
            if det.abs() < 1e-3 {
                break;
            }
            let dt = (-r[0] * j[1][1] + r[1] * j[0][1]) / det;
            let dn = (-r[1] * j[0][0] + r[0] * j[1][0]) / det;
            c.temperature = (c.temperature + dt).clamp(-limit, limit);
            c.tint = (c.tint + dn).clamp(-limit, limit);
        }
    };
    balance(&mut c);

    // 3. Black and white points: a levels stretch of what the picture holds
    //    after exposure and balance, never a compression.
    let now = Measure::of(&samples.graded(&c.applied_to(base), lut));
    let lo = if now.black > BLACK_TARGET + 0.02 {
        now.black
    } else {
        BLACK_TARGET
    };
    let hi = if now.white < WHITE_TARGET - 0.02 {
        now.white
    } else {
        WHITE_TARGET
    };
    if hi > lo {
        let span = ((hi - lo) / (WHITE_TARGET - BLACK_TARGET)).max(MIN_SPAN);
        let black = lo - BLACK_TARGET * span;
        let white = black + span;
        // `render::grade`: white = 1 - 0.25 whites, black = -0.25 blacks.
        c.blacks = (-black / 0.25).clamp(-1.0, 1.0);
        c.whites = ((1.0 - white) / 0.25).clamp(-1.0, 1.0);
    }

    // The stretch scales whatever cast is left with the tones; balance
    // again with the levels in place.
    balance(&mut c);

    // 4. A dull picture gets a little vibrance, which spares what is
    //    colourful already.
    let now = Measure::of(&samples.graded(&c.applied_to(base), lut));
    if now.chroma < DULL_CHROMA {
        c.vibrance = ((DULL_CHROMA - now.chroma) / 40.0).clamp(0.0, 0.3);
    }

    let controls = c.scaled(amount);
    let edit = controls.applied_to(base);
    let after = Measure::on_pool(&samples.graded(&edit, lut), &pool);
    AutoResult {
        controls,
        edit,
        before,
        after,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::modules::grading::pixels::{linear_to_srgb, srgb_to_linear};

    /// A synthetic scene: a grey ramp, skin, sky and foliage, in light.
    pub(crate) fn scene() -> Samples {
        let mut pixels = Vec::new();
        for i in 0..200 {
            let v = 0.02 + 0.96 * i as f32 / 199.0;
            pixels.push([v, v, v]);
        }
        for &(c, n) in &[
            ([0.82, 0.62, 0.52], 120),
            ([0.35, 0.55, 0.85], 160),
            ([0.25, 0.45, 0.2], 120),
            ([0.9, 0.85, 0.75], 40),
        ] {
            for k in 0..n {
                let t = 0.85 + 0.3 * (k as f32 / n as f32);
                pixels.push(c.map(|v: f32| (v * t).min(1.0)));
            }
        }
        Samples { pixels }
    }

    /// The scene filmed badly: underexposed by `stops`, a warm/green cast,
    /// and a lifted, compressed range.
    fn spoiled(scene: &Samples, stops: f32) -> Samples {
        let k = (-stops).exp2();
        Samples {
            pixels: scene
                .pixels
                .iter()
                .map(|&c| {
                    let lin = c.map(srgb_to_linear);
                    let cast = [lin[0] * 1.3, lin[1] * 1.08, lin[2] * 0.7];
                    cast.map(|v| 0.08 + 0.8 * linear_to_srgb(v * k))
                })
                .collect(),
        }
    }

    #[test]
    fn a_dark_cast_flat_shot_is_brightened_balanced_and_stretched() {
        let scene = scene();
        let bad = spoiled(&scene, 1.2);
        let result = auto_adjust(&bad, &GradeEdit::identity(), None, 1.0);
        let (b, a) = (result.before, result.after);
        assert!(result.controls.exposure > 0.3, "{:?}", result.controls);
        assert!(a.key > b.key * 1.3, "key {} -> {}", b.key, a.key);
        // The cast: warm (b* up) and green-ish, mostly gone.
        let cast = |m: &Measure| m.cast[0].hypot(m.cast[1]);
        assert!(cast(&b) > 4.0, "the fixture has a cast: {:?}", b.cast);
        assert!(cast(&a) < cast(&b) * 0.35, "{:?} -> {:?}", b.cast, a.cast);
        // The lifted blacks come down, the range widens.
        assert!(a.black < b.black - 0.04, "black {} -> {}", b.black, a.black);
        assert!(a.white - a.black > (b.white - b.black) * 1.15);
    }

    #[test]
    fn a_good_picture_is_barely_touched_and_amount_scales() {
        let scene = scene();
        let result = auto_adjust(&scene, &GradeEdit::identity(), None, 1.0);
        let c = result.controls;
        assert!(c.exposure.abs() < 0.6, "{c:?}");
        assert!(c.temperature.abs() < 0.15 && c.tint.abs() < 0.15, "{c:?}");
        let half = auto_adjust(&spoiled(&scene, 1.0), &GradeEdit::identity(), None, 0.5);
        let full = auto_adjust(&spoiled(&scene, 1.0), &GradeEdit::identity(), None, 1.0);
        assert!((half.controls.exposure - full.controls.exposure * 0.5).abs() < 1e-4);
        let none = auto_adjust(&spoiled(&scene, 1.0), &GradeEdit::identity(), None, 0.0);
        assert!(none.edit.is_identity());
    }

    #[test]
    fn the_users_other_controls_stay() {
        let mut base = GradeEdit::identity();
        base.contrast = 1.3;
        base.grade.hsl.bands[2].saturation = 0.4;
        base.grade.exposure = -3.0;
        let result = auto_adjust(&spoiled(&scene(), 1.0), &base, None, 1.0);
        assert_eq!(result.edit.contrast, 1.3);
        assert_eq!(result.edit.grade.hsl.bands[2].saturation, 0.4);
        // Exposure is auto adjust's own: measured afresh, not added to -3.
        assert!(result.edit.grade.exposure > 0.0);
    }
}
