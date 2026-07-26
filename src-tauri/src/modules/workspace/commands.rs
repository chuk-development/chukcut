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

/// What this machine can actually do, established by doing it.
///
/// Not a `Result`: a machine with no GPU still gets a report, and "no device
/// could be opened" is exactly the answer someone opening this panel wants.
/// See `hardware.rs` for why every field is measured rather than declared.
#[tauri::command]
pub async fn workspace_hardware() -> super::HardwareReport {
    // Probing opens devices and pushes frames through them — on a cold start
    // roughly a hundred milliseconds per encoder. That is far past what a
    // command may spend on the main thread, so it goes to a blocking worker.
    // Both probes cache for the process, so this is only slow once.
    tauri::async_runtime::spawn_blocking(super::hardware::report)
        .await
        .unwrap_or_else(|error| {
            tracing::error!(%error, "hardware probe panicked");
            super::HardwareReport {
                gpu: None,
                backend: None,
                device_type: None,
                can_import_dmabuf: false,
                encoders: Vec::new(),
                decoders: Vec::new(),
                partial: true,
            }
        })
}
