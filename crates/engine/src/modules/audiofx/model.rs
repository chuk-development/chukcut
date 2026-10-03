//! A clip's audio processing as document data: [`AudioFx`].
//!
//! ## Where it lives
//!
//! One parameter block per clip in `MaterialPool::extras` under the key
//! `"audio_fx"`, referenced by id from `Segment::extras` — the home
//! `voice::cleanup` uses, for its reason: nothing hot reads it. The preview
//! mixer resolves it once per project snapshot and the export once per
//! segment, both through [`super::render`]. A typed pool category would have
//! changed `MaterialPool`, which every module constructs; an extras entry
//! changes nothing outside this module.
//!
//! The block holds the whole stack, not one effect: a clip has one ordered
//! list, and one block makes "the clip's effects" one lookup.
//!
//! ## Never edited in place
//!
//! Every change mints a new block and the undoable edit swaps the clip's
//! reference to it (remove + insert of the segment, as `fx::edit` and
//! `voice::cleanup` do), so undo is exact and the two halves of a split clip,
//! which share a block, never change under each other.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::modules::project::document::{new_id, Keyframe, Micros, Project, Segment};
use crate::modules::timeline::ops::EditCommand;

use super::catalog::descriptor;

/// The key of the block inside its `MaterialPool::extras` value.
pub const KEY: &str = "audio_fx";

/// One effect in a clip's stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioEffect {
    /// Stable inside the stack, so the UI and the CLI can name one effect
    /// across edits (the block itself gets a new id on every change).
    pub id: String,
    /// The catalog id, e.g. `"reverb"`. A string so a file from a later
    /// build with an effect this one lacks still opens; the unknown effect is
    /// skipped and survives a save.
    pub kind: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Values that differ from the catalog default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, f32>,
}

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}

impl AudioEffect {
    pub fn new(kind: &str) -> Self {
        Self {
            id: short_id(),
            kind: kind.to_string(),
            enabled: true,
            params: BTreeMap::new(),
        }
    }

    /// A parameter's value: stored, else the catalog default.
    pub fn value(&self, param: &str) -> Option<f32> {
        let spec = descriptor(&self.kind)?.param(param)?;
        Some(
            self.params
                .get(param)
                .map(|v| spec.clamp(*v))
                .unwrap_or(spec.default),
        )
    }
}

fn short_id() -> String {
    new_id().chars().filter(|c| *c != '-').take(8).collect()
}

/// What auto-ducking did to a music clip, so doing it again starts from the
/// clip's own volume and "remove ducking" can give it back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ducking {
    pub params: DuckParams,
    /// The clip's `Volume` keyframes before ducking wrote its own; empty when
    /// it had none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<Keyframe>,
}

/// The knobs of auto-ducking.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DuckParams {
    /// How far the music goes down under speech, in dB (positive).
    pub depth_db: f32,
    /// How long the music takes to go down, ending where speech starts.
    pub attack: Micros,
    /// How long it takes to come back up after speech stops.
    pub release: Micros,
    /// Speech windows quieter than this are ignored, in dBFS.
    #[serde(default = "default_threshold")]
    pub threshold_db: f32,
}

fn default_threshold() -> f32 {
    -45.0
}

impl Default for DuckParams {
    fn default() -> Self {
        Self {
            depth_db: 12.0,
            attack: 250_000,
            release: 500_000,
            threshold_db: default_threshold(),
        }
    }
}

/// Everything audio a clip carries beyond its volume.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AudioFx {
    /// CapCut's "Change audio pitch": a speed change moves the pitch with it,
    /// as a tape would. Off (the default) keeps the pitch and stretches time.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pitch_follows_speed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<AudioEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ducking: Option<Ducking>,
}

impl AudioFx {
    pub fn is_identity(&self) -> bool {
        !self.pitch_follows_speed && self.effects.is_empty() && self.ducking.is_none()
    }

    /// The effects that change the sound: enabled ones of known kinds.
    pub fn active_effects(&self) -> Vec<AudioEffect> {
        self.effects
            .iter()
            .filter(|e| e.enabled && descriptor(&e.kind).is_some())
            .cloned()
            .collect()
    }

    pub fn effect(&self, id: &str) -> Option<&AudioEffect> {
        self.effects.iter().find(|e| e.id == id)
    }

    pub fn effect_mut(&mut self, id: &str) -> Option<&mut AudioEffect> {
        self.effects.iter_mut().find(|e| e.id == id)
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({ KEY: self })
    }
}

/// The block an extras id resolves to, if it is one.
pub fn fx_entry(project: &Project, id: &str) -> Option<AudioFx> {
    project
        .materials
        .extras
        .get(id)
        .and_then(|value| value.get(KEY))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

/// The block on `segment`, with the id that holds it.
pub fn fx_of(project: &Project, segment: &Segment) -> Option<(String, AudioFx)> {
    segment
        .extras
        .iter()
        .find_map(|id| fx_entry(project, id).map(|fx| (id.clone(), fx)))
}

/// The block on `segment`, or an empty one.
pub fn fx_or_default(project: &Project, segment: &Segment) -> AudioFx {
    fx_of(project, segment)
        .map(|(_, fx)| fx)
        .unwrap_or_default()
}

/// Check every value against the catalog: clamp numbers, refuse non-finite
/// ones and parameters an effect does not have, and drop values equal to
/// their default so "at rest" has one spelling.
pub fn normalize(mut fx: AudioFx) -> Result<AudioFx, String> {
    for effect in &mut fx.effects {
        let Some(desc) = descriptor(&effect.kind) else {
            continue;
        };
        for (name, value) in effect.params.iter_mut() {
            let spec = desc
                .param(name)
                .ok_or_else(|| format!("{} has no parameter {name}", desc.label))?;
            if !value.is_finite() {
                return Err(format!("{} must be a finite number", spec.label));
            }
            *value = spec.clamp(*value);
        }
        effect
            .params
            .retain(|name, value| desc.param(name).is_some_and(|s| *value != s.default));
    }
    if let Some(d) = &fx.ducking {
        let p = d.params;
        if !p.depth_db.is_finite() || !p.threshold_db.is_finite() {
            return Err("ducking needs finite numbers".into());
        }
    }
    Ok(fx)
}

/// The edit that gives `segment_id` the block `fx` (or none), with `mutate`
/// applied to the segment in the same step, and the extras entry to put into
/// the pool before applying it.
///
/// `mutate` is how ducking writes its volume keyframes in the same undo step
/// as the block that remembers them.
pub fn set_fx_command(
    project: &Project,
    segment_id: &str,
    fx: AudioFx,
    label: &str,
    mutate: impl Fn(&mut Segment),
) -> Result<(Option<(String, serde_json::Value)>, EditCommand), String> {
    let (track, before) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let fx = normalize(fx)?;
    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .expect("segment found on this track");

    let entry = (!fx.is_identity()).then(|| (new_id(), fx.to_value()));
    let mut after = before.clone();
    // The reference keeps its place in `extras` when there is one, so a
    // change does not reorder the clip's other references.
    let slot = after
        .extras
        .iter()
        .position(|id| fx_entry(project, id).is_some());
    after.extras.retain(|id| fx_entry(project, id).is_none());
    if let Some((id, _)) = &entry {
        let at = slot.unwrap_or(after.extras.len()).min(after.extras.len());
        after.extras.insert(at, id.clone());
    }
    // Refuse a change that changes nothing: the same block (by content, not
    // id) and a segment the mutation leaves alone. `Segment` has no
    // `PartialEq`; its JSON is its identity.
    let mut probe = before.clone();
    mutate(&mut probe);
    let current = fx_of(project, before).map(|(_, fx)| fx).unwrap_or_default();
    if current == fx && serde_json::to_value(&probe).ok() == serde_json::to_value(before).ok() {
        return Err("the clip already sounds like that".into());
    }
    mutate(&mut after);
    let command = EditCommand::Composite {
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
    };
    Ok((entry, command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        AudioMaterial, CanvasConfig, TimeRange, Track, TrackKind, Transform,
    };
    use crate::modules::timeline::History;

    fn project() -> Project {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.audios.push(AudioMaterial {
            id: "a".into(),
            path: "/nonexistent/a.wav".into(),
            duration: 10_000_000,
            sample_rate: 48_000,
            channels: 2,
        });
        let mut lane = Track::new(TrackKind::Audio, "A1");
        lane.segments.push(Segment {
            id: "s".into(),
            material_id: "a".into(),
            target_range: TimeRange::new(0, 10_000_000),
            source_range: TimeRange::new(0, 10_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: vec!["other".into()],
            keyframes: Vec::new(),
        });
        p.tracks.push(lane);
        p
    }

    fn apply(p: &mut Project, history: &mut History, fx: AudioFx) {
        let (entry, command) = set_fx_command(p, "s", fx, "Audio effect", |_| {}).unwrap();
        if let Some((id, value)) = entry {
            p.materials.extras.insert(id, value);
        }
        history.apply(p, command).unwrap();
    }

    #[test]
    fn a_stack_round_trips_and_undoes_to_the_byte() {
        let mut p = project();
        let original = serde_json::to_string(&p).unwrap();
        let mut history = History::new();
        let mut reverb = AudioEffect::new("reverb");
        reverb.params.insert("mix".into(), 0.6);
        let fx = AudioFx {
            effects: vec![reverb.clone()],
            ..AudioFx::default()
        };
        apply(&mut p, &mut history, fx.clone());
        let (_, s) = p.segment("s").unwrap();
        assert_eq!(fx_or_default(&p, s), fx);
        // The block sits where the old one would have, after "other".
        assert_eq!(s.extras[0], "other");

        // A second change keeps the place and mints a new block.
        let mut louder = fx.clone();
        louder.effects[0].params.insert("mix".into(), 0.9);
        apply(&mut p, &mut history, louder.clone());
        let (_, s) = p.segment("s").unwrap();
        assert_eq!(s.extras.len(), 2);
        assert_eq!(fx_or_default(&p, s), louder);

        history.undo(&mut p).unwrap();
        history.undo(&mut p).unwrap();
        // The pool keeps the inert blocks (undo needs them); the tracks are
        // back exactly.
        let mut restored = p.clone();
        restored.materials.extras.clear();
        let mut expected: Project = serde_json::from_str(&original).unwrap();
        expected.materials.extras.clear();
        assert_eq!(
            serde_json::to_string(&restored.tracks).unwrap(),
            serde_json::to_string(&expected.tracks).unwrap()
        );
    }

    #[test]
    fn values_are_clamped_defaults_dropped_and_nonsense_refused() {
        let mut eq = AudioEffect::new("eq3");
        eq.params.insert("low".into(), 99.0);
        eq.params.insert("mid".into(), 0.0);
        let fx = normalize(AudioFx {
            effects: vec![eq.clone()],
            ..AudioFx::default()
        })
        .unwrap();
        assert_eq!(fx.effects[0].params.get("low"), Some(&18.0));
        assert!(!fx.effects[0].params.contains_key("mid"));

        eq.params.insert("bogus".into(), 1.0);
        assert!(normalize(AudioFx {
            effects: vec![eq.clone()],
            ..AudioFx::default()
        })
        .is_err());
        eq.params.remove("bogus");
        eq.params.insert("high".into(), f32::NAN);
        assert!(normalize(AudioFx {
            effects: vec![eq],
            ..AudioFx::default()
        })
        .is_err());
    }

    #[test]
    fn an_empty_block_clears_the_reference_and_the_same_change_twice_is_refused() {
        let mut p = project();
        let mut history = History::new();
        let fx = AudioFx {
            pitch_follows_speed: true,
            ..AudioFx::default()
        };
        apply(&mut p, &mut history, fx.clone());
        assert!(set_fx_command(&p, "s", fx, "x", |_| {}).is_err());
        apply(&mut p, &mut history, AudioFx::default());
        let (_, s) = p.segment("s").unwrap();
        assert!(fx_of(&p, s).is_none());
        assert_eq!(s.extras, vec!["other".to_string()]);
    }

    #[test]
    fn an_unknown_effect_survives_and_is_skipped() {
        let fx: AudioFx = serde_json::from_value(serde_json::json!({
            "effects": [{"id": "x", "kind": "from_the_future", "params": {"warp": 3.0}}]
        }))
        .unwrap();
        let normalised = normalize(fx.clone()).unwrap();
        assert_eq!(normalised, fx);
        assert!(normalised.active_effects().is_empty());
    }
}
