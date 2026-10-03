//! Speed curves: speed that changes inside one clip (a speed ramp).
//!
//! ## The model
//!
//! A curve is a list of points, each a **source instant** (microseconds into
//! the material, the same clock as `Segment::source_range`) and the speed the
//! clip plays at there. Between two points the speed moves smoothly; before
//! the first and after the last it holds.
//!
//! The points are anchored to the *material*, not to the clip's timeline
//! length, on purpose:
//!
//! - **A split is exact.** Both halves keep the same curve, and each plays
//!   its own part of it. Nothing has to be resampled at the cut, so the speed
//!   on either side of it is the speed the unsplit clip had there.
//! - **A trim does not stretch the ramp.** Trimming a tail drops the frames
//!   past the new end; the slow-motion in the middle stays where it was in the
//!   footage instead of being squeezed to fit.
//!
//! The clip's timeline length follows from the curve: playing a stretch of
//! source `dx` at speed `s` takes `dx / s` of timeline, so the length is the
//! integral of the *slowness* `1 / s` over the source range. To keep that
//! integral exact and cheap, the curve interpolates the slowness, not the
//! speed: between two points it runs along a smoothstep (`3u² − 2u³`), whose
//! integral is a polynomial. The timeline position of every source instant is
//! therefore closed-form, and the inverse (which source frame plays at a
//! timeline instant) is one Newton solve inside one piece. The smoothstep is
//! flat at every point, so the speed never overshoots the values the user set
//! and never reaches zero.
//!
//! ## Where it is stored
//!
//! In `MaterialPool::speed_curves`, referenced from the clip's `extras`, like a
//! colour adjustment. A pool category changes no existing segment,
//! constructor or test (the reason `MaterialPool::color_adjusts` gives), and it
//! lets both halves of a split share one curve without copying it. A curve is
//! never edited in place: `EditCommand::SetSpeedCurve` swaps the reference for
//! a material with a new id, so a curve two clips share cannot change under
//! the one that was not edited.
//!
//! While a clip has a curve, `Segment::speed` is dormant: it is kept, so that
//! removing the curve returns the clip to the constant speed it had, but no
//! mapping reads it. [`TimeMap`] is the one place that decides.

use serde::{Deserialize, Serialize};

use super::document::{Id, MaterialPool, Micros, Segment, TimeRange};

/// The slowest a curve may play: a tenth of real time, as CapCut allows.
pub const MIN_CURVE_SPEED: f32 = 0.1;
/// The fastest a curve may play.
pub const MAX_CURVE_SPEED: f32 = 10.0;

/// One control point of a speed curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpeedPoint {
    /// Microseconds into the material.
    pub source: Micros,
    /// Playback speed at that instant; 1 is real time.
    pub speed: f32,
}

/// The ready-made ramps on the Curve tab. Our own shapes; CapCut has presets
/// with similar intents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedPreset {
    /// A burst, a slow beat, a burst: cuts of an action montage.
    Montage,
    /// Fast in, deep slow motion on the moment, fast out.
    Hero,
    /// A held fast pace that drops into bullet time and recovers.
    Bullet,
    /// Slightly slow, a sharp jump forward in the middle, slow again.
    JumpCut,
    /// Starts fast and settles into real time.
    FlashIn,
    /// Plays in real time and leaves fast.
    FlashOut,
}

impl SpeedPreset {
    pub const ALL: [SpeedPreset; 6] = [
        SpeedPreset::Montage,
        SpeedPreset::Hero,
        SpeedPreset::Bullet,
        SpeedPreset::JumpCut,
        SpeedPreset::FlashIn,
        SpeedPreset::FlashOut,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SpeedPreset::Montage => "Montage",
            SpeedPreset::Hero => "Hero",
            SpeedPreset::Bullet => "Bullet",
            SpeedPreset::JumpCut => "Jump cut",
            SpeedPreset::FlashIn => "Flash in",
            SpeedPreset::FlashOut => "Flash out",
        }
    }

    /// The shape: `(position, speed)` with the position a fraction of the
    /// clip's source range.
    pub fn shape(self) -> &'static [(f32, f32)] {
        match self {
            SpeedPreset::Montage => &[(0.0, 1.0), (0.2, 4.0), (0.45, 1.0), (0.7, 0.3), (1.0, 1.0)],
            SpeedPreset::Hero => &[(0.0, 1.0), (0.2, 4.0), (0.5, 0.25), (0.8, 4.0), (1.0, 1.0)],
            SpeedPreset::Bullet => &[
                (0.0, 3.0),
                (0.28, 3.0),
                (0.42, 0.2),
                (0.58, 0.2),
                (0.72, 3.0),
                (1.0, 3.0),
            ],
            SpeedPreset::JumpCut => &[(0.0, 0.6), (0.35, 0.6), (0.5, 8.0), (0.65, 0.6), (1.0, 0.6)],
            SpeedPreset::FlashIn => &[(0.0, 5.0), (0.3, 5.0), (0.6, 1.0), (1.0, 1.0)],
            SpeedPreset::FlashOut => &[(0.0, 1.0), (0.4, 1.0), (0.7, 5.0), (1.0, 5.0)],
        }
    }

    /// The preset laid over `source`, as absolute points.
    pub fn points(self, source: TimeRange) -> Vec<SpeedPoint> {
        points_from_shape(self.shape(), source)
    }
}

/// A shape of `(fraction, speed)` pairs laid over `source`. Positions that
/// round onto the same microsecond (a source shorter than the point count)
/// keep only the first.
pub fn points_from_shape(shape: &[(f32, f32)], source: TimeRange) -> Vec<SpeedPoint> {
    let mut points: Vec<SpeedPoint> = Vec::with_capacity(shape.len());
    for &(at, speed) in shape {
        let source =
            source.start + (at.clamp(0.0, 1.0) as f64 * source.duration as f64).round() as Micros;
        if points.last().is_some_and(|p| p.source >= source) {
            continue;
        }
        points.push(SpeedPoint { source, speed });
    }
    points
}

/// The flat curve "Custom" starts from: five points at real time.
pub fn custom_points(source: TimeRange) -> Vec<SpeedPoint> {
    points_from_shape(
        &[(0.0, 1.0), (0.25, 1.0), (0.5, 1.0), (0.75, 1.0), (1.0, 1.0)],
        source,
    )
}

/// A speed curve as it sits in the pool. See the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeedCurveMaterial {
    pub id: Id,
    /// The preset this was made from, while it is unchanged. `None` is a
    /// custom curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<SpeedPreset>,
    /// Sorted by `source`, strictly increasing, at least one.
    pub points: Vec<SpeedPoint>,
}

impl SpeedCurveMaterial {
    /// Why this curve cannot be stored, if it cannot.
    pub fn problem(&self) -> Option<String> {
        if self.points.is_empty() {
            return Some("a speed curve needs at least one point".into());
        }
        for pair in self.points.windows(2) {
            if pair[1].source <= pair[0].source {
                return Some("speed curve points must be in order, one per instant".into());
            }
        }
        for point in &self.points {
            if !point.speed.is_finite() {
                return Some(
                    "a speed curve point must be a finite number — a project containing a NaN \
                     saves as null and never opens again"
                        .into(),
                );
            }
            if !(MIN_CURVE_SPEED..=MAX_CURVE_SPEED).contains(&point.speed) {
                return Some(format!(
                    "a speed curve stays between {MIN_CURVE_SPEED}x and {MAX_CURVE_SPEED}x"
                ));
            }
        }
        None
    }

    /// The slowest speed on the curve, which bounds how far one microsecond of
    /// rounding in the source moves the timeline.
    pub fn min_speed(&self) -> f32 {
        self.points
            .iter()
            .map(|p| p.speed)
            .fold(f32::INFINITY, f32::min)
            .max(MIN_CURVE_SPEED)
    }
}

// ---------------------------------------------------------------------------
// The integral
// ---------------------------------------------------------------------------

/// The curve, prepared for evaluation: positions, slownesses, and the
/// timeline time at each point counted from the first.
struct Prepared {
    x: Vec<f64>,
    w: Vec<f64>,
    /// `f[i]` is the timeline time from `x[0]` to `x[i]`.
    f: Vec<f64>,
}

impl Prepared {
    fn new(points: &[SpeedPoint]) -> Self {
        let x: Vec<f64> = points.iter().map(|p| p.source as f64).collect();
        let w: Vec<f64> = points
            .iter()
            .map(|p| 1.0 / (p.speed.clamp(MIN_CURVE_SPEED, MAX_CURVE_SPEED) as f64))
            .collect();
        let mut f = Vec::with_capacity(x.len());
        f.push(0.0);
        for i in 1..x.len() {
            // The smoothstep's integral over a whole piece is the trapezoid.
            let piece = (x[i] - x[i - 1]) * (w[i - 1] + w[i]) * 0.5;
            f.push(f[i - 1] + piece);
        }
        Self { x, w, f }
    }

    /// Slowness at source position `at`.
    fn slowness(&self, at: f64) -> f64 {
        let n = self.x.len();
        if at <= self.x[0] {
            return self.w[0];
        }
        if at >= self.x[n - 1] {
            return self.w[n - 1];
        }
        let i = self.x.partition_point(|&x| x <= at) - 1;
        let u = (at - self.x[i]) / (self.x[i + 1] - self.x[i]);
        self.w[i] + (self.w[i + 1] - self.w[i]) * u * u * (3.0 - 2.0 * u)
    }

    /// Timeline time from `x[0]` to source position `at`; negative before it.
    fn integral(&self, at: f64) -> f64 {
        let n = self.x.len();
        if at <= self.x[0] {
            return (at - self.x[0]) * self.w[0];
        }
        if at >= self.x[n - 1] {
            return self.f[n - 1] + (at - self.x[n - 1]) * self.w[n - 1];
        }
        let i = self.x.partition_point(|&x| x <= at) - 1;
        self.f[i]
            + piece_integral(
                self.x[i + 1] - self.x[i],
                self.w[i],
                self.w[i + 1],
                at - self.x[i],
            )
    }

    /// The source position where the integral reaches `target`.
    fn inverse(&self, target: f64) -> f64 {
        let n = self.x.len();
        if target <= 0.0 {
            return self.x[0] + target / self.w[0];
        }
        if target >= self.f[n - 1] {
            return self.x[n - 1] + (target - self.f[n - 1]) / self.w[n - 1];
        }
        let i = self.f.partition_point(|&f| f <= target) - 1;
        let (h, w0, w1) = (self.x[i + 1] - self.x[i], self.w[i], self.w[i + 1]);
        let want = target - self.f[i];
        // Newton on a strictly increasing function, kept inside a bracket
        // that bisection shrinks whenever a step would leave it.
        let (mut lo, mut hi) = (0.0_f64, h);
        let mut d = want / (self.f[i + 1] - self.f[i]) * h;
        for _ in 0..60 {
            let g = piece_integral(h, w0, w1, d) - want;
            if g.abs() < 1e-9 {
                break;
            }
            if g > 0.0 {
                hi = d;
            } else {
                lo = d;
            }
            let u = d / h;
            let slope = w0 + (w1 - w0) * u * u * (3.0 - 2.0 * u);
            let next = d - g / slope;
            d = if next > lo && next < hi {
                next
            } else {
                0.5 * (lo + hi)
            };
            if hi - lo < 1e-9 {
                break;
            }
        }
        self.x[i] + d
    }
}

/// `∫₀^d w(s) ds` over one piece of length `h` from slowness `w0` to `w1`,
/// with `w` following a smoothstep: `h·(w0·u + Δ·(u³ − u⁴/2))`, `u = d/h`.
fn piece_integral(h: f64, w0: f64, w1: f64, d: f64) -> f64 {
    let u = d / h;
    h * (w0 * u + (w1 - w0) * (u * u * u - 0.5 * u * u * u * u))
}

/// Speed at source position `at`.
pub fn speed_at(points: &[SpeedPoint], at: f64) -> f64 {
    if points.is_empty() {
        return 1.0;
    }
    1.0 / Prepared::new(points).slowness(at)
}

/// Timeline microseconds it takes to play the source from `from` to `to`
/// (negative when `to` is before `from`).
pub fn elapsed(points: &[SpeedPoint], from: f64, to: f64) -> f64 {
    if points.is_empty() {
        return to - from;
    }
    let p = Prepared::new(points);
    p.integral(to) - p.integral(from)
}

/// The source position reached by playing `dt` timeline microseconds from
/// source position `from` (backwards for a negative `dt`).
pub fn advance(points: &[SpeedPoint], from: f64, dt: f64) -> f64 {
    if points.is_empty() {
        return from + dt;
    }
    let p = Prepared::new(points);
    p.inverse(p.integral(from) + dt)
}

/// The timeline length of `source` played through `points`, rounded to the
/// microsecond. The one function that derives a curved clip's length.
pub fn curve_target_duration(points: &[SpeedPoint], source: TimeRange) -> Micros {
    elapsed(points, source.start as f64, source.end() as f64).round() as Micros
}

/// How far a curved clip's timeline length may sit from the one its curve
/// implies. A source instant is a whole microsecond, and one microsecond of
/// source at the slowest speed is `1 / min_speed` of timeline; two of those
/// (one per end) plus two for the rounding of the length itself.
pub fn curve_slack(curve: &SpeedCurveMaterial) -> Micros {
    2 + (1.0 / curve.min_speed() as f64).ceil() as Micros
}

/// Whether `target` is the timeline length `source` takes through `curve`.
pub fn check_curve_ranges(
    curve: &SpeedCurveMaterial,
    target: TimeRange,
    source: TimeRange,
) -> Result<(), String> {
    let implied = curve_target_duration(&curve.points, source);
    if (target.duration - implied).abs() <= curve_slack(curve) {
        Ok(())
    } else {
        Err(format!(
            "this edit would leave the clip {} µs long on the timeline, but its speed curve \
             plays that part of the material in {} µs",
            target.duration, implied
        ))
    }
}

// ---------------------------------------------------------------------------
// The pool and the time map
// ---------------------------------------------------------------------------

impl MaterialPool {
    pub fn speed_curve(&self, id: &str) -> Option<&SpeedCurveMaterial> {
        self.speed_curves.iter().find(|m| m.id == id)
    }

    /// The speed curve of `segment`, if it has one.
    pub fn speed_curve_of(&self, segment: &Segment) -> Option<&SpeedCurveMaterial> {
        if self.speed_curves.is_empty() {
            return None;
        }
        segment.extras.iter().find_map(|id| self.speed_curve(id))
    }

    /// How `segment` maps timeline time to source time.
    pub fn time_map<'a>(&'a self, segment: &'a Segment) -> TimeMap<'a> {
        TimeMap {
            segment,
            curve: self.speed_curve_of(segment),
        }
    }
}

/// How one segment maps timeline time to source time: through its constant
/// speed, or through its speed curve. Every place that turns a timeline
/// instant into a frame of the file, or back, goes through this.
#[derive(Clone, Copy)]
pub struct TimeMap<'a> {
    pub segment: &'a Segment,
    pub curve: Option<&'a SpeedCurveMaterial>,
}

impl TimeMap<'_> {
    pub fn is_curved(&self) -> bool {
        self.curve.is_some()
    }

    fn constant_speed(&self) -> f64 {
        let speed = self.segment.speed;
        if speed.is_finite() && speed > 0.0 {
            speed as f64
        } else {
            1.0
        }
    }

    /// The source instant at `offset` timeline microseconds after the clip's
    /// start. Defined for any offset — before the start and past the end the
    /// clip's edge speeds carry on, which is what a transition borrowing
    /// frames from beyond a cut needs.
    ///
    /// At constant speed this is exactly `Segment::source_time_at`'s
    /// arithmetic (truncating), so nothing changes for a clip without a curve.
    pub fn source_at(&self, offset: Micros) -> Micros {
        let start = self.segment.source_range.start;
        match self.curve {
            None => start + (offset as f64 * self.segment.speed as f64) as Micros,
            Some(curve) => {
                if offset == 0 {
                    return start;
                }
                advance(&curve.points, start as f64, offset as f64).round() as Micros
            }
        }
    }

    /// `Segment::source_time_at`, through the curve: `None` outside the clip.
    pub fn source_time_at(&self, time: Micros) -> Option<Micros> {
        if !self.segment.target_range.contains(time) {
            return None;
        }
        Some(self.source_at(time - self.segment.target_range.start))
    }

    /// The source instant at timeline `time`, held at the clip's edges.
    pub fn clamped_source_time(&self, time: Micros) -> Micros {
        let target = self.segment.target_range;
        let end = (target.end() - 1).max(target.start);
        self.source_at(time.clamp(target.start, end) - target.start)
    }

    /// The timeline offset from the clip's start at which source instant
    /// `source` plays. Rounded; defined outside the clip as well.
    pub fn offset_of(&self, source: Micros) -> Micros {
        let start = self.segment.source_range.start;
        match self.curve {
            None => ((source - start) as f64 / self.constant_speed()).round() as Micros,
            Some(curve) => elapsed(&curve.points, start as f64, source as f64).round() as Micros,
        }
    }

    /// The speed at `offset` timeline microseconds after the clip's start.
    pub fn speed_at(&self, offset: Micros) -> f64 {
        match self.curve {
            None => self.constant_speed(),
            Some(curve) => speed_at(&curve.points, self.source_at(offset) as f64),
        }
    }

    /// The source range the clip reads after its timeline range becomes
    /// `target`, its head and tail moved independently: the part of the file
    /// the old range's speeds carry the new edges to.
    ///
    /// At constant speed this is the rule every trim already used — the head
    /// moves the source start by `source_duration_for(head)`, and the length
    /// is `source_duration_for(target.duration)` — so a curve-less trim is
    /// byte-identical to what it was.
    pub fn retimed_source(&self, target: TimeRange) -> TimeRange {
        let head = target.start - self.segment.target_range.start;
        let source = self.segment.source_range;
        match self.curve {
            None => TimeRange::new(
                (source.start + super::document::source_duration_for(head, self.segment.speed))
                    .max(0),
                super::document::source_duration_for(target.duration, self.segment.speed),
            ),
            Some(_) => {
                let start = self.source_at(head).max(0);
                let end = self.source_at(head + target.duration).max(start + 1);
                TimeRange::new(start, end - start)
            }
        }
    }

    /// The timeline offset from the clip's start at which source instant `at`
    /// would be reached, for limits ("how far may this head move left before
    /// it reads before the file"). Not rounded inward or outward; callers
    /// clamp.
    pub fn offset_of_f(&self, at: Micros) -> f64 {
        let start = self.segment.source_range.start;
        match self.curve {
            None => (at - start) as f64 / self.constant_speed(),
            Some(curve) => elapsed(&curve.points, start as f64, at as f64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{TimeRange, Transform};

    fn flat(speed: f32) -> Vec<SpeedPoint> {
        vec![SpeedPoint { source: 0, speed }]
    }

    #[test]
    fn a_flat_curve_is_a_constant_speed() {
        for speed in [0.25_f32, 1.0, 2.0, 7.5] {
            let points = flat(speed);
            let d = curve_target_duration(&points, TimeRange::new(1_000_000, 4_000_000));
            assert_eq!(d, (4_000_000.0 / speed as f64).round() as Micros);
            let exact = elapsed(&points, 1_000_000.0, 5_000_000.0);
            let back = advance(&points, 1_000_000.0, exact);
            assert!((back - 5_000_000.0).abs() < 1e-3, "{back}");
        }
    }

    /// The closed form against a brute-force midpoint sum of `1/s(x)`.
    #[test]
    fn the_duration_is_the_integral_of_the_slowness() {
        let source = TimeRange::new(0, 10_000_000);
        for preset in SpeedPreset::ALL {
            let points = preset.points(source);
            let closed = elapsed(&points, 0.0, 10_000_000.0);
            let steps = 200_000;
            let dx = 10_000_000.0 / steps as f64;
            let brute: f64 = (0..steps)
                .map(|i| dx / speed_at(&points, (i as f64 + 0.5) * dx))
                .sum();
            assert!(
                (closed - brute).abs() < 1.0,
                "{preset:?}: closed {closed} vs brute {brute}"
            );
        }
    }

    #[test]
    fn two_points_of_known_slowness_integrate_to_the_trapezoid() {
        // 1x to 0.5x over 2 s: slowness 1 → 2, mean 1.5, so 3 s.
        let points = vec![
            SpeedPoint {
                source: 0,
                speed: 1.0,
            },
            SpeedPoint {
                source: 2_000_000,
                speed: 0.5,
            },
        ];
        assert_eq!(
            curve_target_duration(&points, TimeRange::new(0, 2_000_000)),
            3_000_000
        );
        // Past the last point the speed holds at 0.5x.
        assert_eq!(
            curve_target_duration(&points, TimeRange::new(0, 3_000_000)),
            5_000_000
        );
        // Half way through the piece the smoothstep is half way: 1.5.
        assert!((speed_at(&points, 1_000_000.0) - 1.0 / 1.5).abs() < 1e-12);
    }

    #[test]
    fn advance_inverts_elapsed_everywhere() {
        let source = TimeRange::new(250_000, 6_000_000);
        for preset in SpeedPreset::ALL {
            let points = preset.points(source);
            for i in -20..=220 {
                let x = 250_000.0 + i as f64 * 30_000.0;
                let t = elapsed(&points, 250_000.0, x);
                let back = advance(&points, 250_000.0, t);
                assert!((back - x).abs() < 1e-3, "{preset:?} at {x}: {back}");
            }
        }
    }

    fn segment(points: &[SpeedPoint], source: TimeRange) -> (MaterialPool, Segment) {
        let curve = SpeedCurveMaterial {
            id: "curve".into(),
            preset: None,
            points: points.to_vec(),
        };
        let target = TimeRange::new(3_000_000, curve_target_duration(points, source));
        let segment = Segment {
            id: "s".into(),
            material_id: "m".into(),
            target_range: target,
            source_range: source,
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: vec!["curve".into()],
            keyframes: Vec::new(),
        };
        let pool = MaterialPool {
            speed_curves: vec![curve],
            ..MaterialPool::default()
        };
        (pool, segment)
    }

    #[test]
    fn the_first_and_last_frames_map_to_the_ends_of_the_source() {
        let source = TimeRange::new(1_000_000, 5_000_000);
        for preset in SpeedPreset::ALL {
            let (pool, seg) = segment(&preset.points(source), source);
            let map = pool.time_map(&seg);
            assert_eq!(
                map.source_time_at(seg.target_range.start),
                Some(source.start)
            );
            let last = map.source_time_at(seg.target_range.end() - 1).unwrap();
            assert!(
                (source.end() - last).abs() <= 11,
                "{preset:?}: last frame reads {last}, source ends at {}",
                source.end()
            );
            assert_eq!(map.source_time_at(seg.target_range.end()), None);
            // Every offset maps back onto itself within the rounding of a µs
            // of source at the slowest speed.
            for offset in (0..seg.target_range.duration).step_by(33_333) {
                let s = map.source_at(offset);
                assert!(
                    (map.offset_of(s) - offset).abs() <= 11,
                    "{preset:?} {offset}"
                );
            }
        }
    }

    #[test]
    fn a_flat_curve_maps_like_a_constant_speed_frame_for_frame() {
        let source = TimeRange::new(0, 3_000_000);
        let (pool, mut seg) = segment(&flat(2.0), source);
        let curved: Vec<Micros> = (0..45)
            .map(|f| pool.time_map(&seg).source_at(f * 33_333))
            .collect();
        seg.extras.clear();
        seg.speed = 2.0;
        let constant: Vec<Micros> = (0..45)
            .map(|f| pool.time_map(&seg).source_at(f * 33_333))
            .collect();
        for (a, b) in curved.iter().zip(&constant) {
            assert!((a - b).abs() <= 1, "{a} vs {b}");
        }
    }

    #[test]
    fn presets_stay_in_range_and_in_order() {
        for preset in SpeedPreset::ALL {
            let material = SpeedCurveMaterial {
                id: "x".into(),
                preset: Some(preset),
                points: preset.points(TimeRange::new(0, 1_000_000)),
            };
            assert_eq!(material.problem(), None, "{preset:?}");
        }
        let bad = SpeedCurveMaterial {
            id: "x".into(),
            preset: None,
            points: vec![
                SpeedPoint {
                    source: 5,
                    speed: 1.0,
                },
                SpeedPoint {
                    source: 5,
                    speed: 2.0,
                },
            ],
        };
        assert!(bad.problem().is_some());
    }
}
