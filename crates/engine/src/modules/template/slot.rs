//! Slots: the clips of a template that the user's media replaces.
//!
//! A slot is an ordinary clip that carries a marker — a `template_slot`
//! entry in `MaterialPool::extras`, referenced from the clip's own `extras`
//! list, the way analysis results are (`analysis::store`). So a slot keeps
//! everything a clip can have (animations, effects, transitions, keyframes,
//! a grade), the saved project needs no new field, and a slot survives a save
//! because the clip names its marker (`project::prune`).
//!
//! The marker stays after the slot is filled, with `filled: true`, so the
//! fill dialog can still offer "replace" on a project made from a template.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::modules::project::document::{new_id, Id, Micros, Project, Segment};

/// The `kind` tag of a slot marker in `MaterialPool::extras`.
pub const SLOT_KIND: &str = "template_slot";

/// Which media a slot takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotMedia {
    /// A video or a still.
    #[default]
    Any,
    /// Moving pictures only: a still would sit there for the slot's length.
    Video,
    /// Stills only.
    Image,
}

impl SlotMedia {
    pub fn label(self) -> &'static str {
        match self {
            SlotMedia::Any => "video or photo",
            SlotMedia::Video => "video",
            SlotMedia::Image => "photo",
        }
    }
}

/// What a slot's marker stores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotMarker {
    /// The fill order, from 1. Media is put into slots in this order.
    pub index: u32,
    /// What the template author says goes here ("Opening shot").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The shape of the picture on the canvas, reduced (`[9, 16]`). Media
    /// of another shape is cropped to it, centred.
    pub aspect: [u32; 2],
    #[serde(default)]
    pub accepts: SlotMedia,
    /// Whether the user's media is in the slot, or still the placeholder.
    #[serde(default)]
    pub filled: bool,
}

impl SlotMarker {
    /// The JSON stored in the pool: the fields plus `"kind"`.
    pub fn to_value(&self) -> Value {
        let mut json = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(object) = json.as_object_mut() {
            object.insert("kind".into(), SLOT_KIND.into());
        }
        json
    }

    /// A fresh pool entry for this marker.
    pub fn new_entry(&self) -> (Id, Value) {
        (new_id(), self.to_value())
    }
}

/// The marker entry `id`, when it is one.
pub fn marker(project: &Project, id: &str) -> Option<SlotMarker> {
    let value = project.materials.extras.get(id)?;
    if value.get("kind")?.as_str()? != SLOT_KIND {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

/// Whether the pool entry `id` is a slot marker.
pub fn is_marker(project: &Project, id: &str) -> bool {
    project
        .materials
        .extras
        .get(id)
        .and_then(|v| v.get("kind"))
        .and_then(Value::as_str)
        == Some(SLOT_KIND)
}

/// The marker `segment` carries, with its id.
pub fn marker_of(project: &Project, segment: &Segment) -> Option<(Id, SlotMarker)> {
    segment
        .extras
        .iter()
        .find_map(|id| marker(project, id).map(|m| (id.clone(), m)))
}

/// One slot as the shells show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slot {
    pub index: u32,
    pub segment_id: String,
    pub track_id: String,
    pub label: Option<String>,
    pub start: Micros,
    pub duration: Micros,
    pub aspect: [u32; 2],
    pub accepts: SlotMedia,
    pub filled: bool,
    /// The file in the slot once it is filled.
    pub media_path: Option<String>,
}

/// Every slot of `project`, in fill order: by index, then by time, then by
/// lane, so two slots that share an index (a split clip) still come out in a
/// fixed order.
pub fn slots(project: &Project) -> Vec<Slot> {
    let mut out: Vec<(usize, Slot)> = Vec::new();
    for (lane, track) in project.tracks.iter().enumerate() {
        for segment in &track.segments {
            let Some((_, marker)) = marker_of(project, segment) else {
                continue;
            };
            let pool = &project.materials;
            let media_path = marker
                .filled
                .then(|| {
                    pool.video(&segment.material_id)
                        .map(|m| m.path.clone())
                        .or_else(|| pool.image(&segment.material_id).map(|m| m.path.clone()))
                })
                .flatten();
            out.push((
                lane,
                Slot {
                    index: marker.index,
                    segment_id: segment.id.clone(),
                    track_id: track.id.clone(),
                    label: marker.label.clone(),
                    start: segment.target_range.start,
                    duration: segment.target_range.duration,
                    aspect: marker.aspect,
                    accepts: marker.accepts,
                    filled: marker.filled,
                    media_path,
                },
            ));
        }
    }
    out.sort_by_key(|(lane, slot)| (slot.index, slot.start, *lane));
    out.into_iter().map(|(_, slot)| slot).collect()
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// `width:height` reduced, snapped to a common shape when it is within half
/// a percent of one, so a 1078×1920 phone clip is 9:16 and not 539:960.
pub fn reduce_aspect(width: u32, height: u32) -> [u32; 2] {
    let (w, h) = (width.max(1), height.max(1));
    const COMMON: [[u32; 2]; 10] = [
        [9, 16],
        [16, 9],
        [1, 1],
        [4, 5],
        [5, 4],
        [4, 3],
        [3, 4],
        [21, 9],
        [9, 8],
        [2, 3],
    ];
    let ratio = w as f64 / h as f64;
    for [a, b] in COMMON {
        let common = a as f64 / b as f64;
        if (ratio / common - 1.0).abs() < 0.005 {
            return [a, b];
        }
    }
    let g = gcd(w, h);
    [w / g, h / g]
}

/// An aspect as a number, width over height.
pub fn ratio(aspect: [u32; 2]) -> f64 {
    aspect[0].max(1) as f64 / aspect[1].max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, TimeRange, Track, TrackKind};
    use crate::modules::project::Transform;

    fn clip(id: &str, start: Micros, extras: &[&str]) -> Segment {
        Segment {
            id: id.into(),
            material_id: "m".into(),
            target_range: TimeRange::new(start, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: extras.iter().map(|s| s.to_string()).collect(),
            keyframes: Vec::new(),
        }
    }

    #[test]
    fn aspects_reduce_and_snap_to_the_shapes_people_deliver() {
        assert_eq!(reduce_aspect(1080, 1920), [9, 16]);
        assert_eq!(reduce_aspect(1078, 1920), [9, 16]);
        assert_eq!(reduce_aspect(3840, 2160), [16, 9]);
        assert_eq!(reduce_aspect(1080, 960), [9, 8]);
        assert_eq!(reduce_aspect(700, 300), [21, 9]);
        assert_eq!(reduce_aspect(700, 310), [70, 31]);
        assert_eq!(reduce_aspect(0, 0), [1, 1]);
    }

    #[test]
    fn slots_come_out_in_index_order_whatever_the_timeline_order() {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        let first = SlotMarker {
            index: 1,
            label: Some("Opening".into()),
            aspect: [9, 16],
            accepts: SlotMedia::Any,
            filled: false,
        };
        let second = SlotMarker {
            index: 2,
            ..first.clone()
        };
        p.materials.extras.insert("a".into(), second.to_value());
        p.materials.extras.insert("b".into(), first.to_value());
        p.materials
            .extras
            .insert("other".into(), serde_json::json!({"kind": "beats"}));
        let mut track = Track::new(TrackKind::Video, "V");
        track.segments.push(clip("early", 0, &["a"]));
        track
            .segments
            .push(clip("late", 2_000_000, &["other", "b"]));
        track.segments.push(clip("plain", 4_000_000, &[]));
        p.tracks.push(track);

        let slots = slots(&p);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].segment_id, "late");
        assert_eq!(slots[0].label.as_deref(), Some("Opening"));
        assert_eq!(slots[1].segment_id, "early");
        assert!(is_marker(&p, "a"));
        assert!(!is_marker(&p, "other"));
    }
}
