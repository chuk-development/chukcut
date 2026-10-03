//! What a saved file leaves out: parameter materials nothing in the document
//! reaches any more.
//!
//! Every grade commit, effect change, animation change and speed-curve edit
//! mints a new material and leaves the old one in the pool, because undo puts
//! the old id back on the clip (`docs/QA.md`: three of four colour materials
//! unreferenced after four grade edits). That is right for the session and
//! wrong for the file, which kept them for ever.
//!
//! So [`prune_unreferenced`] runs on the **copy that is written**, never on
//! the live pool. Undo, redo and the timeline clipboard resolve their ids
//! against the live pool, which keeps everything, so nothing they hold can
//! dangle — a stronger rule than asking `DocumentHistory::mentions` per id,
//! and one that does not depend on the history's depth cap. It also keeps the
//! round trip exact: what is written references everything it holds, so open
//! and save again writes the same bytes.
//!
//! Only parameter categories are pruned: colour adjustments, effects,
//! compositing (masks, keys, blend modes),
//! animations, speed curves, follow links and the `extras` map (analysis
//! results, voice cleanup, clip names). Media stays — an unused import is the
//! media library (decision 0009) — and so do titles' text, transitions,
//! motion tracks, link groups and licence records, which are either not
//! minted per edit or too expensive to recompute.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::document::Project;

/// Remove the parameter materials no clip, lane or kept material names.
/// Answers how many went.
///
/// "Names" is any string anywhere in the tracks, in a kept material, or in a
/// category that is never pruned — a generous reading, so a reference this
/// module was not told about (a stabilisation naming its camera path, a
/// follow naming its track) keeps its target rather than losing it.
pub fn prune_unreferenced(project: &mut Project) -> usize {
    let pool = &project.materials;
    let mut candidates: HashMap<String, Value> = HashMap::new();
    let mut add = |id: &str, value: Value| {
        candidates.insert(id.to_string(), value);
    };
    for m in &pool.color_adjusts {
        add(&m.id, to_value(m));
    }
    for m in &pool.effects {
        add(&m.id, to_value(m));
    }
    for m in &pool.compositing {
        add(&m.id, to_value(m));
    }
    for m in &pool.animations {
        add(&m.id, to_value(m));
    }
    for m in &pool.speed_curves {
        add(&m.id, to_value(m));
    }
    for m in &pool.follows {
        add(&m.id, to_value(m));
    }
    for (id, value) in &pool.extras {
        add(id, value.clone());
    }
    if candidates.is_empty() {
        return 0;
    }

    // Everything that is kept regardless names its references first.
    let mut queue: Vec<String> = Vec::new();
    let mut roots = |value: Value| strings_in(&value, &candidates, &mut queue);
    roots(to_value(&project.tracks));
    roots(to_value(&project.markers));
    roots(to_value(&pool.videos));
    roots(to_value(&pool.audios));
    roots(to_value(&pool.images));
    roots(to_value(&pool.texts));
    roots(to_value(&pool.transitions));
    roots(to_value(&pool.trackings));
    // Parked timelines and compound clips reference materials like the
    // active lanes do.
    roots(to_value(&pool.sequences));

    // Then whatever a reached material names, until nothing new turns up.
    let mut reached: HashSet<String> = HashSet::new();
    while let Some(id) = queue.pop() {
        if reached.insert(id.clone()) {
            if let Some(value) = candidates.get(&id) {
                strings_in(value, &candidates, &mut queue);
            }
        }
    }

    let pool = &mut project.materials;
    let before = pool.color_adjusts.len()
        + pool.effects.len()
        + pool.compositing.len()
        + pool.animations.len()
        + pool.speed_curves.len()
        + pool.follows.len()
        + pool.extras.len();
    pool.color_adjusts.retain(|m| reached.contains(&m.id));
    pool.effects.retain(|m| reached.contains(&m.id));
    pool.compositing.retain(|m| reached.contains(&m.id));
    pool.animations.retain(|m| reached.contains(&m.id));
    pool.speed_curves.retain(|m| reached.contains(&m.id));
    pool.follows.retain(|m| reached.contains(&m.id));
    pool.extras.retain(|id, _| reached.contains(id));
    let after = pool.color_adjusts.len()
        + pool.effects.len()
        + pool.compositing.len()
        + pool.animations.len()
        + pool.speed_curves.len()
        + pool.follows.len()
        + pool.extras.len();
    before - after
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    // A value that cannot be serialized cannot be saved either; `Null` names
    // nothing, and the save that follows reports the real error.
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Push every string in `value` (object keys included) that is a candidate id.
fn strings_in(value: &Value, candidates: &HashMap<String, Value>, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            if candidates.contains_key(s) {
                out.push(s.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                strings_in(item, candidates, out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if candidates.contains_key(key) {
                    out.push(key.clone());
                }
                strings_in(item, candidates, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, ColorAdjustMaterial, Segment, TimeRange, Track, TrackKind, Transform,
    };
    use crate::modules::project::{AnimationMaterial, EffectMaterial, SpeedCurveMaterial};
    use serde_json::json;

    fn clip(id: &str, extras: &[&str]) -> Segment {
        Segment {
            id: id.into(),
            material_id: "video".into(),
            target_range: TimeRange::new(0, 1_000_000),
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

    fn grade(id: &str) -> ColorAdjustMaterial {
        serde_json::from_value(json!({ "id": id })).unwrap()
    }

    fn effect(id: &str) -> EffectMaterial {
        let mut effect = EffectMaterial::new("blur");
        effect.id = id.into();
        effect
    }

    fn animation(id: &str) -> AnimationMaterial {
        let mut animation = AnimationMaterial::new();
        animation.id = id.into();
        animation
    }

    fn curve(id: &str) -> SpeedCurveMaterial {
        serde_json::from_value(json!({ "id": id, "points": [{ "source": 0, "speed": 2.0 }] }))
            .unwrap()
    }

    /// One clip using one of each kind, an effect clip on an effect lane, and
    /// an orphan of each kind beside them.
    fn project() -> Project {
        let mut p = Project::new("p", CanvasConfig::default(), 30.0);
        let mut video = Track::new(TrackKind::Video, "V");
        video
            .segments
            .push(clip("a", &["grade", "fx", "anim", "curve", "stab", "name"]));
        let mut lane = Track::new(TrackKind::Video, "FX");
        let mut effect_clip = clip("e", &[]);
        effect_clip.material_id = "lane-fx".into();
        lane.segments.push(effect_clip);
        p.tracks.push(video);
        p.tracks.push(lane);

        let pool = &mut p.materials;
        pool.color_adjusts = vec![grade("old-grade"), grade("grade")];
        pool.effects = vec![effect("fx"), effect("lane-fx"), effect("old-fx")];
        pool.animations = vec![animation("anim"), animation("old-anim")];
        pool.speed_curves = vec![curve("curve"), curve("old-curve")];
        // A stabilisation names its camera path; the path is reached through it.
        pool.extras.insert(
            "stab".into(),
            json!({ "kind": "stabilise", "motion_id": "path" }),
        );
        pool.extras
            .insert("path".into(), json!({ "kind": "motion", "samples": [] }));
        pool.extras
            .insert("name".into(), json!({ "clip_name": "Opening" }));
        pool.extras.insert(
            "old-stab".into(),
            json!({ "kind": "stabilise", "motion_id": "old-path" }),
        );
        pool.extras.insert(
            "old-path".into(),
            json!({ "kind": "motion", "samples": [] }),
        );
        p
    }

    #[test]
    fn unreferenced_parameters_go_and_referenced_ones_stay() {
        let mut p = project();
        let removed = prune_unreferenced(&mut p);
        let pool = &p.materials;
        let ids = |v: Vec<&str>| v.into_iter().map(String::from).collect::<Vec<_>>();
        assert_eq!(
            pool.color_adjusts
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>(),
            ids(vec!["grade"])
        );
        assert_eq!(
            pool.effects
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>(),
            ids(vec!["fx", "lane-fx"])
        );
        assert_eq!(pool.animations.len(), 1);
        assert_eq!(pool.speed_curves.len(), 1);
        assert_eq!(
            pool.extras.keys().cloned().collect::<Vec<_>>(),
            ids(vec!["name", "path", "stab"])
        );
        assert_eq!(removed, 6);
        // Nothing left to remove the second time: the file is a fixed point.
        assert_eq!(prune_unreferenced(&mut p), 0);
    }

    #[test]
    fn a_pruned_file_round_trips_byte_for_byte() {
        let mut p = project();
        prune_unreferenced(&mut p);
        let saved = serde_json::to_string_pretty(&p).unwrap();
        let mut reopened: Project = serde_json::from_str(&saved).unwrap();
        prune_unreferenced(&mut reopened);
        assert_eq!(serde_json::to_string_pretty(&reopened).unwrap(), saved);
    }

    #[test]
    fn media_and_other_categories_are_never_pruned() {
        let mut p = Project::new("p", CanvasConfig::default(), 30.0);
        p.materials.extras.insert("orphan".into(), json!({}));
        p.materials.links.insert("lonely-group".into());
        prune_unreferenced(&mut p);
        assert!(p.materials.extras.is_empty());
        assert!(p.materials.links.contains("lonely-group"));
    }
}
