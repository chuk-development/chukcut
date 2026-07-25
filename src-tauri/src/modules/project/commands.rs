//! IPC commands for creating, opening and saving projects.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

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

#[tauri::command]
pub fn project_new(
    state: State<'_, Arc<AppState>>,
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
    Ok(project)
}

#[tauri::command]
pub fn project_open(state: State<'_, Arc<AppState>>, path: String) -> Result<Project, String> {
    let raw = fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let project: Project =
        serde_json::from_str(&raw).map_err(|e| format!("cannot parse {path}: {e}"))?;

    *state.project.write() = Some(project.clone());
    *state.project_path.write() = Some(PathBuf::from(path));
    state.history.write().clear();
    Ok(project)
}

#[tauri::command]
pub fn project_save(state: State<'_, Arc<AppState>>, path: Option<String>) -> Result<String, String> {
    let target = match path {
        Some(p) => PathBuf::from(p),
        None => state
            .project_path
            .read()
            .clone()
            .ok_or("project has never been saved; a path is required")?,
    };

    state.with_project(|project| write_project(&target, project))??;
    *state.project_path.write() = Some(target.clone());
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
#[tauri::command]
pub fn project_import_media(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<ImportedMaterial, String> {
    let info = crate::modules::media::probe(&path).map_err(|e| e.to_string())?;

    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    let mut guard = state.project.write();
    let project = guard.as_mut().ok_or("no project is open")?;

    // Already imported? Hand back what is there.
    if let Some(existing) = project.materials.videos.iter().find(|m| m.path == path) {
        return Ok(ImportedMaterial {
            id: existing.id.clone(),
            kind: MaterialKind::Video,
            name,
            path,
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
            name,
            path,
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
            name,
            path,
            duration: existing.duration,
            width: 0,
            height: 0,
            has_audio: true,
        });
    }

    let id = new_id();

    match (&info.video, &info.audio) {
        (Some(video), _) if is_still_image(&info.format) => {
            project.materials.images.push(ImageMaterial {
                id: id.clone(),
                path: path.clone(),
                width: video.display_width,
                height: video.display_height,
            });
            Ok(ImportedMaterial {
                id,
                kind: MaterialKind::Image,
                name,
                path,
                duration: 0,
                width: video.display_width,
                height: video.display_height,
                has_audio: false,
            })
        }

        (Some(video), _) => {
            project.materials.videos.push(VideoMaterial {
                id: id.clone(),
                path: path.clone(),
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
                name,
                path,
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
                path: path.clone(),
                duration: info.duration,
                sample_rate: audio.sample_rate,
                channels: audio.channels,
            });
            Ok(ImportedMaterial {
                id,
                kind: MaterialKind::Audio,
                name,
                path,
                duration: info.duration,
                width: 0,
                height: 0,
                has_audio: true,
            })
        }

        (None, None) => Err(format!("{name} contains no video or audio stream")),
    }
}

#[tauri::command]
pub fn project_get(state: State<'_, Arc<AppState>>) -> Result<Option<Project>, String> {
    Ok(state.project.read().clone())
}

#[tauri::command]
pub fn project_validate(state: State<'_, Arc<AppState>>) -> Result<Vec<ValidationIssue>, String> {
    state.with_project(|project| project.validate())
}

#[tauri::command]
pub fn project_path(state: State<'_, Arc<AppState>>) -> Option<String> {
    state
        .project_path
        .read()
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
}
