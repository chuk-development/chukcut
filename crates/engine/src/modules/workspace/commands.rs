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

/// One row of the start screen's project list.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentEntry {
    pub path: String,
    /// The project's own name when the file could be read, else the name it
    /// had when it was last opened.
    pub name: String,
    /// Unix millis of the last open.
    pub opened_at: i64,
    /// The file is gone. Shown, not hidden: a project on an unmounted drive
    /// is still the user's, and silently dropping it reads as data loss.
    pub missing: bool,
    pub duration: Option<crate::modules::project::Micros>,
    /// A picture for the card, when the project has one.
    pub poster: Option<Poster>,
}

/// The first visual clip of a project, to draw its card with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Poster {
    /// The media file the clip shows.
    pub path: String,
    /// An image file, usable as it is; otherwise a video to take a frame of.
    pub still: bool,
}

/// The recent list for the start screen, unpruned and with what each card
/// needs. Reads each project file, so call it off the UI thread.
pub fn workspace_recent_entries() -> Vec<RecentEntry> {
    RecentProjects::load()
        .entries
        .into_iter()
        .map(|entry| {
            let document = std::fs::read_to_string(&entry.path)
                .ok()
                .and_then(|raw| crate::modules::project::migrate::load(&raw).ok())
                .map(|loaded| loaded.project);
            RecentEntry {
                missing: !std::path::Path::new(&entry.path).exists(),
                name: document
                    .as_ref()
                    .map(|p| p.name.clone())
                    .unwrap_or(entry.name),
                path: entry.path,
                opened_at: entry.opened_at,
                duration: document.as_ref().map(|p| p.duration()),
                poster: document.as_ref().and_then(poster_of),
            }
        })
        .collect()
}

/// The earliest clip on a video lane whose material is a picture.
pub fn poster_of(project: &crate::modules::project::Project) -> Option<Poster> {
    use crate::modules::project::TrackKind;
    let pool = &project.materials;
    project
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .flat_map(|track| track.segments.iter())
        .filter_map(|segment| {
            let id = &segment.material_id;
            let poster = if let Some(video) = pool.videos.iter().find(|m| &m.id == id) {
                Poster {
                    path: video.path.clone(),
                    still: false,
                }
            } else {
                let image = pool.images.iter().find(|m| &m.id == id)?;
                Poster {
                    path: image.path.clone(),
                    still: true,
                }
            };
            Some((segment.target_range.start, poster))
        })
        .min_by_key(|(start, _)| *start)
        .map(|(_, poster)| poster)
}

/// Take one project off the recent list. The file is not touched.
pub fn workspace_recent_forget(path: String) -> Result<(), String> {
    let mut recent = RecentProjects::load();
    recent.entries.retain(|entry| entry.path != path);
    recent.save()
}
