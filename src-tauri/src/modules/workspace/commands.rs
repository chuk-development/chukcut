//! IPC commands for settings, recent projects and cache management.

use super::menu::{self, MenuState};
use super::settings::{RecentProjects, Settings};
use super::{logging, paths};

/// The menu bar as it should now be drawn, for the document the webview
/// describes.
///
/// The frontend's stores are the only authority on the state, so this is pushed
/// rather than polled: every field of `MenuState` is read straight out of
/// Zustand, and Rust keeps no copy of it. What comes back is the whole bar —
/// titles, labels, accelerators, enabled flags, and the reason for any item
/// that can never be enabled — so that the table in `menu.rs` is the only place
/// either half has to look. Which items a state lights up is decided in one pure
/// function, `menu::enablement`.
///
/// Capabilities are re-probed on every call rather than cached: a log directory
/// appears the first time something is written, and an item that is grey until
/// then and quietly becomes clickable is the honest behaviour.
#[tauri::command]
pub fn workspace_menu_describe(state: MenuState) -> Vec<menu::SectionView> {
    // The recent list rides along un-pruned: a project whose file has gone is
    // drawn greyed with the reason rather than silently missing. Loaded fresh
    // on every call for the same reason capabilities are re-probed — the file
    // can reappear, and the honest bar notices.
    menu::describe(
        &state,
        &menu::Capabilities::probe(),
        &menu::recent_entries(),
    )
}

/// Run one of the four items that are about the machine rather than the
/// document — quitting, the log directory, the docs, the about box.
///
/// The webview runs everything else itself; `menu::is_ours` is the split, and
/// `runMenuAction` in `workspace/lib/menu.ts` is the other side of it.
#[tauri::command]
pub fn workspace_menu_run(app: tauri::AppHandle, id: String) {
    menu::run(&app, &id);
}

/// The webview's answer to the close request raised by File → Quit or by the
/// window's close button.
///
/// `false` is the answer that matters: the window was never closed, so
/// cancelling is simply not closing it. This is why the close is prevented
/// first and confirmed second — once a window has begun closing there is
/// nothing left to cancel.
#[tauri::command]
pub fn workspace_close_answer(window: tauri::Window, confirmed: bool) {
    if confirmed {
        if let Err(error) = window.destroy() {
            tracing::error!(%error, "the window refused to close");
        }
    } else {
        menu::end_close();
    }
}

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

/// File → Recent Projects → Clear List.
///
/// Forgets the list, not the projects: nothing on disk but `recent.json` is
/// touched.
#[tauri::command]
pub fn workspace_recent_clear() -> Result<(), String> {
    RecentProjects::default().save()
}

#[tauri::command]
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
#[tauri::command]
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
