//! What the export remembers between sessions: the user's own presets, and
//! the settings of the last export — for each project and overall.
//!
//! Two small JSON files in the config directory, read on demand and written
//! whole through a temporary file, so a crash mid-write leaves the old file.
//! Losing either costs the user a few clicks, never a project, so a file that
//! does not parse is treated as empty rather than as an error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::modules::project::document::Project;

use super::job::{resolve_settings, ExportOverrides, ExportRequest};
use super::presets::{ExportPreset, PresetCategory, Quality};

const PRESETS_FILE: &str = "export-presets.json";
const MEMORY_FILE: &str = "export-memory.json";

/// How many projects keep their own remembered settings. The oldest are
/// forgotten first; a project not exported for this many exports gets the
/// overall last settings instead, which is what a new project gets anyway.
const PROJECTS_KEPT: usize = 200;

/// The settings of one export, as the dialog needs them to open the same
/// way next time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportMemory {
    /// The preset the user picked, if the settings still match it.
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default)]
    pub overrides: ExportOverrides,
    #[serde(default = "yes")]
    pub include_audio: bool,
    /// Use a GPU encoder when one works for the codec.
    #[serde(default = "yes")]
    pub use_hardware: bool,
    /// The folder the file went to.
    #[serde(default)]
    pub directory: Option<String>,
}

fn yes() -> bool {
    true
}

impl Default for ExportMemory {
    fn default() -> Self {
        Self {
            preset_id: None,
            overrides: ExportOverrides::default(),
            include_audio: true,
            use_hardware: true,
            directory: None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MemoryFile {
    #[serde(default)]
    last: Option<ExportMemory>,
    /// By project id.
    #[serde(default)]
    projects: BTreeMap<String, Remembered>,
    /// Incremented on every write; orders `projects` for forgetting.
    #[serde(default)]
    counter: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Remembered {
    used: u64,
    memory: ExportMemory,
}

fn config_dir() -> PathBuf {
    crate::modules::workspace::paths::config_root()
}

// ---------------------------------------------------------------------------
// User presets
// ---------------------------------------------------------------------------

/// The user's own presets, in the order they were saved.
pub fn user_presets() -> Vec<ExportPreset> {
    read_presets(&config_dir())
}

/// Save `preset` as a user preset and return it as stored. A preset with
/// the same id or the same label is replaced: saving "My reel" twice
/// updates it rather than making two.
pub fn save_user_preset(preset: ExportPreset) -> Result<ExportPreset, String> {
    save_preset_in(&config_dir(), preset)
}

/// Delete the user preset `id`. `false` when there was none.
pub fn remove_user_preset(id: &str) -> Result<bool, String> {
    remove_preset_in(&config_dir(), id)
}

/// A user preset that reproduces `request` on `project`: the same codec,
/// quality, frame rate, sound and loudness, and the same resolution class —
/// the shorter side of the size it resolves to, so the preset follows the
/// shape of whatever canvas it is used on later.
pub fn preset_from_request(
    project: &Project,
    request: &ExportRequest,
    label: &str,
) -> Result<ExportPreset, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("a preset needs a name".into());
    }
    let settings = resolve_settings(project, request).map_err(|e| e.to_string())?;
    let mut preset = settings.preset.clone();
    preset.id = format!("user_{}", slug(label));
    preset.label = label.to_string();
    preset.loudness_target = settings.loudness_target;
    preset.user = true;
    preset.follow_canvas = !settings.audio_only;
    preset.follow_project_fps = false;
    preset.platform_aspect = None;
    preset.max_seconds = None;
    if preset.category != PresetCategory::Audio && preset.category != PresetCategory::Gif {
        preset.category = PresetCategory::Custom;
    }
    if settings.audio.is_none() {
        preset.audio_codec = super::presets::AudioCodec::None;
    }
    preset.description = describe(&preset);
    Ok(preset)
}

/// "1080p · 30 fps · H.264 CRF 20 · AAC 192 kbit/s · −14 LUFS".
pub fn describe(preset: &ExportPreset) -> String {
    use super::presets::AudioCodec;
    let mut parts = Vec::new();
    if !preset.container.is_audio_only() {
        parts.push(format!("{}p", preset.width.min(preset.height)));
        parts.push(format!("{} fps", preset.fps));
        parts.push(if preset.video_codec.uses_quality() {
            match preset.quality {
                Quality::Crf(crf) => format!("{} CRF {crf}", preset.video_codec.label()),
                Quality::Bitrate(bits) => format!(
                    "{} {:.1} Mbit/s",
                    preset.video_codec.label(),
                    bits as f64 / 1e6
                ),
            }
        } else {
            preset.video_codec.label().to_string()
        });
    }
    match preset.audio_codec {
        AudioCodec::None => {}
        AudioCodec::Pcm => parts.push(format!("PCM {} kHz", preset.sample_rate / 1000)),
        codec => parts.push(format!(
            "{} {} kbit/s",
            codec.label(),
            preset.audio_bitrate / 1000
        )),
    }
    if let Some(lufs) = preset.loudness_target {
        parts.push(format!("{lufs:.0} LUFS"));
    }
    parts.push(format!(".{}", preset.container.extension()));
    parts.join(" · ")
}

fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "preset".into()
    } else {
        out
    }
}

fn read_presets(dir: &Path) -> Vec<ExportPreset> {
    std::fs::read_to_string(dir.join(PRESETS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<ExportPreset>>(&text).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|mut preset| {
            preset.user = true;
            preset
        })
        .collect()
}

fn save_preset_in(dir: &Path, mut preset: ExportPreset) -> Result<ExportPreset, String> {
    preset.user = true;
    if ExportPreset::by_id(&preset.id).is_some() {
        return Err(format!(
            "{} is the id of a built-in preset; choose another name",
            preset.id
        ));
    }
    let mut check = preset.clone();
    // A preset that follows the canvas has a nominal size; one with a zero
    // size takes the canvas's own. Either way the check needs a real size.
    if check.width == 0 || check.height == 0 {
        check.width = 1920;
        check.height = 1080;
    }
    check.validate()?;
    let mut presets = read_presets(dir);
    presets.retain(|p| p.id != preset.id && !p.label.eq_ignore_ascii_case(&preset.label));
    presets.push(preset.clone());
    write_json(&dir.join(PRESETS_FILE), &presets)?;
    Ok(preset)
}

fn remove_preset_in(dir: &Path, id: &str) -> Result<bool, String> {
    let mut presets = read_presets(dir);
    let before = presets.len();
    presets.retain(|p| p.id != id);
    if presets.len() == before {
        return Ok(false);
    }
    write_json(&dir.join(PRESETS_FILE), &presets)?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Remembered settings
// ---------------------------------------------------------------------------

/// Keep `memory` as the settings the next export opens with: for this
/// project, and for any project that has none of its own.
pub fn remember(project_id: Option<&str>, memory: &ExportMemory) -> Result<(), String> {
    remember_in(&config_dir(), project_id, memory)
}

/// The settings to open the export with: this project's last ones, else the
/// last ones of any project, else nothing.
pub fn recall(project_id: Option<&str>) -> Option<ExportMemory> {
    recall_in(&config_dir(), project_id)
}

fn read_memory(dir: &Path) -> MemoryFile {
    std::fs::read_to_string(dir.join(MEMORY_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn remember_in(dir: &Path, project_id: Option<&str>, memory: &ExportMemory) -> Result<(), String> {
    let mut file = read_memory(dir);
    file.counter += 1;
    file.last = Some(memory.clone());
    if let Some(id) = project_id.filter(|id| !id.is_empty()) {
        file.projects.insert(
            id.to_string(),
            Remembered {
                used: file.counter,
                memory: memory.clone(),
            },
        );
        while file.projects.len() > PROJECTS_KEPT {
            let oldest = file
                .projects
                .iter()
                .min_by_key(|(_, r)| r.used)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => file.projects.remove(&id),
                None => break,
            };
        }
    }
    write_json(&dir.join(MEMORY_FILE), &file)
}

fn recall_in(dir: &Path, project_id: Option<&str>) -> Option<ExportMemory> {
    let mut file = read_memory(dir);
    project_id
        .and_then(|id| file.projects.remove(id))
        .map(|r| r.memory)
        .or(file.last)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, path))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::export::presets::{AudioCodec, Container, VideoCodec};

    fn dir(name: &str) -> PathBuf {
        crate::modules::export::testing::scratch(&format!("store-{name}"))
    }

    fn project(width: u32, height: u32) -> Project {
        crate::modules::export::testing::project(width, height, 2_000_000)
    }

    fn request() -> ExportRequest {
        ExportRequest {
            output_path: "/x/out.mp4".into(),
            preset_id: Some("tiktok".into()),
            overrides: Some(ExportOverrides {
                quality: Some(Quality::Crf(18)),
                ..Default::default()
            }),
            hardware: None,
            include_audio: true,
            range: None,
        }
    }

    #[test]
    fn a_saved_preset_reproduces_the_settings_and_follows_the_next_canvas() {
        let dir = dir("save");
        let vertical = project(1080, 1920);
        let preset = preset_from_request(&vertical, &request(), "My reel").unwrap();
        assert_eq!(preset.id, "user_my_reel");
        assert_eq!((preset.width, preset.height), (1080, 1920));
        assert_eq!(preset.quality, Quality::Crf(18));
        assert_eq!(preset.loudness_target, Some(-14.0));
        assert!(
            preset.description.contains("CRF 18"),
            "{}",
            preset.description
        );
        save_preset_in(&dir, preset.clone()).unwrap();

        let stored = read_presets(&dir);
        assert_eq!(stored, vec![preset.clone()]);
        let wide = stored[0].for_project(&project(1920, 1080));
        assert_eq!((wide.width, wide.height), (1920, 1080));

        // Saving the same name again replaces it.
        let mut again = preset.clone();
        again.quality = Quality::Crf(23);
        save_preset_in(&dir, again).unwrap();
        let stored = read_presets(&dir);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].quality, Quality::Crf(23));

        assert!(remove_preset_in(&dir, "user_my_reel").unwrap());
        assert!(!remove_preset_in(&dir, "user_my_reel").unwrap());
        assert!(read_presets(&dir).is_empty());
    }

    #[test]
    fn a_preset_cannot_take_a_built_in_id_or_be_invalid() {
        let dir = dir("refuse");
        let mut preset = ExportPreset::by_id("tiktok").unwrap();
        assert!(save_preset_in(&dir, preset.clone()).is_err());
        preset.id = "user_broken".into();
        preset.video_codec = VideoCodec::Vp9;
        preset.container = Container::Mp4;
        assert!(save_preset_in(&dir, preset).is_err());
        assert!(preset_from_request(&project(1080, 1920), &request(), "  ").is_err());
    }

    #[test]
    fn an_audio_only_preset_stays_audio_only() {
        let mut req = request();
        req.preset_id = Some("audio_mp3".into());
        req.overrides = None;
        let preset = preset_from_request(&project(1080, 1920), &req, "Podcast").unwrap();
        assert_eq!(preset.container, Container::Mp3);
        assert_eq!(preset.audio_codec, AudioCodec::Mp3);
        assert!(!preset.follow_canvas);
        assert!(
            !preset.description.contains("fps"),
            "{}",
            preset.description
        );
    }

    #[test]
    fn settings_are_remembered_per_project_and_overall() {
        let dir = dir("memory");
        assert_eq!(recall_in(&dir, Some("a")), None);

        let a = ExportMemory {
            preset_id: Some("tiktok".into()),
            ..Default::default()
        };
        let b = ExportMemory {
            preset_id: Some("youtube_4k".into()),
            overrides: ExportOverrides {
                loudness_target: Some(-16.0),
                ..Default::default()
            },
            ..Default::default()
        };
        remember_in(&dir, Some("a"), &a).unwrap();
        remember_in(&dir, Some("b"), &b).unwrap();

        assert_eq!(recall_in(&dir, Some("a")), Some(a));
        assert_eq!(recall_in(&dir, Some("b")), Some(b.clone()));
        // A project with no export of its own opens with the last one.
        assert_eq!(recall_in(&dir, Some("new")), Some(b.clone()));
        assert_eq!(recall_in(&dir, None), Some(b));
    }

    #[test]
    fn the_oldest_projects_are_forgotten_first() {
        let dir = dir("forget");
        for i in 0..PROJECTS_KEPT + 3 {
            let memory = ExportMemory {
                directory: Some(format!("/d{i}")),
                ..Default::default()
            };
            remember_in(&dir, Some(&format!("p{i}")), &memory).unwrap();
        }
        let file = read_memory(&dir);
        assert_eq!(file.projects.len(), PROJECTS_KEPT);
        assert!(!file.projects.contains_key("p0"));
        assert!(file
            .projects
            .contains_key(&format!("p{}", PROJECTS_KEPT + 2)));
    }

    #[test]
    fn a_damaged_file_reads_as_empty() {
        let dir = dir("damaged");
        std::fs::write(dir.join(MEMORY_FILE), "{not json").unwrap();
        std::fs::write(dir.join(PRESETS_FILE), "[1,2").unwrap();
        assert_eq!(recall_in(&dir, Some("a")), None);
        assert!(read_presets(&dir).is_empty());
    }
}
