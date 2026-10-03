//! Commands for creating, opening and saving projects.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::document::{
    new_id, AudioMaterial, CanvasConfig, ImageMaterial, MaterialKind, Micros, Project, Track,
    TrackKind, ValidationIssue, VideoMaterial,
};
use crate::state::AppState;

/// Saved as pretty JSON: projects are small relative to media, and a
/// diffable, hand-inspectable format is worth far more during development
/// than the bytes it costs.
fn write_project(path: &PathBuf, project: &Project) -> Result<(), String> {
    let json = serde_json::to_string_pretty(project).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Write to a sibling temp file and rename, so a crash mid-write cannot
    // leave a truncated project behind.
    let tmp = path.with_extension("chukcut.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn project_new(
    state: &Arc<AppState>,
    name: String,
    width: u32,
    height: u32,
    fps: f64,
) -> Result<Project, String> {
    let canvas = CanvasConfig {
        width,
        height,
        background: [0.0, 0.0, 0.0, 1.0],
    };
    let mut project = Project::new(name, canvas, fps);

    // A brand new project always has one video and one audio lane; an editor
    // that opens with nothing to drop onto is hostile.
    project.tracks.push(Track::new(TrackKind::Video, "Video 1"));
    project.tracks.push(Track::new(TrackKind::Audio, "Audio 1"));

    *state.project.write() = Some(project.clone());
    *state.project_path.write() = None;
    state.history.write().clear();
    // A new project has never been saved anywhere, which is exactly the case
    // where losing it to a restart hurts most.
    super::autosave::schedule(&project, None);
    media_opened(&project);
    Ok(project)
}

/// A document became the open one: its media is now in use, which keeps its
/// thumbnails, waveforms and proxies out of the cache trim, and its videos are
/// considered for proxies under the user's policy. Neither blocks: the proxy
/// probing runs on a thread of its own and does nothing with proxies off.
fn media_opened(project: &Project) {
    let pool = &project.materials;
    let videos: Vec<String> = pool.videos.iter().map(|m| m.path.clone()).collect();
    crate::modules::workspace::commands::workspace_cache_in_use(media_paths(project));
    crate::modules::proxy::commands::proxy_request_media(videos);
}

fn media_paths(project: &Project) -> Vec<String> {
    let pool = &project.materials;
    pool.videos
        .iter()
        .map(|m| m.path.clone())
        .chain(pool.audios.iter().map(|m| m.path.clone()))
        .chain(pool.images.iter().map(|m| m.path.clone()))
        .collect()
}
pub fn project_open(state: &Arc<AppState>, path: String) -> Result<Project, String> {
    let raw = fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;

    // Never a bare `from_str`. `migrate::load` is what gates `schema_version`
    // — a file from a newer build is refused rather than half-understood — and
    // what repairs a file whose floats were written as `null`.
    let loaded = super::migrate::load(&raw).map_err(|e| format!("cannot open {path}: {e}"))?;
    for warning in &loaded.warnings {
        tracing::warn!(%path, "{warning}");
    }
    let project = loaded.project;

    let path = PathBuf::from(path);
    *state.project.write() = Some(project.clone());
    *state.project_path.write() = Some(path.clone());
    state.history.write().clear();
    super::autosave::schedule(&project, Some(path));
    media_opened(&project);
    Ok(project)
}
pub fn project_save(state: &Arc<AppState>, path: Option<String>) -> Result<String, String> {
    let target = match path {
        Some(p) => PathBuf::from(p),
        None => state
            .project_path
            .read()
            .clone()
            .ok_or("project has never been saved; a path is required")?,
    };

    let project = state.project.read().clone().ok_or("no project is open")?;
    write_project(&target, &project)?;
    *state.project_path.write() = Some(target.clone());
    // The working copy follows the save, so that a restart after "Save as"
    // restores the session pointing at the new file rather than the old one.
    super::autosave::schedule(&project, Some(target.clone()));
    Ok(target.to_string_lossy().to_string())
}

/// What the frontend gets back after importing a file: enough to render a
/// media-library tile and to build the segment that will reference it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportedMaterial {
    pub id: String,
    pub kind: MaterialKind,
    /// File name without the directory, for display.
    pub name: String,
    pub path: String,
    pub duration: Micros,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
}

/// Formats FFmpeg demuxes as a single-frame video stream but which are really
/// stills. Without this check every imported PNG becomes a video material with
/// a one-microsecond duration.
fn is_still_image(format: &str) -> bool {
    format.ends_with("_pipe") || format == "image2" || format == "png" || format == "jpeg"
}

/// A "video stream" that is actually text art.
///
/// FFmpeg's `tty` demuxer matches on the *extension alone* — `.txt`, `.nfo`,
/// `.asc` and friends — and presents the bytes as an `ansi` video stream, so
/// without this check a stray text file imports as a video material whose card
/// can never render a picture. The bintext family is the same trick for DOS
/// art files. Nobody edits ANSI art in a video editor; refusing is the honest
/// answer.
fn is_text_art(format: &str, video_codec: &str) -> bool {
    matches!(format, "tty" | "bin" | "xbin" | "idf" | "adf")
        || matches!(video_codec, "ansi" | "bintext" | "xbin" | "idf")
}

/// The pure half of [`project_import_media`]: decide what a probed file is and
/// add it to the pool.
///
/// Split from the command so the integration suite can drive imports against a
/// bare [`Project`] — the command itself needs a Tauri `State` and an async
/// runtime, neither of which a test wants to stand up.
pub fn import_material(
    project: &mut Project,
    path: &str,
    name: &str,
    info: &crate::modules::media::MediaInfo,
) -> Result<ImportedMaterial, String> {
    // Already imported? Hand back what is there.
    if let Some(existing) = project.materials.videos.iter().find(|m| m.path == path) {
        return Ok(ImportedMaterial {
            id: existing.id.clone(),
            kind: MaterialKind::Video,
            name: name.to_string(),
            path: path.to_string(),
            duration: existing.duration,
            width: existing.width,
            height: existing.height,
            has_audio: existing.has_audio,
        });
    }
    if let Some(existing) = project.materials.images.iter().find(|m| m.path == path) {
        return Ok(ImportedMaterial {
            id: existing.id.clone(),
            kind: MaterialKind::Image,
            name: name.to_string(),
            path: path.to_string(),
            duration: 0,
            width: existing.width,
            height: existing.height,
            has_audio: false,
        });
    }
    if let Some(existing) = project.materials.audios.iter().find(|m| m.path == path) {
        return Ok(ImportedMaterial {
            id: existing.id.clone(),
            kind: MaterialKind::Audio,
            name: name.to_string(),
            path: path.to_string(),
            duration: existing.duration,
            width: 0,
            height: 0,
            has_audio: true,
        });
    }

    // A "video stream" from the tty family is text, not a picture, and would
    // otherwise fall through to the video arm below.
    if let Some(video) = &info.video {
        if is_text_art(&info.format, &video.codec) {
            return Err(format!("{name} is a text file, not video, image or audio"));
        }
    }

    let id = new_id();

    match (&info.video, &info.audio) {
        (Some(video), _) if is_still_image(&info.format) => {
            project.materials.images.push(ImageMaterial {
                id: id.clone(),
                path: path.to_string(),
                width: video.display_width,
                height: video.display_height,
            });
            Ok(ImportedMaterial {
                id,
                kind: MaterialKind::Image,
                name: name.to_string(),
                path: path.to_string(),
                duration: 0,
                width: video.display_width,
                height: video.display_height,
                has_audio: false,
            })
        }

        (Some(video), _) => {
            // Adopt the first clip's shape.
            //
            // A new project defaults to 1080x1920 because this editor is for
            // short-form video, but importing a landscape file into a vertical
            // canvas and letterboxing it is almost never what anyone wanted —
            // and it silently costs performance, since the clip is then drawn
            // into a fraction of the frame. So the first video imported into an
            // empty timeline sets the canvas, exactly as CapCut does.
            //
            // Only while the timeline is empty: once anything has been cut, the
            // canvas is a decision the user has made and moving it under them
            // would reframe their work.
            if project.tracks.iter().all(|t| t.segments.is_empty()) {
                let (w, h) = (video.display_width.max(2), video.display_height.max(2));

                // Take the clip's *shape*, not its resolution.
                //
                // An earlier version adopted the source dimensions directly,
                // which meant importing a 670x672 download made the whole
                // project — preview and export alike — 670x672. That is not
                // what someone means by "I want 1080p": they mean the project
                // is 1080p and the clip sits in it.
                //
                // So the short edge targets 1080, which yields 1920x1080 for
                // 16:9, 1080x1920 for 9:16 and 1080x1080 for square — the three
                // shapes people actually deliver. The long edge is then capped
                // at 1920 so an extreme aspect ratio cannot produce an enormous
                // canvas.
                //
                // Upscaling a small source adds no detail, and it is still
                // right: the project resolution is a delivery decision, and a
                // low-resolution clip on a 1080p timeline is a normal thing to
                // have. The user can change it either way.
                const TARGET_SHORT_EDGE: f32 = 1080.0;
                const MAX_LONG_EDGE: f32 = 1920.0;

                let short = w.min(h) as f32;
                let long = w.max(h) as f32;
                let mut scale = TARGET_SHORT_EDGE / short;
                if long * scale > MAX_LONG_EDGE {
                    scale = MAX_LONG_EDGE / long;
                }

                // Even dimensions, because every 4:2:0 encoder requires them
                // and an odd canvas turns into an encoder error at export time.
                let width = ((w as f32 * scale).round() as u32).max(2) & !1;
                let height = ((h as f32 * scale).round() as u32).max(2) & !1;

                if (project.canvas.width, project.canvas.height) != (width, height) {
                    tracing::info!(
                        from = format_args!("{}x{}", project.canvas.width, project.canvas.height),
                        to = format_args!("{width}x{height}"),
                        source = format_args!("{}x{}", video.display_width, video.display_height),
                        "canvas adopted from the first imported clip"
                    );
                    project.canvas.width = width;
                    project.canvas.height = height;
                }

                // Match the timeline to the source frame rate too, so a 24 fps
                // film is not resampled to 30 for no reason.
                if video.fps > 0.0 && (video.fps - project.fps).abs() > 0.01 {
                    project.fps = video.fps;
                }
            }

            project.materials.videos.push(VideoMaterial {
                id: id.clone(),
                path: path.to_string(),
                width: video.width,
                height: video.height,
                duration: info.duration,
                fps: video.fps,
                has_audio: info.has_audio,
                rotation: video.rotation,
            });
            Ok(ImportedMaterial {
                id,
                kind: MaterialKind::Video,
                name: name.to_string(),
                path: path.to_string(),
                duration: info.duration,
                // Display dimensions, so the UI does not have to know about
                // rotation to lay out a thumbnail.
                width: video.display_width,
                height: video.display_height,
                has_audio: info.has_audio,
            })
        }

        (None, Some(audio)) => {
            project.materials.audios.push(AudioMaterial {
                id: id.clone(),
                path: path.to_string(),
                duration: info.duration,
                sample_rate: audio.sample_rate,
                channels: audio.channels,
            });
            Ok(ImportedMaterial {
                id,
                kind: MaterialKind::Audio,
                name: name.to_string(),
                path: path.to_string(),
                duration: info.duration,
                width: 0,
                height: 0,
                has_audio: true,
            })
        }

        // The container opened but holds nothing this editor can use — a
        // subtitle file, a font, a playlist. Naming what was looked for beats
        // "unsupported format": the user learns the file was readable and
        // simply is not media.
        (None, None) => Err(format!("{name} contains no video, image or audio stream")),
    }
}

/// Add a file to the project's material pool, probing it to fill in the
/// details.
///
/// Importing is not undoable, and deliberately so: a material with no segment
/// referencing it is inert, and putting library imports in the undo stack
/// means Ctrl+Z after a cut can silently empty the media panel.
///
/// Importing the same path twice returns the existing material rather than
/// duplicating it — that is the whole point of the pool being keyed by
/// identity.
pub async fn project_import_media(
    state: &Arc<AppState>,
    path: String,
) -> Result<ImportedMaterial, String> {
    // Probing opens the container and runs FFmpeg's stream-info pass, which is
    // milliseconds on a warm cache and noticeably longer on a large file over a
    // network mount. A synchronous command would do that on the main thread and
    // freeze the window for the duration, so it goes off-thread even though it
    // is usually quick.
    let probe_path = path.clone();
    let info = crate::shell::spawn_blocking(move || crate::modules::media::probe(&probe_path))
        .await
        .map_err(|error| format!("the import task failed: {error}"))?
        .map_err(|error| error.to_string())?;

    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    // A file the cloud module wrote has its licence record beside it.
    let origin = crate::modules::cloud::provenance::read_sidecar(std::path::Path::new(&path));

    let imported = {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let imported = import_material(project, &path, &name, &info)?;
        if let Some(origin) = origin {
            project
                .materials
                .origins
                .insert(imported.id.clone(), origin);
        }
        imported
    };

    // The material pool is document state like any other, and an import that a
    // restart forgets means relinking every clip that referenced it.
    if let Some(project) = state.project.read().clone() {
        let origin = state.project_path.read().clone();
        super::autosave::schedule(&project, origin);
        crate::modules::workspace::commands::workspace_cache_in_use(media_paths(&project));
    }
    // Only the new file: the rest of the pool was considered when it came in.
    if info.has_video {
        crate::modules::proxy::commands::proxy_request_media(vec![path]);
    }
    Ok(imported)
}

/// Whether the working copy has already had its one chance to be restored.
///
/// Restoring is a *startup* act. Once the app is running, "no project is open"
/// is a state the user chose, and quietly reopening yesterday's session under
/// them would be worse than losing it.
static WORKING_COPY_CONSIDERED: AtomicBool = AtomicBool::new(false);
pub fn project_get(state: &Arc<AppState>) -> Result<Option<Project>, String> {
    if let Some(open) = state.project.read().clone() {
        return Ok(Some(open));
    }
    // Nothing open and nothing has asked yet: this is the first call after
    // launch, and the working copy is what the last session left behind.
    if WORKING_COPY_CONSIDERED.swap(true, Ordering::SeqCst) {
        return Ok(None);
    }
    Ok(restore_working_copy(&state))
}

/// Load the autosaved document into the app, if there is one.
///
/// Failures are logged rather than returned: the app must start even when the
/// working copy is unreadable, and a user who is told "chukcut cannot start,
/// autosave.chukcut is corrupt" has been given a problem instead of an editor.
fn restore_working_copy(state: &AppState) -> Option<Project> {
    let restored = match super::autosave::read_from(&super::autosave::file()) {
        Ok(restored) => restored?,
        Err(error) => {
            tracing::warn!(%error, "the working copy could not be restored");
            return None;
        }
    };
    for warning in &restored.warnings {
        tracing::warn!("restoring the working copy: {warning}");
    }
    tracing::info!(
        name = %restored.project.name,
        tracks = restored.project.tracks.len(),
        origin = ?restored.path,
        "restored the working copy left by the last session"
    );

    let project = restored.project;
    *state.project.write() = Some(project.clone());
    *state.project_path.write() = restored.path;
    // The restored document is where the user was, not something they did.
    // Undo must not walk back into a session that is over.
    state.history.write().clear();
    media_opened(&project);
    Some(project)
}

/// What a settings change hands back: the same shape as the timeline's
/// `EditResponse`, because the frontend routes both through the one
/// `applyEditResponse` path and a second shape would be a second code path for
/// no reason.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigureResponse {
    pub project: Project,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
}

/// Change the project's name, canvas, frame rate or background — as **one
/// undoable step** on the same stack as every timeline edit.
///
/// Two things worth knowing before touching this:
///
/// - A no-op is deliberately not recorded. The dialog's Save button always
///   commits, and "Undo Project settings" reversing nothing visible would read
///   as undo being broken.
/// - Changing `fps` re-times **nothing**. Every time in the document is `i64`
///   microseconds (`docs/architecture/project-format.md`), so the rate is
///   presentation and export, never position. The dialog warns about this; the
///   command relies on it.
pub fn project_configure(
    state: &Arc<AppState>,
    config: super::configure::ProjectConfig,
) -> Result<ConfigureResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let command = super::configure::ConfigureCommand::new(project, config);
        if !command.is_noop() {
            state.history.write().apply_configure(project, command)?;
        }
    }

    // The same tail as the timeline's `respond`: the working copy follows every
    // change to the document, and the history flags ride along so the menu's
    // Undo item is right without a second round trip.
    let project = state.project.read().clone().ok_or("no project is open")?;
    let origin = state.project_path.read().clone();
    super::autosave::schedule(&project, origin);

    let history = state.history.read();
    Ok(ConfigureResponse {
        project,
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label(),
        redo_label: history.redo_label(),
    })
}
pub fn project_validate(state: &Arc<AppState>) -> Result<Vec<ValidationIssue>, String> {
    state.with_project(|project| project.validate())
}
pub fn project_path(state: &Arc<AppState>) -> Option<String> {
    state
        .project_path
        .read()
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// Lifecycle: close, and crash recovery (`recovery.rs`)
// ---------------------------------------------------------------------------

/// Close the open document cleanly: nothing open, no undo history, and the
/// working copy deleted, because the user has saved or chosen to discard by
/// the time a shell calls this. With `exiting`, the app is about to quit and
/// the session lock is released as well.
///
/// The working copy surviving a session is how the next launch knows the
/// session crashed, so every clean way out must come through here.
pub fn project_close(state: &Arc<AppState>, exiting: bool) {
    *state.project.write() = None;
    *state.project_path.write() = None;
    state.history.write().clear();
    super::recovery::close_at(&super::autosave::file(), exiting);
    crate::modules::workspace::commands::workspace_cache_in_use(Vec::new());
}

/// Called once at launch, before any project is opened: set a crashed
/// session's working copy aside and answer what can be restored.
pub fn project_recovery_claim() -> Option<super::recovery::RecoveryInfo> {
    super::recovery::claim_at(&super::autosave::file())
}

/// What can be restored, without claiming anything.
pub fn project_recovery_pending() -> Option<super::recovery::RecoveryInfo> {
    super::recovery::pending_at(&super::autosave::file())
}

/// Open the recovered work. The document comes back pointing at the file it
/// came from, if it had one, so Ctrl+S goes where the user expects — but it is
/// not *in* that file yet, which the shell shows as unsaved.
pub fn project_recovery_restore(state: &Arc<AppState>) -> Result<Project, String> {
    let restored = super::recovery::take_at(&super::autosave::file())?;
    for warning in &restored.warnings {
        tracing::warn!("restoring unsaved work: {warning}");
    }
    let project = restored.project;
    *state.project.write() = Some(project.clone());
    *state.project_path.write() = restored.path.clone();
    state.history.write().clear();
    super::autosave::schedule(&project, restored.path);
    media_opened(&project);
    Ok(project)
}

/// Throw the recovered work away.
pub fn project_recovery_discard() {
    super::recovery::discard_at(&super::autosave::file());
}
