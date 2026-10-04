//! A compound clip's inside, seen from the outer timeline: where an inner
//! instant plays, how fast, and through which speed curve.
//!
//! The compound clip maps outer timeline time to inner (nested sequence) time
//! through its own [`TimeMap`] — a constant speed or a speed curve whose
//! "source" is the nested sequence's time. A clip inside maps inner time to
//! its file through its own. Both the audio flattening (`super::audio`) and
//! putting a compound clip's clips back (`super::build::flatten`) need the
//! composition of the two, and get it here.
//!
//! What composes exactly:
//!
//! - constant over constant: the speeds multiply;
//! - a curve on the inner clip under a constant compound speed: every point's
//!   speed is multiplied;
//! - a constant inner clip under a curved compound clip: the compound's curve
//!   points, moved into the clip's source time (`x = x0 + s·(t − a)`) with
//!   their speeds multiplied by `s`. The curve interpolates slowness along a
//!   smoothstep between points, and an affine change of variable maps a
//!   smoothstep piece onto a smoothstep piece, so this is the same curve, not
//!   an approximation.
//!
//! A curve under a curve has no exact form in the curve model; [`dense`]
//! samples the product, which is what the sound uses. Flattening refuses that
//! case rather than write an approximation into the document.

use crate::modules::project::speed::{self, SpeedPoint};
use crate::modules::project::{
    AnimatableProperty, Easing, Keyframe, KeyframeTrack, MaterialPool, Micros, Segment,
    SpeedCurveMaterial, TimeMap, TimeRange,
};

/// How a compound clip maps its inside onto the outer timeline.
#[derive(Clone, Copy)]
pub struct Outer<'a> {
    pub segment: &'a Segment,
    pub map: TimeMap<'a>,
}

impl<'a> Outer<'a> {
    pub fn new(pool: &'a MaterialPool, segment: &'a Segment) -> Self {
        Self {
            segment,
            map: pool.time_map(segment),
        }
    }

    pub fn curve(&self) -> Option<&'a SpeedCurveMaterial> {
        self.map.curve
    }

    /// The constant speed, when there is no curve.
    pub fn speed(&self) -> f64 {
        sane(self.segment.speed)
    }

    /// The outer timeline instant at which inner instant `inner` plays.
    pub fn at(&self, inner: Micros) -> Micros {
        self.segment.target_range.start + self.map.offset_of(inner)
    }

    /// The part of the nested sequence the compound clip shows.
    pub fn window(&self) -> TimeRange {
        self.segment.source_range
    }
}

pub fn sane(speed: f32) -> f64 {
    if speed.is_finite() && speed > 0.0 {
        speed as f64
    } else {
        1.0
    }
}

/// The curve an inner clip at constant speed plays through once the compound
/// clip's curve applies on top, in the inner clip's source time. Exact; see
/// the module docs.
pub fn under_curve(outer: &SpeedCurveMaterial, inner: &Segment) -> Vec<SpeedPoint> {
    let s = sane(inner.speed);
    let a = inner.target_range.start;
    let x0 = inner.source_range.start;
    let mut points: Vec<SpeedPoint> = Vec::with_capacity(outer.points.len());
    for p in &outer.points {
        let source = x0 + (s * (p.source - a) as f64).round() as Micros;
        if points.last().is_some_and(|q| q.source >= source) {
            continue;
        }
        points.push(SpeedPoint {
            source,
            speed: (p.speed as f64 * s) as f32,
        });
    }
    points
}

/// An inner clip's own curve under a constant compound speed `k`. Exact.
pub fn scaled(curve: &SpeedCurveMaterial, k: f64) -> Vec<SpeedPoint> {
    curve
        .points
        .iter()
        .map(|p| SpeedPoint {
            source: p.source,
            speed: (p.speed as f64 * k) as f32,
        })
        .collect()
}

/// Samples per curve when a curved clip sits inside a curved compound clip.
const DENSE_POINTS: usize = 48;

/// The speed of an inner clip with a curve, under a compound clip with a
/// curve, sampled at [`DENSE_POINTS`] instants across `source` (the part of
/// the clip's file that plays). Exact at the samples, close between them.
pub fn dense(
    outer: &SpeedCurveMaterial,
    inner_map: TimeMap<'_>,
    source: TimeRange,
) -> Vec<SpeedPoint> {
    let a = inner_map.segment.target_range.start as f64;
    let inner_curve = inner_map.curve;
    let mut points: Vec<SpeedPoint> = Vec::with_capacity(DENSE_POINTS + 1);
    for n in 0..=DENSE_POINTS {
        let x = source.start + (source.duration as f64 * n as f64 / DENSE_POINTS as f64) as Micros;
        if points.last().is_some_and(|q| q.source >= x) {
            continue;
        }
        let inner_speed = match inner_curve {
            Some(c) => speed::speed_at(&c.points, x as f64),
            None => sane(inner_map.segment.speed),
        };
        let t = a + inner_map.offset_of_f(x);
        let outer_speed = speed::speed_at(&outer.points, t);
        points.push(SpeedPoint {
            source: x,
            speed: (inner_speed * outer_speed) as f32,
        });
    }
    points
}

/// Keyframe tracks of inner clip `inner`, moved onto the outer timeline for
/// its piece that now starts at outer instant `start`: each key keeps the
/// inner instant it was set at, wherever the compound clip's speed puts that.
pub fn keyframes(outer: &Outer<'_>, inner: &Segment, start: Micros) -> Vec<KeyframeTrack> {
    inner
        .keyframes
        .iter()
        .cloned()
        .map(|mut track| {
            for key in &mut track.keyframes {
                key.time = outer.at(inner.target_range.start + key.time) - start;
            }
            track
        })
        .collect()
}

/// How often the product of two volume envelopes is sampled.
const VOLUME_STEP: Micros = 20_000;

/// The volume envelope of a piece of an inner clip heard through a compound
/// clip with volume keyframes of its own: the product of the two, as one
/// track relative to the piece's start. `inner` is already relative to the
/// piece (see [`keyframes`]); `target` is the piece on the outer timeline.
///
/// Only the compound clip's envelope: shifted, exact. Both: sampled every
/// [`VOLUME_STEP`] and at every key, linear in between.
pub fn volume(
    outer: &Segment,
    inner: Option<&KeyframeTrack>,
    target: TimeRange,
) -> Option<KeyframeTrack> {
    let own = outer
        .keyframes
        .iter()
        .find(|k| k.property == AnimatableProperty::Volume && !k.keyframes.is_empty());
    let shift = target.start - outer.target_range.start;
    let Some(own) = own else {
        return inner.cloned();
    };
    let Some(inner) = inner.filter(|k| !k.keyframes.is_empty()) else {
        let mut moved = own.clone();
        for key in &mut moved.keyframes {
            key.time -= shift;
        }
        return Some(moved);
    };
    let mut times: Vec<Micros> = (0..=target.duration / VOLUME_STEP)
        .map(|n| n * VOLUME_STEP)
        .chain(std::iter::once(target.duration))
        .chain(own.keyframes.iter().map(|k| k.time - shift))
        .chain(inner.keyframes.iter().map(|k| k.time))
        .filter(|t| (0..=target.duration).contains(t))
        .collect();
    times.sort_unstable();
    times.dedup();
    Some(KeyframeTrack {
        property: AnimatableProperty::Volume,
        keyframes: times
            .into_iter()
            .map(|t| Keyframe {
                time: t,
                value: own.sample(t + shift).unwrap_or(1.0) * inner.sample(t).unwrap_or(1.0),
                easing: Easing::Linear,
            })
            .collect(),
    })
}
