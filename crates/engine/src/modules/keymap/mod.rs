//! Keyboard shortcuts: one registry of every bindable action, three presets,
//! and the user's own changes on top, kept in `<config>/shortcuts.json`.
//!
//! The app binds keys only from here (`crates/app/src/editor/keymap.rs`),
//! its shortcuts sheet reads the same effective map, and the settings'
//! shortcut editor writes through [`commands`]. A change is an override per
//! action — the keys it has instead of the preset's, possibly none — so a
//! reset is deleting the override, and switching presets keeps the user's
//! changes on top of the new base.
//!
//! Not part of the document: shortcuts belong to the installation, like the
//! rest of the settings, and are not undoable.

pub mod commands;
pub mod keys;
pub mod registry;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use registry::{action, ActionSpec, Preset, ACTIONS, GROUPS};

/// What is stored: the base preset and the user's overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Keymap {
    #[serde(default)]
    pub preset: Preset,
    /// Action id → the keys it has instead of the preset's. Empty means
    /// "no key". Ordered, so the file diffs cleanly.
    #[serde(default)]
    pub overrides: BTreeMap<String, Vec<String>>,
}

/// One action's keys as they are in effect.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Binding {
    pub action: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    /// Canonical key strings (`keys::normalize`).
    pub keys: Vec<String>,
    /// Changed from the preset.
    pub overridden: bool,
    pub typing_off: bool,
}

impl Binding {
    /// The GPUI context a key of this action is bound in: plain keys and
    /// `typing_off` actions stay out of text fields.
    pub fn context_for(&self, key: &str) -> Option<&'static str> {
        if self.typing_off || !keys::has_command_modifier(key) {
            Some("!Input")
        } else {
            None
        }
    }
}

/// Two actions answering one key.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    pub key: String,
    pub actions: Vec<&'static str>,
}

impl Keymap {
    /// Every action with its keys in effect, in registry order.
    pub fn effective(&self) -> Vec<Binding> {
        ACTIONS
            .iter()
            .map(|spec| {
                let overridden = self.overrides.get(spec.id);
                let keys = match overridden {
                    Some(keys) => keys.clone(),
                    None => spec
                        .defaults(self.preset)
                        .iter()
                        .map(|k| keys::normalize(k).unwrap_or_else(|_| (*k).to_string()))
                        .collect(),
                };
                Binding {
                    action: spec.id,
                    group: spec.group,
                    label: spec.label,
                    keys,
                    overridden: overridden.is_some(),
                    typing_off: spec.typing_off,
                }
            })
            .collect()
    }

    /// The keys of `action` in effect.
    pub fn keys_of(&self, action: &str) -> Vec<String> {
        self.effective()
            .into_iter()
            .find(|b| b.action == action)
            .map(|b| b.keys)
            .unwrap_or_default()
    }

    /// Every key more than one action answers, or that starts another
    /// action's sequence.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let bindings = self.effective();
        let mut out: Vec<Conflict> = Vec::new();
        for (i, a) in bindings.iter().enumerate() {
            for key in &a.keys {
                let mut actions = vec![a.action];
                for b in bindings.iter().skip(i + 1) {
                    if b.keys.iter().any(|other| keys::collide(key, other)) {
                        actions.push(b.action);
                    }
                }
                if actions.len() > 1 && !out.iter().any(|c| c.key == *key) {
                    out.push(Conflict {
                        key: key.clone(),
                        actions,
                    });
                }
            }
        }
        out
    }

    /// The actions other than `action` that already answer any of `wanted`.
    pub fn clashes(&self, action: &str, wanted: &[String]) -> Vec<(String, &'static str)> {
        let mut out = Vec::new();
        for binding in self.effective() {
            if binding.action == action {
                continue;
            }
            for key in wanted {
                if binding.keys.iter().any(|k| keys::collide(k, key)) {
                    out.push((key.clone(), binding.action));
                }
            }
        }
        out
    }

    /// Give `action` exactly `wanted`. With `steal`, any other action that
    /// answers one of those keys loses it; without, a clash is refused, by
    /// name. Setting the preset's own keys removes the override.
    pub fn set(&mut self, action: &str, wanted: &[String], steal: bool) -> Result<(), String> {
        let spec =
            registry::action(action).ok_or_else(|| format!("there is no action {action}"))?;
        let mut keys: Vec<String> = Vec::new();
        for raw in wanted {
            let key = keys::normalize(raw)?;
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        let clashes = self.clashes(action, &keys);
        if !clashes.is_empty() && !steal {
            let (key, other) = &clashes[0];
            let label = registry::action(other).map_or(*other, |s| s.label);
            return Err(format!(
                "{} is already \u{201c}{label}\u{201d}",
                keys::display(key)
            ));
        }
        for (key, other) in clashes {
            let remaining: Vec<String> = self
                .keys_of(other)
                .into_iter()
                .filter(|k| !keys::collide(k, &key))
                .collect();
            self.put(other, remaining);
        }
        self.put(spec.id, keys);
        Ok(())
    }

    /// Store `keys` for `action`, or drop the override when they are the
    /// preset's.
    fn put(&mut self, action: &str, keys: Vec<String>) {
        let Some(spec) = registry::action(action) else {
            return;
        };
        let preset: Vec<String> = spec
            .defaults(self.preset)
            .iter()
            .filter_map(|k| keys::normalize(k).ok())
            .collect();
        if keys == preset {
            self.overrides.remove(action);
        } else {
            self.overrides.insert(action.to_string(), keys);
        }
    }

    /// Back to the preset's keys for `action`.
    pub fn reset(&mut self, action: &str) {
        self.overrides.remove(action);
    }

    /// Back to the preset's keys for everything.
    pub fn reset_all(&mut self) {
        self.overrides.clear();
    }

    /// A new base. The user's changes stay on top of it; an override that
    /// now equals the new preset's keys is dropped as redundant.
    pub fn set_preset(&mut self, preset: Preset) {
        self.preset = preset;
        let overrides = std::mem::take(&mut self.overrides);
        for (action, keys) in overrides {
            self.put(&action, keys);
        }
    }

    /// Drop overrides for actions this build does not know, and keys that do
    /// not parse — a file from a newer build, or edited by hand.
    fn sanitize(&mut self) {
        self.overrides
            .retain(|id, _| registry::action(id).is_some());
        for keys in self.overrides.values_mut() {
            *keys = keys
                .iter()
                .filter_map(|k| keys::normalize(k).ok())
                .collect();
        }
    }
}

/// Where the shortcuts live.
pub fn file() -> PathBuf {
    crate::modules::workspace::paths::config_root().join("shortcuts.json")
}

/// The keymap stored at `path`; the default when there is none. A file that
/// does not parse is logged and treated as absent rather than locking the
/// user out of every key.
pub fn load_from(path: &Path) -> Keymap {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Keymap::default();
    };
    match serde_json::from_str::<Keymap>(&raw) {
        Ok(mut keymap) => {
            keymap.sanitize();
            keymap
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "the shortcuts file does not parse");
            Keymap::default()
        }
    }
}

/// Write `keymap` to `path`, atomically.
pub fn save_to(path: &Path, keymap: &Keymap) -> Result<(), String> {
    let json = serde_json::to_string_pretty(keymap).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_preset_is_free_of_conflicts_and_parses() {
        for preset in Preset::ALL {
            let map = Keymap {
                preset,
                ..Keymap::default()
            };
            assert_eq!(map.conflicts(), Vec::new(), "{preset:?}");
            for spec in ACTIONS {
                for key in spec.defaults(preset) {
                    keys::normalize(key).unwrap_or_else(|e| panic!("{} {key}: {e}", spec.id));
                }
            }
        }
    }

    #[test]
    fn action_ids_are_unique_and_grouped() {
        for (i, a) in ACTIONS.iter().enumerate() {
            assert!(GROUPS.contains(&a.group), "{} in {}", a.id, a.group);
            assert!(
                ACTIONS[i + 1..].iter().all(|b| b.id != a.id),
                "{} twice",
                a.id
            );
        }
    }

    #[test]
    fn a_clash_is_refused_by_name_unless_the_key_is_taken_over() {
        let mut map = Keymap::default();
        let error = map.set("Export", &keys(&["Ctrl+S"]), false).unwrap_err();
        assert!(error.contains("Ctrl+S"), "{error}");
        assert!(error.contains("Save"), "{error}");
        assert_eq!(map.keys_of("Export"), keys(&["ctrl-e"]));

        map.set("Export", &keys(&["Ctrl+S"]), true).unwrap();
        assert_eq!(map.keys_of("Export"), keys(&["ctrl-s"]));
        assert!(map.keys_of("Save").is_empty());
        assert!(map.conflicts().is_empty());
        assert!(map.overrides.contains_key("Save"));
    }

    #[test]
    fn taking_one_key_leaves_the_other_keys_of_an_action() {
        let mut map = Keymap::default();
        map.set("Export", &keys(&["s"]), true).unwrap();
        assert_eq!(map.keys_of("Split"), keys(&["ctrl-b"]));
    }

    #[test]
    fn a_conflict_written_by_hand_is_reported() {
        let mut map = Keymap::default();
        map.overrides.insert("Export".into(), keys(&["ctrl-s"]));
        let conflicts = map.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].key, "ctrl-s");
        assert!(conflicts[0].actions.contains(&"Save"));
        assert!(conflicts[0].actions.contains(&"Export"));
    }

    #[test]
    fn resets_and_preset_switches() {
        let mut map = Keymap::default();
        map.set("Split", &keys(&["ctrl-k"]), false).unwrap();
        assert!(map
            .effective()
            .iter()
            .any(|b| b.action == "Split" && b.overridden));
        map.reset("Split");
        assert_eq!(map.keys_of("Split"), keys(&["s", "ctrl-b"]));

        // An override equal to the new preset's keys is dropped; others stay.
        map.set("Split", &keys(&["ctrl-k"]), false).unwrap();
        map.set("Export", &keys(&["ctrl-shift-e"]), false).unwrap();
        map.set_preset(Preset::Premiere);
        assert!(!map.overrides.contains_key("Split"));
        assert_eq!(map.keys_of("Export"), keys(&["ctrl-shift-e"]));
        map.reset_all();
        assert_eq!(map.keys_of("Export"), keys(&["ctrl-m"]));
        assert_eq!(map.preset, Preset::Premiere);
    }

    #[test]
    fn setting_the_preset_keys_removes_the_override() {
        let mut map = Keymap::default();
        map.set("Save", &keys(&["Ctrl+S"]), false).unwrap();
        assert!(map.overrides.is_empty());
    }

    #[test]
    fn the_keymap_persists_and_a_bad_file_is_survivable() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generated");
        let path = dir.join(format!(
            "shortcuts-{}.json",
            crate::modules::project::document::new_id()
        ));
        let mut map = Keymap::default();
        map.set_preset(Preset::Capcut);
        map.set("Export", &keys(&["ctrl-shift-e"]), false).unwrap();
        map.set("DetectBeats", &[], false).unwrap();
        save_to(&path, &map).unwrap();
        let back = load_from(&path);
        assert_eq!(back, map);

        // Unknown actions and unparseable keys are dropped, not fatal.
        std::fs::write(
            &path,
            r#"{"preset":"premiere","overrides":{"Nope":["x"],"Save":["ctrl-banana","ctrl-w"]}}"#,
        )
        .unwrap();
        let back = load_from(&path);
        assert_eq!(back.preset, Preset::Premiere);
        assert_eq!(back.keys_of("Save"), keys(&["ctrl-w"]));
        assert!(!back.overrides.contains_key("Nope"));

        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load_from(&path), Keymap::default());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(load_from(&path), Keymap::default());
    }

    #[test]
    fn plain_keys_stay_out_of_text_fields() {
        let map = Keymap::default();
        let bindings = map.effective();
        let split = bindings.iter().find(|b| b.action == "Split").unwrap();
        assert_eq!(split.context_for("s"), Some("!Input"));
        assert_eq!(split.context_for("ctrl-b"), None);
        let copy = bindings.iter().find(|b| b.action == "CopyClips").unwrap();
        assert_eq!(copy.context_for("ctrl-c"), Some("!Input"));
    }
}
