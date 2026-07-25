//! IPC commands for timeline editing.
//!
//! Each command returns the updated `Project` so the frontend can replace its
//! store in one shot. That is deliberately blunt: incremental patches are
//! faster but produce desync bugs that are miserable to chase, and a project
//! document is a few hundred kilobytes at worst.

use std::sync::Arc;
use tauri::State;

use super::ops::{split_at, EditCommand};
use crate::modules::project::{Micros, Project};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct EditResponse {
    pub project: Project,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
}

fn respond(state: &AppState) -> Result<EditResponse, String> {
    let project = state
        .project
        .read()
        .clone()
        .ok_or("no project is open")?;
    let history = state.history.read();
    Ok(EditResponse {
        project,
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label(),
        redo_label: history.redo_label(),
    })
}

#[tauri::command]
pub fn timeline_apply(
    state: State<'_, Arc<AppState>>,
    command: EditCommand,
) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        state.history.write().apply(project, command)?;
    }
    respond(&state)
}

#[tauri::command]
pub fn timeline_split(
    state: State<'_, Arc<AppState>>,
    segment_id: String,
    at: Micros,
) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        let command = split_at(project, &segment_id, at)?;
        state.history.write().apply(project, command)?;
    }
    respond(&state)
}

#[tauri::command]
pub fn timeline_undo(state: State<'_, Arc<AppState>>) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        state.history.write().undo(project)?;
    }
    respond(&state)
}

#[tauri::command]
pub fn timeline_redo(state: State<'_, Arc<AppState>>) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        state.history.write().redo(project)?;
    }
    respond(&state)
}
