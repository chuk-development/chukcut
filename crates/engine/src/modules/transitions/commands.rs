//! Commands for transitions.
//!
//! The mutating ones build an `EditCommand` and push it through the same
//! history every other edit goes through, then answer with the whole updated
//! project — the shape `timeline/commands.rs` established, and for the same
//! reason: incremental patches are faster and produce desync bugs that are
//! miserable to chase.
//!
//! They exist as commands of their own rather than leaving the frontend to
//! construct an `AddTransition` in TypeScript because constructing one means
//! minting a uuid, knowing the default duration, and knowing how short a clip
//! shortens it to. That is three pieces of policy, and policy in the webview is
//! policy in two places.

use std::sync::Arc;

use super::edit;
use crate::modules::project::document::{Micros, TransitionKind, TransitionMaterial};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::state::AppState;

use super::catalog::{catalog, TransitionDescriptor};

/// Every transition the renderer implements, with the controls each one has.
pub fn transitions_catalog() -> Vec<TransitionDescriptor> {
    catalog()
}

/// The longest transition that may sit at the head of `segment_id`, in
/// microseconds. `0` means one may not be placed there at all.
///
/// The timeline asks this before it lets a transition be dropped, so the drag
/// can be refused with a cursor rather than with an error dialog.
pub fn transitions_max_duration(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<Micros, String> {
    state.with_project(|project| edit::allowed_duration(project, &segment_id))
}
pub fn transitions_add(
    state: &Arc<AppState>,
    segment_id: String,
    kind: TransitionKind,
    duration: Option<Micros>,
) -> Result<EditResponse, String> {
    apply(&state, |project| {
        edit::add_command(project, &segment_id, kind, duration)
    })
}
pub fn transitions_remove(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<EditResponse, String> {
    apply(&state, |project| edit::remove_command(project, &segment_id))
}
pub fn transitions_retime(
    state: &Arc<AppState>,
    segment_id: String,
    duration: Micros,
) -> Result<EditResponse, String> {
    apply(&state, |project| {
        edit::retime_command(project, &segment_id, duration)
    })
}

/// Replace an existing transition's parameters wholesale, keeping its identity.
/// What a parameter panel sends when the user lets go of a control.
pub fn transitions_set(
    state: &Arc<AppState>,
    segment_id: String,
    transition: TransitionMaterial,
) -> Result<EditResponse, String> {
    apply(&state, move |project| {
        edit::set_command(project, &segment_id, transition.clone())
    })
}

/// Build a command against the open project, apply it, and answer with the
/// result.
///
/// The builder runs while the write lock is held because it reads the document
/// to decide what the command should be — the current transition, the room
/// available — and a command built against a project that changed underneath it
/// is a command that will be rejected or, worse, accepted against the wrong
/// clip.
fn apply(
    state: &AppState,
    build: impl FnOnce(&crate::modules::project::Project) -> Result<EditCommand, String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let command = build(project)?;
        state.history.write().apply(project, command)?;
    }

    let project = state.project.read().clone().ok_or("no project is open")?;
    let history = state.history.read();
    Ok(EditResponse {
        project,
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label(),
        redo_label: history.redo_label(),
    })
}
