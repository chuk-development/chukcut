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
    /// Long-edge cap for preview rendering. See the preview pipeline doc for
    /// why the preview is proxy-resolution by default.
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            preview_max_edge: 960,
            preview_full_quality: false,
            preview_quality: 80,
            snapping: true,
            // Vertical by default: this editor exists to make short-form video.
            default_canvas: (1080, 1920),
            default_fps: 30.0,
            cache_limit: 8 * 1024 * 1024 * 1024,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = paths::settings_file();
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str(&raw) {
            Ok(settings) => settings,
            Err(e) => {
                tracing::warn!("settings file is unreadable, using defaults: {e}");
                Self::default()
            }
        }
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
