//! IPC commands for creating, opening and saving projects.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

use super::document::{CanvasConfig, Project, Track, TrackKind, ValidationIssue};
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
