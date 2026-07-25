//! IPC commands for settings, recent projects and cache management.

use super::paths;
use super::settings::{RecentProjects, Settings};

#[tauri::command]
pub fn workspace_settings_get() -> Settings {
    Settings::load()
}

#[tauri::command]
pub fn workspace_settings_set(settings: Settings) -> Result<(), String> {
    settings.save()
}

#[tauri::command]
pub fn workspace_recent_list() -> Vec<super::settings::RecentProject> {
    let mut recent = RecentProjects::load();
    recent.prune_missing();
    recent.entries
}

#[tauri::command]
pub fn workspace_recent_record(path: String, name: String, now: i64) -> Result<(), String> {
    let mut recent = RecentProjects::load();
    recent.record(path, name, now);
    recent.save()
}

#[tauri::command]
pub fn workspace_cache_size() -> u64 {
    paths::cache_size()
}

#[tauri::command]
pub fn workspace_cache_clear() -> Result<(), String> {
    paths::clear_cache().map_err(|e| format!("could not clear the cache: {e}"))
}
