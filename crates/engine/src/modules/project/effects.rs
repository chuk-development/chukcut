//! Built-in video effects as document data: [`EffectMaterial`].
//!
//! An effect is a parameter block in the material pool
//! (`MaterialPool::effects`), referenced by id. It reaches the picture in one
//! of two ways, and both are the same material:
//!
//! - **On a clip.** The id sits in the clip's `Segment::extras`, in
//!   application order, exactly like a colour adjustment or a transition. The
//!   effect sees only that clip.
//! - **As an effect clip.** A segment on an effect lane (`TrackKind::Effect`)
//!   whose `material_id` names the effect. For its duration it applies to
//!   everything composited *below* it, CapCut's adjustment layer. Further
//!   effects may stack on it through its own `extras`.
//!
//! `docs/decisions/0012-effects-on-clips-and-effect-clips.md` has the
//! reasoning; `modules/fx` has the catalog, the edit builders and the
//! renderer.
//!
//! ## Schema safety
//!
//! - `kind` is a string, not an enum. A project written by a later build with
//!   an effect this one does not know still opens; the unknown effect renders
//!   as nothing and survives a save untouched.
//! - `params` holds only values the user changed. A parameter missing from the
//!   map reads its catalog default, so a new parameter added to an effect later
//!   changes no existing file, and a default that is tuned later moves every
//!   clip that never touched it — which is what a default is for.
//! - Every optional field is skipped on save at rest, and the pool category is
//!   skipped when empty, so a project that never uses effects saves
//!   byte-identical to one written before they existed. A test pins that.
//!
//! ## Time
//!
//! Keyframe times, and the clock that animated effects (shake, glitch, a light
//! sweep) run on, are in the clip's **source time**, in microseconds. A clip
//! split in two therefore keeps one continuous animation across the cut
//! without anything being rewritten, and the right half — which shares the
//! material, because materials are never edited in place — reads the same
//! keyframes at the right instants. An effect clip's source starts at 0 when
//! it is created, so for it source time is simply time since its head.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::document::{Easing, Id, Keyframe, Micros};

/// One parameter's stored value.
///
/// Untagged, so the JSON is a bare number or a bare `[r, g, b, a]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EffectValue {
    Number(f32),
    /// Linear RGBA, straight alpha.
    Color([f32; 4]),
}

impl EffectValue {
    pub fn number(self) -> Option<f32> {
        match self {
            EffectValue::Number(v) => Some(v),
            EffectValue::Color(_) => None,
        }
    }

    pub fn color(self) -> Option<[f32; 4]> {
        match self {
            EffectValue::Color(c) => Some(c),
            EffectValue::Number(_) => None,
        }
    }

    pub fn is_finite(self) -> bool {
        match self {
            EffectValue::Number(v) => v.is_finite(),
            EffectValue::Color(c) => c.iter().all(|v| v.is_finite()),
        }
    }
}

/// One built-in effect with its parameters. See the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectMaterial {
    pub id: Id,
    /// The catalog id, e.g. `"gaussian_blur"`. See `modules/fx/catalog.rs`.
    pub kind: String,
    /// Switched off effects stay in the stack, with their values, and render
    /// as if absent: the eye icon in the inspector.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Values that differ from the catalog default, by parameter id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, EffectValue>,
    /// Animated numeric parameters: keyframes sorted by time, in the clip's
    /// source time. A parameter with keyframes ignores its static value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keyframes: BTreeMap<String, Vec<Keyframe>>,
    /// Seeds the random effects (shake, glitch, grain), so two clips with the
    /// same shake do not shake in step and one clip shakes the same way in the
    /// preview, in the export and next week.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seed: u32,
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl EffectMaterial {
    /// A fresh effect of `kind` with every parameter at its default and a
    /// seed derived from its new id.
    pub fn new(kind: impl Into<String>) -> Self {
        let id = super::document::new_id();
        let seed = seed_from(&id);
        Self {
            id,
            kind: kind.into(),
            enabled: true,
            params: BTreeMap::new(),
            keyframes: BTreeMap::new(),
            seed,
        }
    }

    /// The stored static value of a parameter, without keyframes or defaults.
    pub fn stored(&self, param: &str) -> Option<EffectValue> {
        self.params.get(param).copied()
    }

    /// A numeric parameter at `source_time`: its keyframes when it has any,
    /// else its stored value, else `default`.
    pub fn number_at(&self, param: &str, source_time: Micros, default: f32) -> f32 {
        if let Some(keys) = self.keyframes.get(param).filter(|k| !k.is_empty()) {
            return sample(keys, source_time);
        }
        self.params
            .get(param)
            .and_then(|v| v.number())
            .unwrap_or(default)
    }

    /// A colour parameter, or `default`. Colours are not animated.
    pub fn color(&self, param: &str, default: [f32; 4]) -> [f32; 4] {
        self.params
            .get(param)
            .and_then(|v| v.color())
            .unwrap_or(default)
    }

    /// The first value on this effect that is not a finite number, by name.
    pub fn non_finite_field(&self) -> Option<String> {
        for (name, value) in &self.params {
            if !value.is_finite() {
                return Some(name.clone());
            }
        }
        for (name, keys) in &self.keyframes {
            if keys.iter().any(|k| !k.value.is_finite()) {
                return Some(format!("{name} keyframe"));
            }
        }
        None
    }
}

/// A stable 32-bit seed from an id: FNV-1a, never 0 (0 is "no seed" on disk).
pub fn seed_from(id: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in id.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash.max(1)
}

/// The value of a sorted keyframe list at `time`, clamped at both ends — the
/// same rule as `KeyframeTrack::sample`, which this cannot reuse because that
/// type is tied to a transform property.
pub fn sample(keys: &[Keyframe], time: Micros) -> f32 {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return 0.0;
    };
    if time <= first.time {
        return first.value;
    }
    if time >= last.time {
        return last.value;
    }
    let idx = keys.partition_point(|k| k.time <= time);
    let a = &keys[idx - 1];
    let b = &keys[idx];
    let span = (b.time - a.time) as f32;
    let t = if span <= 0.0 {
        0.0
    } else {
        (time - a.time) as f32 / span
    };
    a.value + (b.value - a.value) * a.easing.apply(t)
}

/// Insert or replace the keyframe at `time` in a sorted list.
pub fn put_keyframe(keys: &mut Vec<Keyframe>, time: Micros, value: f32, easing: Easing) {
    match keys.binary_search_by_key(&time, |k| k.time) {
        Ok(i) => {
            keys[i].value = value;
        }
        Err(i) => keys.insert(
            i,
            Keyframe {
                time,
                value,
                easing,
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resting_effect_saves_only_its_identity() {
        let mut effect = EffectMaterial::new("gaussian_blur");
        effect.id = "e".into();
        effect.seed = 0;
        let json = serde_json::to_string(&effect).unwrap();
        assert_eq!(json, r#"{"id":"e","kind":"gaussian_blur"}"#);
        let back: EffectMaterial = serde_json::from_str(&json).unwrap();
        assert!(back.enabled);
        assert_eq!(back, effect);
    }

    #[test]
    fn values_round_trip_as_bare_numbers_and_colours() {
        let mut effect = EffectMaterial::new("glow");
        effect
            .params
            .insert("intensity".into(), EffectValue::Number(0.25));
        effect
            .params
            .insert("tint".into(), EffectValue::Color([1.0, 0.5, 0.0, 1.0]));
        let json = serde_json::to_value(&effect).unwrap();
        assert_eq!(json["params"]["intensity"], serde_json::json!(0.25));
        assert_eq!(
            json["params"]["tint"],
            serde_json::json!([1.0, 0.5, 0.0, 1.0])
        );
        let back: EffectMaterial = serde_json::from_value(json).unwrap();
        assert_eq!(back, effect);
    }

    #[test]
    fn an_unknown_kind_and_unknown_fields_still_load() {
        let json = r#"{"id":"x","kind":"from_the_future","enabled":false,
                       "params":{"warp":3},"brand_new_field":true}"#;
        let effect: EffectMaterial = serde_json::from_str(json).unwrap();
        assert_eq!(effect.kind, "from_the_future");
        assert!(!effect.enabled);
        assert_eq!(effect.number_at("warp", 0, 0.0), 3.0);
    }

    #[test]
    fn keyframes_win_over_the_static_value_and_clamp_at_the_ends() {
        let mut effect = EffectMaterial::new("gaussian_blur");
        effect
            .params
            .insert("radius".into(), EffectValue::Number(50.0));
        let mut keys = Vec::new();
        put_keyframe(&mut keys, 1_000_000, 0.0, Easing::Linear);
        put_keyframe(&mut keys, 3_000_000, 100.0, Easing::Linear);
        effect.keyframes.insert("radius".into(), keys);
        assert_eq!(effect.number_at("radius", 0, 10.0), 0.0);
        assert_eq!(effect.number_at("radius", 2_000_000, 10.0), 50.0);
        assert_eq!(effect.number_at("radius", 9_000_000, 10.0), 100.0);
        // A parameter with neither reads the default.
        assert_eq!(effect.number_at("other", 0, 7.0), 7.0);
    }

    #[test]
    fn putting_a_keyframe_at_an_existing_time_replaces_it() {
        let mut keys = Vec::new();
        put_keyframe(&mut keys, 2, 1.0, Easing::Linear);
        put_keyframe(&mut keys, 1, 1.0, Easing::Linear);
        put_keyframe(&mut keys, 2, 5.0, Easing::Linear);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].time, 1);
        assert_eq!(keys[1].value, 5.0);
    }

    #[test]
    fn a_project_without_effects_saves_no_effects_key_and_one_with_them_round_trips() {
        use crate::modules::project::document::{CanvasConfig, Project};
        let mut project = Project::new("p", CanvasConfig::default(), 30.0);
        let json = serde_json::to_value(&project).unwrap();
        assert!(json["materials"].get("effects").is_none());

        let mut effect = EffectMaterial::new("glow");
        effect
            .params
            .insert("intensity".into(), EffectValue::Number(10.0));
        project.materials.effects.push(effect.clone());
        let text = serde_json::to_string(&project).unwrap();
        let back: Project = serde_json::from_str(&text).unwrap();
        assert_eq!(back.materials.effects, vec![effect]);
    }

    #[test]
    fn seeds_are_stable_and_never_zero() {
        assert_eq!(seed_from("abc"), seed_from("abc"));
        assert_ne!(seed_from("abc"), seed_from("abd"));
        assert_ne!(seed_from(""), 0);
    }
}
