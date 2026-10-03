//! What a track *is* in the document: the samples one analysis produced, and
//! the link that makes an overlay follow them.
//!
//! Both are pool materials (`MaterialPool::trackings`, `MaterialPool::follows`)
//! for the reason `docs/research/ml-features.md` §2.5 gives: a track is edit
//! data, made once and read on every frame, and one track can drive several
//! overlays. The overlay names its follow link in `Segment::extras`, exactly
//! like a colour adjustment, so no segment constructor anywhere had to change.

use serde::{Deserialize, Serialize};

use crate::modules::project::document::{Id, Micros};

/// The tracker this build runs, as it is stamped into every track it makes.
///
/// A re-track with a different tracker version gives slightly different
/// numbers; the stamp is how a later build knows a track was not made by it.
pub const TRACKER: &str = "klt";
pub const TRACKER_VERSION: u32 = 1;

/// The long side, in pixels, frames are decoded at for analysis. Decode is the
/// bottleneck, not the tracker, and 640 px keeps a ball of a few dozen pixels
/// trackable.
pub const DEFAULT_ANALYSIS_SIZE: u32 = 640;

/// A sample the user placed by hand. Smoothing never crosses it and a re-track
/// starts from it.
pub const FLAG_ANCHOR: u8 = 1;
/// The tracker lost the object here. The sample's box is the last good one
/// held (or a prediction); evaluation skips it and interpolates across.
pub const FLAG_LOST: u8 = 2;

/// Confidence under which a sample is drawn as doubtful on the player.
pub const LOW_CONFIDENCE: f32 = 0.5;

/// The object's pose in one source frame.
///
/// Coordinates are fractions of the **displayed** source frame (rotation
/// applied, as the decoder hands it out and the compositor draws it): `0,0` is
/// the top-left corner, `1,1` the bottom-right. Fractions survive a proxy, a
/// different analysis size and a re-import at another resolution.
///
/// Short field names because a minute of video is 1 800 of these and the
/// project file is pretty-printed JSON.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackSample {
    /// Source time of the frame, µs.
    pub t: Micros,
    /// Box centre.
    pub x: f32,
    pub y: f32,
    /// Box size.
    pub w: f32,
    pub h: f32,
    /// Rotation in degrees, clockwise as the viewer sees it, relative to the
    /// frame the track started on. Not wrapped: a spinning object keeps
    /// counting, so interpolation never takes the long way round.
    #[serde(default)]
    pub a: f32,
    /// How sure the tracker was, `0..1`.
    #[serde(default = "one")]
    pub c: f32,
    /// [`FLAG_ANCHOR`], [`FLAG_LOST`].
    #[serde(default)]
    pub f: u8,
}

impl TrackSample {
    pub fn is_lost(&self) -> bool {
        self.f & FLAG_LOST != 0
    }

    pub fn is_anchor(&self) -> bool {
        self.f & FLAG_ANCHOR != 0
    }

    fn is_finite(&self) -> bool {
        [self.x, self.y, self.w, self.h, self.a, self.c]
            .iter()
            .all(|v| v.is_finite())
    }
}

/// How a track was made and how it is read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackSettings {
    /// [`TRACKER`] of the build that made the samples.
    #[serde(default = "default_tracker")]
    pub tracker: String,
    #[serde(default = "default_version")]
    pub version: u32,
    /// Long side of the analysis frames, in pixels.
    #[serde(default = "default_analysis_size")]
    pub analysis_size: u32,
    /// `0` is the raw track, `1` the strongest smoothing. Applied when the
    /// track is read, never baked into the samples, so it can be changed
    /// after the fact without re-tracking.
    #[serde(default)]
    pub smoothing: f32,
}

impl Default for TrackSettings {
    fn default() -> Self {
        Self {
            tracker: default_tracker(),
            version: default_version(),
            analysis_size: default_analysis_size(),
            smoothing: 0.0,
        }
    }
}

/// One analysis of one object in one video file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackingMaterial {
    pub id: Id,
    /// The video material whose pixels were tracked. Samples are in its source
    /// time, so every clip cut from that file can carry the track.
    pub media_id: Id,
    #[serde(default)]
    pub settings: TrackSettings,
    /// Sorted by `t`, one per analysed frame, no duplicates.
    #[serde(default)]
    pub samples: Vec<TrackSample>,
}

/// What of the object's motion a follower takes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowMode {
    #[default]
    Position,
    PositionScale,
    PositionScaleRotation,
}

impl FollowMode {
    pub const ALL: [FollowMode; 3] = [
        FollowMode::Position,
        FollowMode::PositionScale,
        FollowMode::PositionScaleRotation,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FollowMode::Position => "Position",
            FollowMode::PositionScale => "Position and scale",
            FollowMode::PositionScaleRotation => "Position, scale and rotation",
        }
    }

    pub fn scales(self) -> bool {
        !matches!(self, FollowMode::Position)
    }

    pub fn rotates(self) -> bool {
        matches!(self, FollowMode::PositionScaleRotation)
    }
}

/// "This overlay follows that track, as it is seen through that clip."
///
/// ## How a follower is placed
///
/// At timeline time `t` the target clip maps `t` to a source time, the track
/// gives the object's pose there, and the target clip's crop and transform put
/// that point on the canvas: `P(t)`. The overlay's own (animated) position plus
/// `offset` is a vector from the object, and lands at `P(t) + that vector` —
/// turned and stretched with the object in the modes that say so.
///
/// Attaching sets `offset` to `-P` at the playhead, so attaching moves
/// nothing: the overlay's transform keeps meaning "where it sits at the
/// reference frame", and the follow adds the object's motion since then.
/// Because `P` goes through the clip, trimming, slipping, retiming, moving or
/// scaling the video keeps the overlay on the object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowMaterial {
    pub id: Id,
    pub track_id: Id,
    /// The video clip whose placement maps the track onto the canvas. When it
    /// no longer covers the instant (it was split, say) any clip of the same
    /// file that does is used instead.
    pub target_segment_id: Id,
    #[serde(default)]
    pub mode: FollowMode,
    /// Canvas-normalised, added to the overlay's position.
    #[serde(default)]
    pub offset: [f32; 2],
    /// Source time of the pose that scale and rotation are measured against.
    #[serde(default)]
    pub reference: Micros,
}

/// A pose read off a track: interpolated, smoothed, never a lost frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub angle: f32,
    pub confidence: f32,
}

impl Pose {
    /// One number for the object's size, stable against aspect jitter.
    pub fn size(&self) -> f32 {
        (self.w.max(1e-6) * self.h.max(1e-6)).sqrt()
    }

    fn from_sample(s: &TrackSample) -> Self {
        Self {
            x: s.x,
            y: s.y,
            w: s.w,
            h: s.h,
            angle: s.a,
            confidence: s.c,
        }
    }

    fn lerp(a: &TrackSample, b: &TrackSample, f: f32) -> Self {
        let mix = |p: f32, q: f32| p + (q - p) * f;
        Self {
            x: mix(a.x, b.x),
            y: mix(a.y, b.y),
            w: mix(a.w, b.w),
            h: mix(a.h, b.h),
            angle: mix(a.a, b.a),
            confidence: a.c.min(b.c),
        }
    }
}

/// The widest Gaussian smoothing reaches, in frames (σ at `smoothing = 1`).
const MAX_SMOOTHING_FRAMES: f32 = 8.0;

impl TrackingMaterial {
    pub fn new(media_id: Id, settings: TrackSettings, samples: Vec<TrackSample>) -> Self {
        Self {
            id: crate::modules::project::document::new_id(),
            media_id,
            settings,
            samples,
        }
    }

    /// First and last source time covered.
    pub fn range(&self) -> Option<(Micros, Micros)> {
        Some((self.samples.first()?.t, self.samples.last()?.t))
    }

    /// The first value that is not a finite number, by name. Refused at the
    /// edit boundary for the reason `Segment::non_finite_field` gives.
    pub fn non_finite_field(&self) -> Option<&'static str> {
        if !self.settings.smoothing.is_finite() {
            return Some("smoothing");
        }
        if self.samples.iter().any(|s| !s.is_finite()) {
            return Some("track sample");
        }
        None
    }

    /// Whether the samples are strictly increasing in time.
    pub fn is_sorted(&self) -> bool {
        self.samples.windows(2).all(|w| w[0].t < w[1].t)
    }

    /// The sample nearest `t`, lost or not — what the player draws as "the
    /// box at the playhead".
    pub fn nearest(&self, t: Micros) -> Option<&TrackSample> {
        let i = self.samples.partition_point(|s| s.t < t);
        let after = self.samples.get(i);
        let before = i.checked_sub(1).and_then(|i| self.samples.get(i));
        match (before, after) {
            (Some(b), Some(a)) => Some(if t - b.t <= a.t - t { b } else { a }),
            (b, a) => b.or(a),
        }
    }

    /// The object's pose at source time `t`.
    ///
    /// Lost frames are skipped, so the pose interpolates across them; outside
    /// the tracked range it holds the first or last good one. With smoothing
    /// on, each channel is a Gaussian-weighted *local linear* fit around `t`
    /// rather than a plain weighted mean: a moving object stays centred under
    /// smoothing instead of lagging behind at the ends of the track. The
    /// window never reaches past a user anchor, so an anchored frame is
    /// exactly where the user put it.
    pub fn pose_at(&self, t: Micros) -> Option<Pose> {
        let good: Vec<&TrackSample> = self.samples.iter().filter(|s| !s.is_lost()).collect();
        let first = *good.first()?;
        let last = *good.last()?;

        let i = good.partition_point(|s| s.t < t);
        if let Some(s) = good.get(i).filter(|s| s.t == t && s.is_anchor()) {
            return Some(Pose::from_sample(s));
        }

        let smoothing = self.settings.smoothing.clamp(0.0, 1.0);
        if smoothing <= 0.0 || good.len() < 3 {
            if t <= first.t {
                return Some(Pose::from_sample(first));
            }
            if t >= last.t {
                return Some(Pose::from_sample(last));
            }
            let (a, b) = (good[i - 1], good[i]);
            let f = (t - a.t) as f32 / (b.t - a.t).max(1) as f32;
            return Some(Pose::lerp(a, b, f));
        }

        let t = t.clamp(first.t, last.t);
        let i = good.partition_point(|s| s.t < t);
        let period = (last.t - first.t) as f64 / (good.len() - 1) as f64;
        let sigma = (smoothing * MAX_SMOOTHING_FRAMES) as f64 * period.max(1.0);
        let reach = (3.0 * sigma) as Micros;

        // Bounded by the nearest anchors on either side, inclusive.
        let mut lo = i;
        while lo > 0 && good[lo - 1].t >= t - reach {
            lo -= 1;
            if good[lo].is_anchor() {
                break;
            }
        }
        let mut hi = i;
        while hi < good.len() && good[hi].t <= t + reach {
            let anchor = good[hi].is_anchor();
            hi += 1;
            if anchor && good[hi - 1].t > t {
                break;
            }
        }
        let window = &good[lo..hi.max(lo)];
        if window.is_empty() {
            return self.pose_at_raw(t, &good);
        }

        // Weighted least squares of value = alpha + beta·(s.t - t); alpha is
        // the smoothed value at t.
        let mut s0 = 0.0f64;
        let mut s1 = 0.0f64;
        let mut s2 = 0.0f64;
        let mut sums = [[0.0f64; 2]; 5];
        let mut confidence = 1.0f32;
        for s in window {
            let d = (s.t - t) as f64 / sigma;
            let w = (-0.5 * d * d).exp();
            s0 += w;
            s1 += w * d;
            s2 += w * d * d;
            for (slot, v) in sums.iter_mut().zip([s.x, s.y, s.w, s.h, s.a]) {
                slot[0] += w * v as f64;
                slot[1] += w * v as f64 * d;
            }
            if d.abs() < 1.0 {
                confidence = confidence.min(s.c);
            }
        }
        let det = s0 * s2 - s1 * s1;
        let fit = |[t0, t1]: [f64; 2]| -> f32 {
            if det.abs() < 1e-9 || window.len() < 3 {
                (t0 / s0) as f32
            } else {
                ((s2 * t0 - s1 * t1) / det) as f32
            }
        };
        Some(Pose {
            x: fit(sums[0]),
            y: fit(sums[1]),
            w: fit(sums[2]).max(1e-6),
            h: fit(sums[3]).max(1e-6),
            angle: fit(sums[4]),
            confidence,
        })
    }

    fn pose_at_raw(&self, t: Micros, good: &[&TrackSample]) -> Option<Pose> {
        let i = good.partition_point(|s| s.t < t);
        match (i.checked_sub(1).map(|i| good[i]), good.get(i).copied()) {
            (Some(a), Some(b)) => {
                let f = (t - a.t) as f32 / (b.t - a.t).max(1) as f32;
                Some(Pose::lerp(a, b, f))
            }
            (Some(s), None) | (None, Some(s)) => Some(Pose::from_sample(s)),
            (None, None) => None,
        }
    }

    /// Replace the samples from `from` onward (or up to it, `forward ==
    /// false`) with `new`, keeping the rest. What a re-track writes.
    pub fn splice(&mut self, from: Micros, forward: bool, new: Vec<TrackSample>) {
        let mut kept: Vec<TrackSample> = self
            .samples
            .iter()
            .copied()
            .filter(|s| if forward { s.t < from } else { s.t > from })
            .collect();
        kept.extend(new);
        kept.sort_by_key(|s| s.t);
        kept.dedup_by_key(|s| s.t);
        self.samples = kept;
    }
}

fn one() -> f32 {
    1.0
}

fn default_tracker() -> String {
    TRACKER.into()
}

fn default_version() -> u32 {
    TRACKER_VERSION
}

fn default_analysis_size() -> u32 {
    DEFAULT_ANALYSIS_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: Micros, x: f32) -> TrackSample {
        TrackSample {
            t,
            x,
            y: 0.5,
            w: 0.1,
            h: 0.1,
            a: 0.0,
            c: 1.0,
            f: 0,
        }
    }

    fn track(samples: Vec<TrackSample>) -> TrackingMaterial {
        TrackingMaterial::new("m".into(), TrackSettings::default(), samples)
    }

    #[test]
    fn a_pose_between_samples_is_interpolated_and_holds_outside() {
        let t = track(vec![sample(0, 0.1), sample(100, 0.3)]);
        assert!((t.pose_at(50).unwrap().x - 0.2).abs() < 1e-6);
        assert_eq!(t.pose_at(-10).unwrap().x, 0.1);
        assert_eq!(t.pose_at(500).unwrap().x, 0.3);
    }

    #[test]
    fn lost_frames_are_bridged_not_shown() {
        let mut lost = sample(50, 0.9);
        lost.f = FLAG_LOST;
        let t = track(vec![sample(0, 0.1), lost, sample(100, 0.3)]);
        assert!((t.pose_at(50).unwrap().x - 0.2).abs() < 1e-6);
    }

    #[test]
    fn smoothing_removes_jitter_but_not_constant_motion() {
        // A straight line with alternating noise: the smoothed track is
        // within a hair of the line, the raw one is not.
        let samples: Vec<TrackSample> = (0..60)
            .map(|i| {
                let noise = if i % 2 == 0 { 0.01 } else { -0.01 };
                sample(i * 33_333, 0.1 + i as f32 * 0.01 + noise)
            })
            .collect();
        let mut t = track(samples);
        let truth = |i: i64| 0.1 + i as f32 * 0.01;
        assert!((t.pose_at(30 * 33_333).unwrap().x - truth(30)).abs() > 0.009);
        t.settings.smoothing = 0.5;
        for i in [1, 10, 30, 58] {
            let x = t.pose_at(i * 33_333).unwrap().x;
            assert!((x - truth(i)).abs() < 0.003, "frame {i}: {x}");
        }
    }

    #[test]
    fn smoothing_never_moves_an_anchor() {
        let mut samples: Vec<TrackSample> = (0..30).map(|i| sample(i * 1000, 0.5)).collect();
        samples[15].x = 0.8;
        samples[15].f = FLAG_ANCHOR;
        let mut t = track(samples);
        t.settings.smoothing = 1.0;
        assert_eq!(t.pose_at(15_000).unwrap().x, 0.8);
    }

    #[test]
    fn splice_keeps_what_comes_before_a_retrack() {
        let mut t = track((0..10).map(|i| sample(i * 10, 0.0)).collect());
        t.splice(50, true, (5..12).map(|i| sample(i * 10, 1.0)).collect());
        assert_eq!(t.samples.len(), 12);
        assert!(t.samples[..5].iter().all(|s| s.x == 0.0));
        assert!(t.samples[5..].iter().all(|s| s.x == 1.0));
        assert!(t.is_sorted());
    }

    #[test]
    fn an_old_sample_without_optional_fields_reads() {
        let s: TrackSample =
            serde_json::from_str(r#"{"t":5,"x":0.1,"y":0.2,"w":0.3,"h":0.4}"#).expect("parse");
        assert_eq!((s.a, s.c, s.f), (0.0, 1.0, 0));
        let m: TrackingMaterial = serde_json::from_str(r#"{"id":"a","media_id":"b"}"#).unwrap();
        assert_eq!(m.settings, TrackSettings::default());
    }
}
