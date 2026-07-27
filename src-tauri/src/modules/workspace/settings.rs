//! Persisted application settings and the recent-projects list.
//!
//! Deliberately small. Anything that belongs to a project belongs in the
//! project file; this is only for preferences that outlive any one document.
//!
//! Loading is forgiving: a settings file that fails to parse is replaced with
//! defaults rather than blocking startup. Nobody should be locked out of their
//! editor by a stray comma.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Long-edge cap for preview rendering. **0 means automatic**: render at
    /// the player panel's own size in device pixels, never above the canvas.
    /// A non-zero value is a deliberate override for weak machines.
    ///
    /// The default is 0, and it used to be 960 — which was harmless for as
    /// long as the setting was *inert* (`session.ts` passed `null`, so the
    /// dialog wrote a value nothing read). The day the settings were wired,
    /// that never-chosen 960 silently became a hard cap on every preview, and
    /// 1080p playback came out soft on the very build whose pipeline could
    /// finally afford full size. Third incarnation of this exact bug class:
    /// canvas adoption once capped the long edge at 1080, the preview once
    /// capped again at 720. The lesson each time: a resolution cap the user
    /// did not ask for reads as "broken quality", never as "fast".
    pub preview_max_edge: u32,
    /// Render the preview at full canvas resolution, accepting the frame rate
    /// cost. For users on fast machines who want to judge fine detail.
    pub preview_full_quality: bool,
    /// JPEG quality for preview frames, 1..100.
    pub preview_quality: u8,
    /// Snap clips to edges, playhead and second boundaries while dragging.
    pub snapping: bool,
    /// Default canvas for new projects.
    pub default_canvas: (u32, u32),
    pub default_fps: f64,
    /// Cap on cache size in bytes; the cache is trimmed past this. 0 = no cap.
    pub cache_limit: u64,
    /// Schema version of the file on disk, for one-time migrations. Absent in
    /// files written before it existed, which serde reads as 0.
    #[serde(default)]
    pub settings_version: u32,
}

/// The current schema version. Bump it when a migration is added below.
const SETTINGS_VERSION: u32 = 1;

impl Default for Settings {
    fn default() -> Self {
        Self {
            preview_max_edge: 0,
            preview_full_quality: false,
            preview_quality: 80,
            snapping: true,
            // Vertical by default: this editor exists to make short-form video.
            default_canvas: (1080, 1920),
            default_fps: 30.0,
            cache_limit: 8 * 1024 * 1024 * 1024,
            settings_version: SETTINGS_VERSION,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = paths::settings_file();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&raw) {
            Ok(settings) => settings.migrated(),
            Err(e) => {
                tracing::warn!("settings file is unreadable, using defaults: {e}");
                Self::default()
            }
        }
    }

    /// One-time repairs to values an older build wrote.
    ///
    /// Version 0 → 1: a stored `preview_max_edge` of 960 becomes 0 (automatic).
    /// For the whole life of version 0 that setting was inert — the dialog
    /// wrote a value nothing read — so a stored 960 is the old default, not a
    /// choice; nobody could have chosen a knob that did nothing. The moment the
    /// setting started working, every existing file's 960 turned into a silent
    /// cap that made 1080p playback soft. A *deliberate* 960 set from now on
    /// lives in a version-1 file and is left alone.
    fn migrated(mut self) -> Self {
        if self.settings_version < 1 {
            if self.preview_max_edge == 960 {
                tracing::info!(
                    "settings migration: preview_max_edge 960 was the inert-era default, \
                     now 0 (automatic panel size)"
                );
                self.preview_max_edge = 0;
            }
            self.settings_version = 1;
        }
        self
    }

    pub fn save(&self) -> Result<(), String> {
        let path = paths::settings_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())
    }
}

/// How many recent projects are remembered.
const RECENT_LIMIT: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: String,
    pub name: String,
    /// Unix millis of the last time it was opened.
    pub opened_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecentProjects {
    #[serde(default)]
    pub entries: Vec<RecentProject>,
}

impl RecentProjects {
    pub fn load() -> Self {
        std::fs::read_to_string(paths::recent_projects_file())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = paths::recent_projects_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())
    }

    /// Record an open. An already-listed project moves to the front rather
    /// than appearing twice.
    pub fn record(&mut self, path: impl Into<String>, name: impl Into<String>, now: i64) {
        let path = path.into();
        self.entries.retain(|e| e.path != path);
        self.entries.insert(
            0,
            RecentProject {
                path,
                name: name.into(),
                opened_at: now,
            },
        );
        self.entries.truncate(RECENT_LIMIT);
    }

    /// Drop entries whose file no longer exists. Called before showing the
    /// list, so the start screen never offers a project that cannot open.
    pub fn prune_missing(&mut self) {
        self.entries
            .retain(|e| PathBuf::from(&e.path).exists());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The migration exists because the user hit this: the day
    /// `preview_max_edge` started working, every settings file on disk carried
    /// the inert-era default of 960 and 1080p playback silently went soft.
    #[test]
    fn a_version_zero_file_with_the_inert_default_becomes_automatic() {
        let stored: Settings = serde_json::from_str(r#"{"preview_max_edge": 960}"#).unwrap();
        assert_eq!(stored.settings_version, 0, "absent version must read as 0");
        let migrated = stored.migrated();
        assert_eq!(migrated.preview_max_edge, 0, "the never-chosen 960 becomes auto");
        assert_eq!(migrated.settings_version, SETTINGS_VERSION);
    }

    #[test]
    fn a_deliberate_cap_survives_migration() {
        // 720 in a version-0 file cannot be the old default, so it was typed by
        // hand into a dialog — inert or not, it is the user's number.
        let stored: Settings = serde_json::from_str(r#"{"preview_max_edge": 720}"#).unwrap();
        assert_eq!(stored.migrated().preview_max_edge, 720);

        // And a 960 in a *version-1* file was chosen while the setting worked.
        let chosen: Settings =
            serde_json::from_str(r#"{"preview_max_edge": 960, "settings_version": 1}"#).unwrap();
        assert_eq!(chosen.migrated().preview_max_edge, 960);
    }

    #[test]
    fn recording_moves_an_existing_entry_to_the_front() {
        let mut recent = RecentProjects::default();
        recent.record("/a.chukcut", "A", 1);
        recent.record("/b.chukcut", "B", 2);
        recent.record("/a.chukcut", "A", 3);

        assert_eq!(recent.entries.len(), 2);
        assert_eq!(recent.entries[0].path, "/a.chukcut");
        assert_eq!(recent.entries[0].opened_at, 3);
    }

    #[test]
    fn the_recent_list_is_capped() {
        let mut recent = RecentProjects::default();
        for i in 0..(RECENT_LIMIT + 10) {
            recent.record(format!("/p{i}.chukcut"), format!("P{i}"), i as i64);
        }
        assert_eq!(recent.entries.len(), RECENT_LIMIT);
        // Newest first.
        assert_eq!(
            recent.entries[0].path,
            format!("/p{}.chukcut", RECENT_LIMIT + 9)
        );
    }

    #[test]
    fn unparseable_settings_fall_back_to_defaults() {
        let parsed: Result<Settings, _> = serde_json::from_str("{ not json ");
        assert!(parsed.is_err());
        let defaults = Settings::default();
        assert_eq!(defaults.default_canvas, (1080, 1920));
    }
}
