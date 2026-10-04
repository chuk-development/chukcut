//! The shortcut commands: what the settings' shortcut editor and the app's
//! key binding call. Every change is written through at once, like the rest
//! of the settings (decision 0004), and answers the keymap as stored.

use super::{file, load_from, save_to, Binding, Conflict, Keymap, Preset};

/// The keymap as stored, or the default.
pub fn keymap_get() -> Keymap {
    load_from(&file())
}

/// Every action with its keys in effect.
pub fn keymap_bindings() -> Vec<Binding> {
    keymap_get().effective()
}

/// Keys more than one action answers.
pub fn keymap_conflicts() -> Vec<Conflict> {
    keymap_get().conflicts()
}

fn change(edit: impl FnOnce(&mut Keymap) -> Result<(), String>) -> Result<Keymap, String> {
    let mut keymap = keymap_get();
    edit(&mut keymap)?;
    save_to(&file(), &keymap)?;
    Ok(keymap)
}

/// Give `action` exactly `keys`; `steal` takes them from any action that has
/// them, otherwise a clash is refused by name.
pub fn keymap_set(action: &str, keys: &[String], steal: bool) -> Result<Keymap, String> {
    change(|k| k.set(action, keys, steal))
}

/// `action` back to its preset keys.
pub fn keymap_reset(action: &str) -> Result<Keymap, String> {
    change(|k| {
        k.reset(action);
        Ok(())
    })
}

/// Every action back to its preset keys.
pub fn keymap_reset_all() -> Result<Keymap, String> {
    change(|k| {
        k.reset_all();
        Ok(())
    })
}

/// A new base preset; the user's changes stay on top.
pub fn keymap_set_preset(preset: Preset) -> Result<Keymap, String> {
    change(|k| {
        k.set_preset(preset);
        Ok(())
    })
}
