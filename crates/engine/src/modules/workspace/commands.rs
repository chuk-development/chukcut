//! Commands for settings, recent projects and cache management.

use super::settings::{RecentProjects, Settings};
use super::{logging, paths};

pub fn workspace_settings_get() -> Settings {
    Settings::load()
}
pub fn workspace_settings_set(settings: Settings) -> Result<(), String> {
    settings.save()
}
pub fn workspace_recent_list() -> Vec<super::settings::RecentProject> {
    let mut recent = RecentProjects::load();
    recent.prune_missing();
    recent.entries
}
pub fn workspace_recent_record(path: String, name: String, now: i64) -> Result<(), String> {
    let mut recent = RecentProjects::load();
    recent.record(path, name, now);
    recent.save()
}

/// File → Recent Projects → Clear List.
///
/// Forgets the list, not the projects: nothing on disk but `recent.json` is
/// touched.
pub fn workspace_recent_clear() -> Result<(), String> {
    RecentProjects::default().save()
}
pub fn workspace_cache_size() -> u64 {
    paths::cache_size()
}

/// Where this run is writing its log, and where the logs live in general.
///
/// The directory is always answered and the file only when one was opened, so
/// the UI can still show a person where to look on the machine where opening it
/// failed. `file` is what a "show me" button reveals: revealing the file
/// selects it in the file manager, which is strictly more useful than opening a
/// directory of seven.
pub fn workspace_log_path() -> LogLocation {
    LogLocation {
        directory: paths::logs_dir().display().to_string(),
        file: logging::log_file().map(|path| path.display().to_string()),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LogLocation {
    pub directory: String,
    /// `None` when file logging could not start — a read-only home directory,
    /// most likely. The app runs anyway; see `logging::init`.
    pub file: Option<String>,
}
pub fn workspace_cache_clear() -> Result<(), String> {
    paths::clear_cache().map_err(|e| format!("could not clear the cache: {e}"))
}

/// What this machine can actually do, established by doing it.
///
/// Not a `Result`: a machine with no GPU still gets a report, and "no device
/// could be opened" is exactly the answer someone opening this panel wants.
/// See `hardware.rs` for why every field is measured rather than declared.
pub async fn workspace_hardware() -> super::HardwareReport {
    // Probing opens devices and pushes frames through them — on a cold start
    // roughly a hundred milliseconds per encoder. That is far past what a
    // command may spend on the main thread, so it goes to a blocking worker.
    // Both probes cache for the process, so this is only slow once.
    crate::shell::spawn_blocking(super::hardware::report)
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
