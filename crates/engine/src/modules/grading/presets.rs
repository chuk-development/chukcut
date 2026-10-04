//! Grade presets: a clip's whole grade saved under a name, for the Filters
//! tab's "My presets" and for any other clip.
//!
//! A preset is a JSON file in `paths::grade_presets_dir()` holding a
//! [`GradeEdit`] — every slider, the curves, the wheels, HSL and the LUT
//! reference — so applying it gives exactly the grade that was saved. Not a
//! baked `.cube`: a LUT cannot hold exposure in light, sharpen, clarity,
//! vignette or grain, and a baked look could no longer be edited control by
//! control. The LUT a preset references stays a path into the LUT library,
//! which outlives downloads.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::modules::inspector::edit::GradeEdit;
use crate::modules::workspace::atomic::write_atomically;

/// The file format's version: a preset written by a newer build with a
/// higher number is listed but refused when applied.
const VERSION: u32 = 1;
const MAX_NAME: usize = 60;

/// The file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradePreset {
    pub version: u32,
    pub name: String,
    pub grade: GradeEdit,
}

/// What the panel lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PresetEntry {
    pub name: String,
    pub path: String,
}

/// A name as a file name: the characters a file system or a shell would
/// trip on become `-`.
fn file_stem(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string()
}

pub fn path_in(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.json", file_stem(name)))
}

fn clean_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || file_stem(name).is_empty() {
        return Err("a preset needs a name".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("a preset name has at most {MAX_NAME} characters"));
    }
    Ok(name.to_string())
}

/// Save `grade` as `name`. A preset of that name is replaced only when
/// `replace` is set; otherwise the clash is refused in words.
pub fn save(
    dir: &Path,
    name: &str,
    grade: &GradeEdit,
    replace: bool,
) -> Result<PresetEntry, String> {
    let name = clean_name(name)?;
    if grade.is_identity() {
        return Err("the clip has no grade to save".into());
    }
    let path = path_in(dir, &name);
    if path.exists() && !replace {
        return Err(format!(
            "there is a preset called \u{201c}{name}\u{201d} already"
        ));
    }
    let preset = GradePreset {
        version: VERSION,
        name: name.clone(),
        grade: grade.clone(),
    };
    let text = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
    write_atomically(&path, text.as_bytes())?;
    Ok(PresetEntry {
        name,
        path: path.to_string_lossy().into_owned(),
    })
}

/// Every preset in `dir`, sorted by name. Files that do not parse are
/// skipped: one broken file must not empty the list.
pub fn list(dir: &Path) -> Vec<PresetEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PresetEntry> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let preset: GradePreset =
                serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
            Some(PresetEntry {
                name: preset.name,
                path: p.to_string_lossy().into_owned(),
            })
        })
        .collect();
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

/// The preset called `name`.
pub fn load(dir: &Path, name: &str) -> Result<GradePreset, String> {
    let path = path_in(dir, name.trim());
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("there is no preset called \u{201c}{}\u{201d}", name.trim()))?;
    let preset: GradePreset = serde_json::from_str(&text)
        .map_err(|e| format!("the preset \u{201c}{}\u{201d} is damaged: {e}", name.trim()))?;
    if preset.version > VERSION {
        return Err(format!(
            "the preset \u{201c}{}\u{201d} was saved by a newer chukcut",
            preset.name
        ));
    }
    Ok(preset)
}

pub fn delete(dir: &Path, name: &str) -> Result<(), String> {
    let path = path_in(dir, name.trim());
    std::fs::remove_file(&path)
        .map_err(|_| format!("there is no preset called \u{201c}{}\u{201d}", name.trim()))
}

/// The next free "Preset N" — the name the panel suggests.
pub fn next_name(dir: &Path) -> String {
    (1..)
        .map(|n| format!("Preset {n}"))
        .find(|name| !path_in(dir, name).exists())
        .expect("an unbounded range finds a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chukcut-presets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_grade_round_trips_through_a_preset() {
        let d = dir("round-trip");
        let mut grade = GradeEdit::identity();
        grade.contrast = 1.2;
        grade.grade.exposure = 0.5;
        grade.grade.curves.red = vec![[0.0, 0.1], [1.0, 0.9]];
        grade.grade.vignette.amount = 0.3;
        let entry = save(&d, "Warm / Punchy", &grade, false).unwrap();
        assert_eq!(entry.name, "Warm / Punchy");
        assert!(entry.path.ends_with("Warm - Punchy.json"), "{}", entry.path);
        assert_eq!(load(&d, "Warm / Punchy").unwrap().grade, grade);
        assert_eq!(list(&d), vec![entry]);

        // A clash is refused unless asked to replace.
        let error = save(&d, "Warm / Punchy", &grade, false).unwrap_err();
        assert!(error.contains("already"), "{error}");
        grade.contrast = 1.4;
        save(&d, "Warm / Punchy", &grade, true).unwrap();
        assert_eq!(load(&d, "Warm / Punchy").unwrap().grade.contrast, 1.4);

        assert_eq!(next_name(&d), "Preset 1");
        delete(&d, "Warm / Punchy").unwrap();
        assert!(list(&d).is_empty());
        assert!(load(&d, "Warm / Punchy").unwrap_err().contains("no preset"));
    }

    #[test]
    fn empty_names_ungraded_clips_and_damaged_files_are_refused() {
        let d = dir("refused");
        let mut grade = GradeEdit::identity();
        assert!(save(&d, "x", &grade, false)
            .unwrap_err()
            .contains("no grade"));
        grade.saturation = 0.5;
        assert!(save(&d, "  ", &grade, false).unwrap_err().contains("name"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("broken.json"), "{ not json").unwrap();
        assert!(list(&d).is_empty());
        assert!(load(&d, "broken").unwrap_err().contains("damaged"));
    }
}
