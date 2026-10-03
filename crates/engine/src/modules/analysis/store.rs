//! Where analysis results live in the document.
//!
//! Results that drive edits — scene cuts, beats, the camera path a clip is
//! stabilised along — go **into the project** (`docs/research/ml-features.md`
//! §5.6): re-running an analysis must not move a cut, and the project must
//! render the same on another machine without the cache.
//!
//! They are entries in `MaterialPool::extras`, the map the document keeps for
//! parameter blocks "so adding a new kind does not change the schema", each a
//! JSON object tagged with a `kind`, referenced from the `extras` of the clip
//! they belong to. That is the voice-cleanup pattern (`voice/cleanup.rs`), and
//! it buys the same three things: no schema change, no new undo variant (the
//! clip swaps one id for another through `RemoveSegment` + `InsertSegment`,
//! one `Composite`, one undo step), and a split clip keeps its analysis
//! because `split_at` clones the clip's `extras`.
//!
//! Entries are **immutable**: a changed result is a new entry under a new id,
//! and undo puts the old id back on the clip. That is also what lets the
//! compositor memoise a parsed entry by id (`stabilise.rs`). An entry no clip
//! names any more stays in the pool, inert, like an unreferenced link group.
//!
//! Every time inside an entry is **source** time — microseconds into the file
//! — so trimming, slipping, splitting and retiming a clip never invalidate it.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::modules::project::document::{new_id, Id, Micros, Project, Segment, TimeRange};
use crate::modules::project::TimeMap;
use crate::modules::timeline::ops::EditCommand;

/// Scene cuts of a video file.
pub const SCENES: &str = "analysis.scenes";
/// Beats of a sound.
pub const BEATS: &str = "analysis.beats";
/// The camera path of a video file: the heavy half of stabilisation.
pub const MOTION: &str = "analysis.motion";
/// How a clip is stabilised along a camera path: the light half, replaced on
/// every settings change so the path itself is stored once.
pub const STABILISE: &str = "analysis.stabilise";

/// Scene cuts found in a stretch of a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneCuts {
    pub media_id: Id,
    /// The source range that was analysed; there are no cuts outside it
    /// because nobody looked.
    pub analysed: TimeRange,
    /// Source time of the first frame of each new shot, ascending.
    pub cuts: Vec<Micros>,
    /// The sensitivity the cuts were found with, `0..=1`.
    pub sensitivity: f32,
}

/// Beats found in a stretch of a sound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Beats {
    pub media_id: Id,
    pub analysed: TimeRange,
    /// Source time of each beat, ascending.
    pub beats: Vec<Micros>,
    /// The tempo the beats were tracked at.
    pub bpm: f32,
}

/// One frame of a camera path: where the picture had drifted to by `t`,
/// relative to the first analysed frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PathSample {
    /// Source time of the frame.
    pub t: Micros,
    /// Horizontal drift as a fraction of the frame width, +x right.
    pub x: f32,
    /// Vertical drift as a fraction of the frame height, +y down.
    pub y: f32,
    /// Rotation in degrees, clockwise on screen.
    pub a: f32,
    /// The first frame of a new shot: the path is smoothed on each side of
    /// it separately, so a cut never pulls the frames before it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cut: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The camera path of a stretch of a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraPath {
    pub media_id: Id,
    pub analysed: TimeRange,
    /// Width over height of the frames the path was measured on, for turning
    /// a rotation into the zoom that hides its corners.
    pub aspect: f32,
    pub samples: Vec<PathSample>,
}

/// How a clip is stabilised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stabilise {
    /// The [`CameraPath`] entry.
    pub motion_id: Id,
    pub enabled: bool,
    /// `0..=1`: how much of the shake goes. 1 locks the camera like a tripod.
    pub strength: f32,
    /// How far in the picture is zoomed to hide the moving edges, as a
    /// fraction cropped off: `None` picks the least that hides them all.
    #[serde(default)]
    pub crop: Option<f32>,
}

/// The `kind` an entry is stored under, so `entry` can check it.
pub trait Kind: Serialize + DeserializeOwned {
    const KIND: &'static str;
}

impl Kind for SceneCuts {
    const KIND: &'static str = SCENES;
}
impl Kind for Beats {
    const KIND: &'static str = BEATS;
}
impl Kind for CameraPath {
    const KIND: &'static str = MOTION;
}
impl Kind for Stabilise {
    const KIND: &'static str = STABILISE;
}

/// The JSON stored for `value`: its fields plus `"kind"`.
pub fn to_value<T: Kind>(value: &T) -> serde_json::Value {
    let mut json = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    if let Some(object) = json.as_object_mut() {
        object.insert("kind".into(), T::KIND.into());
    }
    json
}

/// The kind tag of the pool entry `id`, without parsing the rest of it.
pub fn kind_of<'a>(project: &'a Project, id: &str) -> Option<&'a str> {
    project.materials.extras.get(id)?.get("kind")?.as_str()
}

/// The pool entry `id`, when it is a `T`.
pub fn entry<T: Kind>(project: &Project, id: &str) -> Option<T> {
    let value = project.materials.extras.get(id)?;
    if value.get("kind")?.as_str()? != T::KIND {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

/// The `T` that `segment` carries, with its id.
pub fn entry_of<T: Kind>(project: &Project, segment: &Segment) -> Option<(Id, T)> {
    segment
        .extras
        .iter()
        .filter(|id| kind_of(project, id) == Some(T::KIND))
        .find_map(|id| entry::<T>(project, id).map(|e| (id.clone(), e)))
}

/// A pool entry to insert before a command runs, and to take out again if
/// the command is refused.
pub type NewEntry = (Id, serde_json::Value);

/// A fresh entry for `value`.
pub fn new_entry<T: Kind>(value: &T) -> NewEntry {
    (new_id(), to_value(value))
}

/// The edit that makes `segment_id` carry `entry` (an id already in the pool,
/// or about to be) in place of whatever entry of kind `kind` it carried, or
/// carry none of that kind when `entry` is `None`.
///
/// `RemoveSegment` + `InsertSegment` in one `Composite`, exactly like
/// `voice::cleanup::set_cleanup_command`: no `EditCommand` writes a clip's
/// `extras` alone, and swapping the whole clip undoes byte for byte.
pub fn swap_entry(
    project: &Project,
    segment_id: &str,
    kind: &str,
    entry: Option<&str>,
    label: &str,
) -> Result<EditCommand, String> {
    let (track, before) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let mut after = before.clone();
    after.extras.retain(|id| kind_of(project, id) != Some(kind));
    if let Some(id) = entry {
        after.extras.push(id.to_string());
    }
    if after.extras == before.extras {
        return Err("nothing to change".into());
    }
    Ok(EditCommand::Composite {
        label: label.into(),
        commands: vec![
            EditCommand::RemoveSegment {
                track_id: track.id.clone(),
                segment: before.clone(),
                index,
            },
            EditCommand::InsertSegment {
                track_id: track.id.clone(),
                segment: after,
                index,
            },
        ],
    })
}

/// Where source time `source` of the clip `map` describes sits on the
/// timeline, or `None` when the clip does not show that instant.
///
/// Through the clip's `TimeMap` (`project.materials.time_map(segment)`), so a
/// speed curve is honoured: `Segment::source_time_at` knows only the constant
/// speed and put every beat and scene cut of a ramped clip on the wrong frame.
/// Found by bisection on the forward mapping rather than by inverting it, so
/// it stays right for any mapping that only moves forward without this module
/// knowing how the mapping is computed.
pub fn timeline_time_of(map: &TimeMap<'_>, source: Micros) -> Option<Micros> {
    let range = map.segment.target_range;
    if range.duration <= 0 {
        return None;
    }
    let last = range.end() - 1;
    let first_source = map.source_time_at(range.start)?;
    let last_source = map.source_time_at(last)?;
    if source < first_source || source > last_source {
        return None;
    }
    // The first timeline instant whose source time reaches `source`.
    let (mut lo, mut hi) = (range.start, last);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match map.source_time_at(mid) {
            Some(t) if t >= source => hi = mid,
            _ => lo = mid + 1,
        }
    }
    Some(lo)
}

/// `time` moved onto the nearest frame boundary of a timeline at `fps`.
pub fn snap_to_frame(time: Micros, fps: f64) -> Micros {
    if !(fps.is_finite() && fps > 0.0) {
        return time;
    }
    let frame = (time as f64 * fps / 1e6).round();
    (frame * 1e6 / fps).round() as Micros
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Track, TrackKind, Transform};

    pub(crate) fn segment(start: Micros, duration: Micros, source: Micros, speed: f32) -> Segment {
        Segment {
            id: "s".into(),
            material_id: "m".into(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(
                source,
                crate::modules::project::document::source_duration_for(duration, speed),
            ),
            render_index: 0,
            speed,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    #[test]
    fn timeline_time_inverts_source_time_at_any_speed() {
        for speed in [0.5f32, 1.0, 2.0, 3.0] {
            let s = segment(1_000_000, 4_000_000, 2_000_000, speed);
            for t in [1_000_000, 1_333_333, 2_500_000, 4_999_999] {
                let src = s.source_time_at(t).unwrap();
                let map = TimeMap {
                    segment: &s,
                    curve: None,
                };
                let back = timeline_time_of(&map, src).unwrap();
                assert_eq!(s.source_time_at(back), Some(src), "speed {speed}");
                assert!((back - t).abs() <= speed.recip().ceil() as Micros + 1);
            }
        }
        let s = segment(0, 1_000_000, 0, 1.0);
        let map = TimeMap {
            segment: &s,
            curve: None,
        };
        assert_eq!(timeline_time_of(&map, 2_000_000), None);
    }

    #[test]
    fn frame_snapping_lands_on_the_grid() {
        assert_eq!(snap_to_frame(1_010_000, 30.0), 1_000_000);
        assert_eq!(snap_to_frame(33_000, 30.0), 33_333);
    }

    #[test]
    fn swapping_an_entry_is_one_undoable_step() {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        let mut track = Track::new(TrackKind::Video, "V");
        track.segments.push(segment(0, 1_000_000, 0, 1.0));
        p.tracks.push(track);
        let cuts = SceneCuts {
            media_id: "m".into(),
            analysed: TimeRange::new(0, 1_000_000),
            cuts: vec![500_000],
            sensitivity: 0.5,
        };
        let (id, value) = new_entry(&cuts);
        p.materials.extras.insert(id.clone(), value);
        let command = swap_entry(&p, "s", SCENES, Some(&id), "Detect scenes").unwrap();
        command.apply(&mut p).unwrap();
        let (_, s) = p.segment("s").unwrap();
        assert_eq!(entry_of::<SceneCuts>(&p, s).map(|(_, c)| c), Some(cuts));
        command.invert().apply(&mut p).unwrap();
        let (_, s) = p.segment("s").unwrap();
        assert!(entry_of::<SceneCuts>(&p, s).is_none());
        // The wrong kind is not read as this one.
        assert!(entry::<Beats>(&p, &id).is_none());
    }
}
