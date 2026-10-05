//! Video stabilisation: measure how the camera shook, then hold the picture
//! still in the compositor.
//!
//! Two passes, as vid.stab does it and as `docs/research/ml-features.md`
//! §3.10 lays out, but on our own feature tracking — the tracking module's
//! KLT and RANSAC similarity fit — so the result is a transform our
//! compositor applies, and the preview and the export are the same pixels:
//!
//! 1. **Measure** ([`measure`]): corners in each frame, followed into the
//!    next with pyramidal Lucas–Kanade; a similarity fit over the matches
//!    (RANSAC, so a person walking through the shot is outvoted by the
//!    background) is the camera's motion between the two frames. Summed, it
//!    is the camera path, stored in the document ([`CameraPath`]) and cached.
//! 2. **Smooth and apply** ([`resolve`]): a Gaussian over the path, as wide
//!    as the strength asks, is where the camera *should* have been; the
//!    difference is the correction. The compositor moves the clip's crop
//!    window against the shake and turns the clip against the roll, zoomed in
//!    just enough that the moving edges never show.
//!
//! Applying is a crop and a transform — no pixel is re-rendered into a cache
//! — so the strength and the crop change instantly, and nothing has to be
//! rebuilt before an export.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::cache;
use super::frames::{luma, walk, Walk};
use super::jobs::JobContext;
use super::scenes::{self, Scores, Signature};
use super::store::{self, CameraPath, PathSample, Stabilise};
use crate::modules::project::document::{
    AnimatableProperty, Crop, Id, Micros, Project, Segment, TimeRange,
};
use crate::modules::render::layout::animated_transform;
use crate::modules::tracking::klt::{good_features, track_point, FlowParams, Gray, Pyramid};
use crate::modules::tracking::tracker::{fit_similarity, Pair, Similarity};

/// Height of the frames the camera motion is measured on.
pub const ANALYSIS_HEIGHT: u32 = 270;
const LEVELS: usize = 3;
const MAX_POINTS: usize = 240;
/// Cells the frame is split into for picking corners, columns × rows.
const GRID: (usize, usize) = (6, 4);
/// Bump when [`measure`] changes what it writes.
const ALGORITHM: &str = "camera-path-v4";
/// The most a clip is zoomed in to hide moving edges, as a fraction cropped.
pub const MAX_CROP: f32 = 0.3;

/// Strength presets the inspector offers.
pub const STRENGTHS: [(&str, f32); 4] = [
    ("Light", 0.35),
    ("Medium", 0.6),
    ("Strong", 0.85),
    ("Tripod", 1.0),
];

/// Crop choices the inspector offers; `None` is automatic.
pub const CROPS: [(&str, Option<f32>); 4] = [
    ("Auto", None),
    ("5 %", Some(0.05)),
    ("10 %", Some(0.1)),
    ("20 %", Some(0.2)),
];

/// The camera's motion from one frame to the next, from point matches, in
/// pixels about the frame's centre. `None` when too little matched to say.
#[derive(Default)]
pub struct MotionEstimator {
    params: FlowParams,
}

impl MotionEstimator {
    pub fn pyramid(gray: Gray) -> Pyramid {
        Pyramid::new(gray, LEVELS)
    }

    pub fn estimate(&self, prev: &Pyramid, next: &Pyramid) -> Option<Similarity> {
        let (w, h) = (prev.width() as f32, prev.height() as f32);
        // Corners from every part of the frame, not just the busiest: the
        // strongest corners of a frame cluster on whatever has the most
        // detail, and that is often the thing moving through the shot rather
        // than the background the camera's motion is read from.
        let margin = 8.0;
        let (cols, rows) = (GRID.0 as f32, GRID.1 as f32);
        let (cell_w, cell_h) = ((w - 2.0 * margin) / cols, (h - 2.0 * margin) / rows);
        let mut points = Vec::with_capacity(MAX_POINTS);
        for row in 0..GRID.1 {
            for col in 0..GRID.0 {
                let x0 = margin + col as f32 * cell_w;
                let y0 = margin + row as f32 * cell_h;
                points.extend(good_features(
                    &prev.levels[0],
                    (x0, y0, x0 + cell_w, y0 + cell_h),
                    MAX_POINTS / (GRID.0 * GRID.1),
                    0.02,
                    (w.min(h) / 30.0).max(5.0),
                ));
            }
        }
        if points.len() < 8 {
            return None;
        }
        let (cx, cy) = (w * 0.5, h * 0.5);
        let pairs: Vec<Pair> = points
            .iter()
            .filter_map(|&p| {
                let q = track_point(prev, next, p, (0.0, 0.0), &self.params)?;
                Some(((p.0 - cx, p.1 - cy), (q.0 - cx, q.1 - cy)))
            })
            .collect();
        if pairs.len() < 8 {
            return None;
        }
        // Consensus on a plain shift first: a thing turning or sweeping
        // through the shot agrees with itself on a rotation, and a similarity
        // RANSAC can pick that. It cannot agree with the background on a
        // shift. The turn is then read from the points that moved with the
        // background only.
        let background = shift_consensus(&pairs, 2.0);
        if background.len() < 8 {
            return None;
        }
        let fit = fit_similarity(&background, 1.0)?;
        // A fit on a handful of points is a guess; a frame that guessed a
        // jump would put a jolt into a stabilised clip.
        (fit.inliers * 3 >= pairs.len() && fit.inliers >= 6).then_some(fit)
    }
}

/// The pairs that agree with the most common shift, within `threshold`
/// pixels. Deterministic: every pair's own shift is a candidate.
fn shift_consensus(pairs: &[Pair], threshold: f32) -> Vec<Pair> {
    let t2 = threshold * threshold;
    let agree = |shift: (f32, f32)| {
        pairs
            .iter()
            .filter(|(p, q)| {
                let (dx, dy) = (q.0 - p.0 - shift.0, q.1 - p.1 - shift.1);
                dx * dx + dy * dy <= t2
            })
            .count()
    };
    let step = (pairs.len() / 64).max(1);
    let best = pairs
        .iter()
        .step_by(step)
        .map(|(p, q)| (q.0 - p.0, q.1 - p.1))
        .max_by_key(|&shift| agree(shift));
    let Some(shift) = best else {
        return Vec::new();
    };
    pairs
        .iter()
        .copied()
        .filter(|(p, q)| {
            let (dx, dy) = (q.0 - p.0 - shift.0, q.1 - p.1 - shift.1);
            dx * dx + dy * dy <= t2
        })
        .collect()
}

/// Measure the camera path over `job`. Cached by file, range and algorithm.
pub fn measure(job: &Walk, media_id: &str, ctx: Option<&JobContext>) -> Result<CameraPath, String> {
    let key = cache::key(ALGORITHM, &job.path, job.range);
    if let Some(mut cached) = key.as_deref().and_then(cache::load::<CameraPath>) {
        cached.media_id = media_id.to_string();
        return Ok(cached);
    }
    let estimator = MotionEstimator::default();
    let mut samples: Vec<PathSample> = Vec::new();
    let mut previous: Option<Pyramid> = None;
    let mut signature: Option<Signature> = None;
    let mut scores = Scores {
        fps: job.fps,
        ..Default::default()
    };
    let mut aspect = 16.0 / 9.0;
    let (mut x, mut y, mut a) = (0.0f32, 0.0f32, 0.0f32);
    walk(job, ctx, (0.0, 1.0), |frame| {
        aspect = frame.width as f32 / frame.height.max(1) as f32;
        let gray = Gray {
            width: frame.width,
            height: frame.height,
            data: luma(&frame.rgba),
        };
        let pyramid = MotionEstimator::pyramid(gray);
        let next = Signature::of(&frame.rgba, frame.width, frame.height);
        scores.times.push(frame.pts);
        scores
            .scores
            .push(signature.as_ref().map_or(0.0, |p| next.distance(p)));
        signature = Some(next);
        if let Some(prev) = &previous {
            if let Some(motion) = estimator.estimate(prev, &pyramid) {
                x += motion.tx / frame.width as f32;
                y += motion.ty / frame.height as f32;
                a += motion.angle_degrees();
            }
        }
        samples.push(PathSample {
            t: frame.pts,
            x,
            y,
            a,
            cut: false,
        });
        previous = Some(pyramid);
        Ok(())
    })?;
    if samples.is_empty() {
        return Err("the clip has no frames to analyse".into());
    }
    for i in scenes::detect(&scores, 0.5) {
        if let Some(sample) = samples.get_mut(i) {
            sample.cut = true;
        }
    }
    let path = CameraPath {
        media_id: media_id.to_string(),
        analysed: job.range,
        aspect,
        samples,
    };
    if let Some(key) = key {
        cache::store(&key, &path);
    }
    Ok(path)
}

/// How far the picture has to move at one instant to cancel the shake.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Correction {
    pub t: Micros,
    /// Fractions of the frame, +x right, +y down.
    pub x: f32,
    pub y: f32,
    /// Degrees, clockwise.
    pub a: f32,
}

/// The Gaussian width, in seconds, that a strength `0..1` stands for. At 1
/// the camera is locked: there is no width, the target is the mean.
fn sigma_seconds(strength: f32) -> Option<f32> {
    let strength = strength.clamp(0.0, 1.0);
    (strength < 0.999).then(|| 0.04 * 60f32.powf(strength))
}

/// The correction at every sample of `path` for `strength`, smoothed within
/// each shot: a cut is a jump no camera made, and smoothing across it would
/// drag the frames on both sides towards each other.
pub fn corrections(path: &CameraPath, strength: f32) -> Vec<Correction> {
    let samples = &path.samples;
    let mut out = Vec::with_capacity(samples.len());
    let mut start = 0;
    for i in 1..=samples.len() {
        if i == samples.len() || samples[i].cut {
            out.extend(shot_corrections(&samples[start..i], strength));
            start = i;
        }
    }
    out
}

fn shot_corrections(samples: &[PathSample], strength: f32) -> Vec<Correction> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    let target: Vec<(f32, f32, f32)> = match sigma_seconds(strength) {
        None => {
            let mean = samples.iter().fold((0.0, 0.0, 0.0), |acc, s| {
                (acc.0 + s.x, acc.1 + s.y, acc.2 + s.a)
            });
            let k = 1.0 / n as f32;
            vec![(mean.0 * k, mean.1 * k, mean.2 * k); n]
        }
        Some(sigma) => {
            let span = (samples[n - 1].t - samples[0].t).max(1) as f32 / 1e6;
            let per_second = if n > 1 { (n - 1) as f32 / span } else { 30.0 };
            let sigma = (sigma * per_second).max(0.5);
            let radius = (sigma * 3.0).ceil() as usize;
            let weights: Vec<f32> = (0..=radius)
                .map(|d| (-(d as f32).powi(2) / (2.0 * sigma * sigma)).exp())
                .collect();
            (0..n)
                .map(|i| {
                    let (mut sx, mut sy, mut sa, mut sw) = (0.0, 0.0, 0.0, 0.0);
                    let lo = i.saturating_sub(radius);
                    let hi = (i + radius).min(n - 1);
                    for (j, s) in samples.iter().enumerate().take(hi + 1).skip(lo) {
                        let w = weights[i.abs_diff(j)];
                        sx += w * s.x;
                        sy += w * s.y;
                        sa += w * s.a;
                        sw += w;
                    }
                    (sx / sw, sy / sw, sa / sw)
                })
                .collect()
        }
    };
    samples
        .iter()
        .zip(target)
        .map(|(s, (tx, ty, ta))| Correction {
            t: s.t,
            x: tx - s.x,
            y: ty - s.y,
            a: ta - s.a,
        })
        .collect()
}

/// How much bigger a `aspect`-shaped rectangle turned by `degrees` has to be
/// to cover the unturned one.
pub fn rotation_cover(degrees: f32, aspect: f32) -> f32 {
    let t = degrees.to_radians().abs().min(0.5);
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let (c, s) = (t.cos(), t.sin());
    (c + s * aspect).max(c + s / aspect)
}

/// The least crop, as a fraction, that hides the moving edges for all but
/// the worst one per cent of frames.
pub fn auto_crop(corrections: &[Correction], aspect: f32) -> f32 {
    if corrections.is_empty() {
        return 0.0;
    }
    let mut needed: Vec<f32> = corrections
        .iter()
        .map(|c| {
            let cover = rotation_cover(c.a, aspect);
            // The window is `k·cover` of the frame and has to stay inside it
            // while it moves by the correction.
            let k = ((1.0 - 2.0 * c.x.abs()) / cover).min((1.0 - 2.0 * c.y.abs()) / cover);
            1.0 - k
        })
        .collect();
    needed.sort_by(f32::total_cmp);
    let index = ((needed.len() as f32 * 0.99) as usize).min(needed.len() - 1);
    needed[index].clamp(0.0, MAX_CROP)
}

/// A stabilisation, ready for the compositor.
#[derive(Debug, Clone)]
pub struct Applied {
    pub enabled: bool,
    pub corrections: Vec<Correction>,
    /// Fraction cropped off.
    pub crop: f32,
    pub aspect: f32,
}

impl Applied {
    pub fn build(settings: &Stabilise, path: &CameraPath) -> Self {
        let corrections = corrections(path, settings.strength);
        let crop = settings
            .crop
            .unwrap_or_else(|| auto_crop(&corrections, path.aspect))
            .clamp(0.0, MAX_CROP);
        Self {
            enabled: settings.enabled,
            corrections,
            crop,
            aspect: path.aspect,
        }
    }

    /// The correction at source time `t`, interpolated, held at the ends.
    pub fn at(&self, t: Micros) -> Correction {
        let c = &self.corrections;
        if c.is_empty() {
            return Correction::default();
        }
        let i = c.partition_point(|s| s.t <= t);
        if i == 0 {
            return c[0];
        }
        if i >= c.len() {
            return c[c.len() - 1];
        }
        let (a, b) = (c[i - 1], c[i]);
        let span = (b.t - a.t).max(1) as f32;
        let f = ((t - a.t) as f32 / span).clamp(0.0, 1.0);
        Correction {
            t,
            x: a.x + (b.x - a.x) * f,
            y: a.y + (b.y - a.y) * f,
            a: a.a + (b.a - a.a) * f,
        }
    }

    /// The crop window and the extra scale and rotation that show the clip
    /// corrected at source time `t`, inside the clip's own crop `base`.
    pub fn window(&self, base: Crop, t: Micros) -> (Crop, f32, f32) {
        let c = self.at(t);
        let (bw, bh) = (base.right - base.left, base.bottom - base.top);
        let cover = rotation_cover(c.a, self.aspect);
        let k = ((1.0 - self.crop) * cover).min(1.0);
        let (w, h) = (bw * k, bh * k);
        // The picture moves by the correction, so the window moves against
        // it. The correction is in whole-frame fractions, like the crop.
        let clamp = |centre: f32, lo: f32, hi: f32, size: f32| {
            let (a, b) = (lo + size * 0.5, hi - size * 0.5);
            if a > b {
                (lo + hi) * 0.5
            } else {
                centre.clamp(a, b)
            }
        };
        let cx = clamp(
            (base.left + base.right) * 0.5 - c.x,
            base.left,
            base.right,
            w,
        );
        let cy = clamp(
            (base.top + base.bottom) * 0.5 - c.y,
            base.top,
            base.bottom,
            h,
        );
        let window = Crop {
            left: cx - w * 0.5,
            top: cy - h * 0.5,
            right: cx + w * 0.5,
            bottom: cy + h * 0.5,
        };
        (window, cover.min(1.0 / (1.0 - self.crop)), c.a)
    }
}

fn memo() -> &'static Mutex<HashMap<Id, Option<Arc<Applied>>>> {
    static MEMO: OnceLock<Mutex<HashMap<Id, Option<Arc<Applied>>>>> = OnceLock::new();
    MEMO.get_or_init(Default::default)
}

/// The stabilisation `segment` carries, built once per entry id. Entries are
/// immutable (`store.rs`), so the id is a complete key.
pub fn applied(project: &Project, segment: &Segment) -> Option<Arc<Applied>> {
    if project.materials.extras.is_empty() {
        return None;
    }
    let id = segment
        .extras
        .iter()
        .find(|id| store::kind_of(project, id) == Some(store::STABILISE))?;
    if let Some(hit) = memo().lock().get(id) {
        return hit.clone();
    }
    let built = store::entry::<Stabilise>(project, id).and_then(|settings| {
        let path = store::entry::<CameraPath>(project, &settings.motion_id)?;
        Some(Arc::new(Applied::build(&settings, &path)))
    });
    let mut memo = memo().lock();
    if memo.len() > 64 {
        memo.clear();
    }
    memo.insert(id.clone(), built.clone());
    built
}

/// `segment` as the compositor should draw it at `time`: itself, or a copy
/// whose crop window and transform hold the picture still. Borrowed when the
/// clip is not stabilised, so a project without stabilisation pays one map
/// lookup per clip and no allocation. The transform keyframes are folded into
/// the copy's transform, as `tracking::follow::resolve` does.
pub fn resolve<'a>(project: &Project, segment: Cow<'a, Segment>, time: Micros) -> Cow<'a, Segment> {
    let Some(applied) = applied(project, &segment) else {
        return segment;
    };
    if !applied.enabled {
        return segment;
    }
    // Through the time map: a stabilised clip on a speed curve must look
    // up the correction for the frame it actually shows.
    let source = project
        .materials
        .time_map(&segment)
        .clamped_source_time(time);
    // The crop shown at this instant: a keyframed crop moves the window's
    // frame with it.
    let base = segment.crop_at(time).unwrap_or_default();
    let (window, scale, roll) = applied.window(base, source);
    let mut transform = animated_transform(&segment, time);
    transform.scale[0] *= scale;
    transform.scale[1] *= scale;
    // A mirrored picture turns the other way.
    let mirrored = transform.flip_h != transform.flip_v;
    transform.rotation += if mirrored { -roll } else { roll };
    let mut copy = segment.into_owned();
    copy.crop = Some(window);
    copy.transform = transform;
    copy.keyframes
        .retain(|t| t.property == AnimatableProperty::Volume);
    Cow::Owned(copy)
}

/// The settings `segment` is stabilised with, if it is.
pub fn settings_of(project: &Project, segment: &Segment) -> Option<(Id, Stabilise)> {
    store::entry_of::<Stabilise>(project, segment)
}

/// The analysed range of a camera path, for "analysed up to here" notes.
pub fn analysed_range(project: &Project, settings: &Stabilise) -> Option<TimeRange> {
    store::entry::<CameraPath>(project, &settings.motion_id).map(|p| p.analysed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(xs: &[f32]) -> CameraPath {
        CameraPath {
            media_id: "m".into(),
            analysed: TimeRange::new(0, xs.len() as Micros * 33_333),
            aspect: 16.0 / 9.0,
            samples: xs
                .iter()
                .enumerate()
                .map(|(i, &x)| PathSample {
                    t: i as Micros * 33_333,
                    x,
                    y: -x * 0.5,
                    a: 0.0,
                    cut: false,
                })
                .collect(),
        }
    }

    #[test]
    fn tripod_cancels_all_motion_around_the_mean() {
        let p = path(&[0.0, 0.02, -0.02, 0.0]);
        let c = corrections(&p, 1.0);
        for (s, c) in p.samples.iter().zip(&c) {
            // Path plus correction is the same everywhere: the mean.
            assert!((s.x + c.x).abs() < 1e-6);
        }
    }

    #[test]
    fn smoothing_keeps_a_slow_pan_and_removes_jitter() {
        // A pan of 0.2 over 4 s with ±0.01 jitter every other frame.
        let xs: Vec<f32> = (0..120)
            .map(|i| 0.2 * i as f32 / 119.0 + if i % 2 == 0 { 0.01 } else { -0.01 })
            .collect();
        let p = path(&xs);
        let c = corrections(&p, 0.6);
        let mid = 60;
        let corrected: Vec<f32> = (mid - 5..mid + 5).map(|i| xs[i] + c[i].x).collect();
        // Frame to frame, the corrected path moves by the pan only.
        for pair in corrected.windows(2) {
            let step = pair[1] - pair[0];
            assert!((step - 0.2 / 119.0).abs() < 0.004, "step {step}");
        }
    }

    #[test]
    fn a_cut_splits_the_smoothing() {
        // Two still shots whose paths sit far apart: each is its own mean.
        let mut p = path(&[0.0, 0.0, 0.0, 0.3, 0.3, 0.3]);
        p.samples[3].cut = true;
        let c = corrections(&p, 1.0);
        assert!(c.iter().all(|c| c.x.abs() < 1e-6), "{c:?}");
    }

    #[test]
    fn the_window_moves_against_the_shake_and_stays_inside() {
        let p = path(&[0.0, 0.05]);
        let applied = Applied::build(
            &Stabilise {
                motion_id: String::new(),
                enabled: true,
                strength: 1.0,
                crop: Some(0.1),
            },
            &p,
        );
        // At the second frame the picture drifted right of the mean by
        // 0.025, so the correction is -0.025: the window moves right.
        let (w, scale, _) = applied.window(Crop::default(), 33_333);
        assert!(((w.left + w.right) * 0.5 - 0.525).abs() < 1e-4, "{w:?}");
        assert!((w.right - w.left - 0.9).abs() < 1e-4);
        assert!(w.left >= 0.0 && w.right <= 1.0 && w.top >= 0.0 && w.bottom <= 1.0);
        assert!((scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn auto_crop_is_just_enough() {
        let corrections = vec![
            Correction {
                t: 0,
                x: 0.04,
                y: 0.0,
                a: 0.0,
            };
            10
        ];
        assert!((auto_crop(&corrections, 1.0) - 0.08).abs() < 1e-4);
        let turned = vec![
            Correction {
                t: 0,
                x: 0.0,
                y: 0.0,
                a: 2.0,
            };
            10
        ];
        assert!(auto_crop(&turned, 16.0 / 9.0) > 0.0);
    }

    #[test]
    fn the_cover_of_no_turn_is_one() {
        assert!((rotation_cover(0.0, 1.78) - 1.0).abs() < 1e-6);
        assert!(rotation_cover(3.0, 1.78) > 1.05);
    }

    #[test]
    fn the_estimator_finds_a_known_shift() {
        // A textured image shifted by (3, -2) pixels.
        let (w, h) = (160usize, 90usize);
        let texture = |x: f32, y: f32| {
            128.0 + 60.0 * (x * 0.31).sin() * (y * 0.23).cos() + 40.0 * ((x + 2.0 * y) * 0.11).sin()
        };
        let mut a = Gray::new(w, h);
        let mut b = Gray::new(w, h);
        for y in 0..h {
            for x in 0..w {
                a.data[y * w + x] = texture(x as f32, y as f32);
                b.data[y * w + x] = texture(x as f32 - 3.0, y as f32 + 2.0);
            }
        }
        let e = MotionEstimator::default();
        let m = e
            .estimate(&MotionEstimator::pyramid(a), &MotionEstimator::pyramid(b))
            .expect("a fit");
        assert!(
            (m.tx - 3.0).abs() < 0.3 && (m.ty + 2.0).abs() < 0.3,
            "{m:?}"
        );
    }
}
