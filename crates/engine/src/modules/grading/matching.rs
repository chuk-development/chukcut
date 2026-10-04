//! Colour match: grade one clip so its colour statistics match another's.
//!
//! The transfer is made in CIE L*a*b*, where distances are roughly what the
//! eye sees (Reinhard et al., "Color Transfer between Images", 2001: match
//! each channel's mean and spread). It is written into the clip's ordinary
//! controls rather than into an opaque filter (`docs/research/ml-features.md`
//! §3.14), in two steps:
//!
//! 1. **The sliders** — exposure, contrast, saturation, temperature, tint —
//!    are fitted by damped Gauss–Newton steps so the graded clip's L*a*b*
//!    means and spreads meet the reference's. The residual is measured on the
//!    picture as the shader would draw it (`pixels::Simulation`).
//! 2. **The red, green and blue curves** take what the sliders cannot: the
//!    *shape* of each channel's histogram, matched at a handful of quantiles
//!    (histogram specification, with few enough points that the user can
//!    still drag them). Only when the clip has no LUT and no fade, the two
//!    stages after the curves that would bend the match again; and only if
//!    the curves bring the statistics closer, which they almost always do.
//!
//! The reference is taken *as seen*: its own grade applied.

use serde::Serialize;

use super::pixels::{quantile, LabStats, Samples};
use crate::modules::inspector::edit::GradeEdit;
use crate::modules::render::lut::Cube;

/// The quantiles the curves are matched at. Seven inner points: the shape
/// of a histogram, and still a curve a person can edit by hand.
const CURVE_QUANTILES: [f32; 7] = [0.02, 0.1, 0.25, 0.5, 0.75, 0.9, 0.98];
/// Curve points closer than this in `x` are merged; a steeper step than
/// that between two quantiles is noise in the samples, not a look.
const MIN_GAP: f32 = 0.03;

/// The controls colour match owns.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct MatchControls {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
}

impl Default for MatchControls {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
        }
    }
}

impl MatchControls {
    fn to_vec(self) -> [f32; 5] {
        [
            self.exposure,
            self.contrast,
            self.saturation,
            self.temperature,
            self.tint,
        ]
    }

    fn from_vec(v: [f32; 5]) -> Self {
        Self {
            exposure: v[0].clamp(-3.0, 3.0),
            contrast: v[1].clamp(0.3, 2.0),
            saturation: v[2].clamp(0.0, 2.0),
            temperature: v[3].clamp(-1.0, 1.0),
            tint: v[4].clamp(-1.0, 1.0),
        }
    }

    /// `base` with these controls written in, the brightness slider and the
    /// colour curves cleared (match owns both: brightness would fight the
    /// exposure fit, and old curves would bend the new match).
    pub fn applied_to(&self, base: &GradeEdit) -> GradeEdit {
        let mut edit = base.clone();
        edit.brightness = 0.0;
        edit.grade.exposure = self.exposure;
        edit.contrast = self.contrast;
        edit.saturation = self.saturation;
        edit.temperature = self.temperature;
        edit.grade.tint = self.tint;
        edit.grade.curves.red.clear();
        edit.grade.curves.green.clear();
        edit.grade.curves.blue.clear();
        edit
    }

    fn scaled(self, amount: f32) -> Self {
        let k = amount.clamp(0.0, 1.0);
        let rest = Self::default().to_vec();
        let v = self.to_vec();
        Self::from_vec(std::array::from_fn(|i| rest[i] + (v[i] - rest[i]) * k))
    }
}

/// What [`colour_match`] made and how close it got.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MatchResult {
    pub controls: MatchControls,
    /// Whether the colour curves are part of the match.
    pub curves: bool,
    pub edit: GradeEdit,
    pub reference: LabStats,
    pub before: LabStats,
    pub after: LabStats,
    /// [`LabStats::distance`] to the reference, before and after.
    pub distance_before: f32,
    pub distance_after: f32,
}

/// The residual the slider fit drives to zero: the differences of the L*
/// mean and spread, the a* and b* means, and the mean chroma spread.
fn residual(stats: &LabStats, reference: &LabStats) -> [f32; 5] {
    [
        stats.mean[0] - reference.mean[0],
        stats.std[0] - reference.std[0],
        stats.mean[1] - reference.mean[1],
        stats.mean[2] - reference.mean[2],
        0.5 * (stats.std[1] + stats.std[2]) - 0.5 * (reference.std[1] + reference.std[2]),
    ]
}

fn norm(r: &[f32; 5]) -> f32 {
    r.iter().map(|v| v * v).sum::<f32>().sqrt()
}

/// Solve the 5×5 system `a x = b` by Gaussian elimination with partial
/// pivoting. `None` when it is singular.
// Index loops read as the textbook elimination; iterators would hide which
// row and column each step touches.
#[allow(clippy::needless_range_loop)]
fn solve(mut a: [[f32; 5]; 5], mut b: [f32; 5]) -> Option<[f32; 5]> {
    for col in 0..5 {
        let pivot = (col..5).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-9 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..5 {
            let f = a[row][col] / a[col][col];
            for k in col..5 {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0f32; 5];
    for row in (0..5).rev() {
        let mut s = b[row];
        for k in row + 1..5 {
            s -= a[row][k] * x[k];
        }
        x[row] = s / a[row][row];
    }
    Some(x)
}

/// The slider fit: damped Gauss–Newton (Levenberg–Marquardt) with a
/// numerical Jacobian. Five unknowns, five residuals, a dozen steps at most;
/// each step simulates the grade six times over the samples.
fn fit_sliders(
    target: &Samples,
    base: &GradeEdit,
    lut: Option<&Cube>,
    reference: &LabStats,
) -> MatchControls {
    let stats_of = |c: MatchControls| LabStats::of(&target.graded(&c.applied_to(base), lut));
    let mut controls = MatchControls::default();
    let mut r = residual(&stats_of(controls), reference);
    let mut lambda = 1e-2f32;
    // Step sizes for the derivatives, per control, in its own units.
    let h = [0.05, 0.03, 0.03, 0.03, 0.03];
    for _ in 0..12 {
        if norm(&r) < 0.25 {
            break;
        }
        let v = controls.to_vec();
        let mut j = [[0f32; 5]; 5];
        for (k, step) in h.iter().enumerate() {
            let mut p = v;
            p[k] += step;
            let rk = residual(&stats_of(MatchControls::from_vec(p)), reference);
            for i in 0..5 {
                j[i][k] = (rk[i] - r[i]) / step;
            }
        }
        // (JᵀJ + λ diag(JᵀJ)) δ = −Jᵀr
        let mut jtj = [[0f32; 5]; 5];
        let mut jtr = [0f32; 5];
        for a in 0..5 {
            for b in 0..5 {
                jtj[a][b] = (0..5).map(|i| j[i][a] * j[i][b]).sum();
            }
            jtr[a] = -(0..5).map(|i| j[i][a] * r[i]).sum::<f32>();
        }
        let mut improved = false;
        for _ in 0..6 {
            let mut m = jtj;
            for (d, row) in m.iter_mut().enumerate() {
                row[d] += lambda * jtj[d][d].max(1e-3);
            }
            let Some(delta) = solve(m, jtr) else {
                lambda *= 10.0;
                continue;
            };
            let candidate = MatchControls::from_vec(std::array::from_fn(|i| v[i] + delta[i]));
            let rc = residual(&stats_of(candidate), reference);
            if norm(&rc) < norm(&r) {
                controls = candidate;
                r = rc;
                lambda = (lambda * 0.3).max(1e-4);
                improved = true;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    controls
}

/// Per-channel histogram specification as curve points: for each channel,
/// the target's quantiles on `x` and the reference's on `y`.
fn curve_points(target: &Samples, reference: &Samples, channel: usize) -> Vec<[f32; 2]> {
    let mut t: Vec<f32> = target.pixels.iter().map(|c| c[channel]).collect();
    let mut r: Vec<f32> = reference.pixels.iter().map(|c| c[channel]).collect();
    let mut points: Vec<[f32; 2]> = Vec::new();
    for &q in &CURVE_QUANTILES {
        let x = quantile(&mut t, q);
        let y = quantile(&mut r, q);
        match points.last() {
            Some(last) if x - last[0] < MIN_GAP => continue,
            _ => points.push([x, y]),
        }
    }
    // Monotone in y as well: a curve that falls inverts tones.
    for i in 1..points.len() {
        if points[i][1] < points[i - 1][1] {
            points[i][1] = points[i - 1][1];
        }
    }
    // Pinned ends, so the darkest and brightest few percent follow the
    // nearest point's slope instead of being clamped flat by the curve.
    if points.first().is_some_and(|p| p[0] > MIN_GAP) {
        let [x0, y0] = points[0];
        points.insert(0, [0.0, (y0 - x0).max(0.0).min(y0)]);
    }
    if points.last().is_some_and(|p| p[0] < 1.0 - MIN_GAP) {
        let [x1, y1] = *points.last().expect("checked");
        points.push([1.0, (y1 + (1.0 - x1)).min(1.0).max(y1)]);
    }
    points
}

/// Match `target` (the clip's source pixels; `base` its grade now, `lut` its
/// parsed look) to `reference` (the reference's pixels *as seen*), by
/// `amount` (`0..1`).
pub fn colour_match(
    target: &Samples,
    base: &GradeEdit,
    lut: Option<&Cube>,
    reference: &Samples,
    amount: f32,
) -> MatchResult {
    let reference_stats = LabStats::of(reference);
    let before = LabStats::of(&target.graded(base, lut));
    let fitted = fit_sliders(target, base, lut, &reference_stats);

    let mut edit = fitted.applied_to(base);
    let mut curves = false;
    if base.lut.is_none() && base.grade.fade == 0.0 {
        let sliders_only = target.graded(&edit, lut);
        let mut with_curves = edit.clone();
        with_curves.grade.curves.red = curve_points(&sliders_only, reference, 0);
        with_curves.grade.curves.green = curve_points(&sliders_only, reference, 1);
        with_curves.grade.curves.blue = curve_points(&sliders_only, reference, 2);
        let d_sliders = LabStats::of(&sliders_only).distance(&reference_stats);
        let d_curves = LabStats::of(&target.graded(&with_curves, lut)).distance(&reference_stats);
        if d_curves < d_sliders {
            edit = with_curves;
            curves = true;
        }
    }

    // The amount pulls the sliders and the curves towards rest together.
    let controls = fitted.scaled(amount);
    let k = amount.clamp(0.0, 1.0);
    let mut scaled = controls.applied_to(base);
    if curves && k > 0.0 {
        let ease = |points: &[[f32; 2]]| -> Vec<[f32; 2]> {
            points
                .iter()
                .map(|&[x, y]| [x, (x + (y - x) * k).clamp(0.0, 1.0)])
                .collect()
        };
        scaled.grade.curves.red = ease(&edit.grade.curves.red);
        scaled.grade.curves.green = ease(&edit.grade.curves.green);
        scaled.grade.curves.blue = ease(&edit.grade.curves.blue);
    }
    let after = LabStats::of(&target.graded(&scaled, lut));
    MatchResult {
        controls,
        curves,
        distance_before: before.distance(&reference_stats),
        distance_after: after.distance(&reference_stats),
        edit: scaled,
        reference: reference_stats,
        before,
        after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::grading::auto::tests::scene;
    use crate::modules::grading::pixels::{linear_to_srgb, srgb_to_linear};

    fn transformed(s: &Samples, f: impl Fn([f32; 3]) -> [f32; 3]) -> Samples {
        Samples {
            pixels: s.pixels.iter().map(|&c| f(c)).collect(),
        }
    }

    #[test]
    fn a_cool_dark_clip_is_matched_to_a_warm_bright_reference() {
        let scene = scene();
        // The reference: warm, bright, a touch more saturated.
        let reference = transformed(&scene, |c| {
            let l = c.map(srgb_to_linear);
            [l[0] * 1.25, l[1] * 1.1, l[2] * 0.85].map(linear_to_srgb)
        });
        // The clip: the same scene, cool and a stop darker, flatter.
        let target = transformed(&scene, |c| {
            let l = c.map(srgb_to_linear);
            [l[0] * 0.4, l[1] * 0.5, l[2] * 0.65].map(|v| 0.05 + 0.85 * linear_to_srgb(v))
        });
        let result = colour_match(&target, &GradeEdit::identity(), None, &reference, 1.0);
        assert!(
            result.distance_before > 15.0,
            "the fixture differs: {}",
            result.distance_before
        );
        assert!(
            result.distance_after < result.distance_before * 0.15,
            "{} -> {}",
            result.distance_before,
            result.distance_after
        );
        assert!(result.controls.temperature > 0.05, "{:?}", result.controls);
        assert!(result.controls.exposure > 0.2, "{:?}", result.controls);
    }

    #[test]
    fn matching_a_clip_to_itself_changes_next_to_nothing() {
        let scene = scene();
        let result = colour_match(&scene, &GradeEdit::identity(), None, &scene, 1.0);
        assert!(result.distance_after < 0.5, "{}", result.distance_after);
        let c = result.controls;
        assert!(
            c.exposure.abs() < 0.05 && (c.contrast - 1.0).abs() < 0.05,
            "{c:?}"
        );
    }

    #[test]
    fn the_curves_stay_monotone_and_amount_zero_is_rest() {
        let scene = scene();
        let reference = transformed(&scene, |c| c.map(|v| v * v));
        let result = colour_match(&scene, &GradeEdit::identity(), None, &reference, 1.0);
        for points in [
            &result.edit.grade.curves.red,
            &result.edit.grade.curves.green,
            &result.edit.grade.curves.blue,
        ] {
            for w in points.windows(2) {
                assert!(w[1][0] > w[0][0] && w[1][1] >= w[0][1], "{points:?}");
            }
        }
        let none = colour_match(&scene, &GradeEdit::identity(), None, &reference, 0.0);
        assert!(none.edit.grade.curves.is_identity());
        assert_eq!(none.controls, MatchControls::default());
    }

    #[test]
    fn a_lut_on_the_clip_keeps_the_match_to_the_sliders() {
        let scene = scene();
        let reference = transformed(&scene, |c| c.map(|v| (v * 1.2).min(1.0)));
        let mut base = GradeEdit::identity();
        base.lut = Some(crate::modules::project::document::LutRef {
            path: "/nowhere.cube".into(),
            intensity: 1.0,
        });
        let cube = crate::modules::render::lut::parse(
            &crate::modules::render::lut::fixtures::identity_cube(17),
        )
        .unwrap();
        let result = colour_match(&scene, &base, Some(&cube), &reference, 1.0);
        assert!(!result.curves);
        assert!(result.edit.lut.is_some());
        assert!(result.distance_after < result.distance_before);
    }
}
