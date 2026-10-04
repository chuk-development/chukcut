//! Commands for settings, recent projects and cache management.

use super::settings::{RecentProjects, Settings};
use super::{logging, paths, trim};
use crate::modules::proxy::commands as proxy_commands;
use crate::modules::proxy::ProxyPolicy;

pub fn workspace_settings_get() -> Settings {
    Settings::load()
}

/// Persist the settings and put the ones the engine acts on into effect.
pub fn workspace_settings_set(settings: Settings) -> Result<(), String> {
    settings.save()?;
    workspace_settings_apply(&settings);
    Ok(())
}

/// Put the settings the engine acts on into effect, without writing them.
///
/// `crate::init` calls this with the stored settings at startup, and
/// [`workspace_settings_set`] after every change, so a shell never has to.
/// Cheap and non-blocking: a trim, when one is due, runs on its own thread.
///
/// - **Proxy policy**: whether imports and opened projects queue proxies, and
///   whether the preview decodes them. Turning proxies on considers the open
///   project's media at once rather than waiting for the next import.
/// - **Cache limit**: trims when the limit changes. The startup trim waits for
///   the first project to be opened, so it knows what not to delete; see
///   `workspace::trim`.
/// - **Video decoding**: the decode path decoders opened from now on take
///   (`media::provider::set_decode_preference`); `CHUKCUT_DECODE` wins.
/// - **AI runtime**: the ONNX Runtime pack the ML worker loads; a change
///   stops a running worker so the next request starts one on it
///   (`ml::set_runtime_setting`); `CHUKCUT_ML_RUNTIME` wins.
pub fn workspace_settings_apply(settings: &Settings) {
    crate::modules::media::provider::set_decode_preference(settings.decode.acceleration());
    crate::modules::ml::set_runtime_setting(settings.ml_runtime.clone());

    let was = proxy_commands::proxy_policy();
    proxy_commands::proxy_set_policy(settings.proxy_policy);
    if was == ProxyPolicy::Off && settings.proxy_policy != ProxyPolicy::Off {
        proxy_commands::proxy_request_media(
            trim::in_use()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        );
    }

    let previous = trim::set_limit(settings.cache_limit);
    if previous != settings.cache_limit {
        trim::trim_in_background();
    }

    // AI acceleration: a worker running in the other mode stops, and the
    // next request starts one in this mode. `CHUKCUT_ML_ACCELERATION`, when
    // set, overrides the setting for the whole process.
    if std::env::var_os("CHUKCUT_ML_ACCELERATION").is_none() {
        crate::modules::ml::worker::set_acceleration(if settings.ml_fast {
            crate::modules::ml::commands::Acceleration::Fast
        } else {
            crate::modules::ml::commands::Acceleration::Standard
        });
    }
}

/// Delete least-recently-used cache files until the cache fits in
/// `limit_bytes` (0: no limit, nothing is deleted). The open project's
/// thumbnails, waveforms and proxies are kept; see `workspace::trim`.
///
/// Walks the cache directory: call it off the UI thread.
pub fn workspace_trim_cache(limit_bytes: u64) -> trim::TrimReport {
    trim::trim_cache(limit_bytes)
}

/// Tell the engine which media the open document uses, so trimming the cache
/// keeps their derived files. The project commands call this on open, import
/// and close; an empty list means nothing is open.
pub fn workspace_cache_in_use(media: Vec<String>) {
    trim::set_in_use(media.into_iter().map(std::path::PathBuf::from).collect());
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
