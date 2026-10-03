//! Commands for timeline editing.
//!
//! Each command returns the updated `Project` so the frontend can replace its
//! store in one shot. That is deliberately blunt: incremental patches are
//! faster but produce desync bugs that are miserable to chase, and a project
//! document is a few hundred kilobytes at worst.

use std::sync::Arc;

use super::ops::{compose_edits, link, split_all_at, split_at, unlink, EditCommand};
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

/// Build the reply to an edit, and persist the working copy on the way out.
///
/// Every path that changes the document ends here — apply, split, undo and
/// redo alike — which is what makes "every edit is on disk" true rather than
/// nearly true. The write itself happens on the autosave thread, so no edit
/// waits on the disk, and it is deliberately *not* an entry in `History`: undo
/// reverses what the user did, and saving is not something they did.
fn respond(state: &AppState) -> Result<EditResponse, String> {
    let project = state.project.read().clone().ok_or("no project is open")?;
    let origin = state.project_path.read().clone();
    crate::modules::project::autosave::schedule(&project, origin);

    let history = state.history.read();
    Ok(EditResponse {
        project,
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label(),
        redo_label: history.redo_label(),
    })
}
pub fn timeline_apply(state: &Arc<AppState>, command: EditCommand) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        state.history.write().apply(project, command)?;
    }
    respond(&state)
}

/// Apply one edit per clip as a single undo step.
///
/// What a multi-selection produces. The frontend sends one primitive per clip
/// it means to touch and nothing else — no link partners, no ordering — because
/// both of those are facts about the document rather than about what the user
/// could see, and `compose_edits` is where the document is. See its docs for
/// why a batch cannot simply be a `Composite` the webview built itself.
///
/// `label` is what the undo menu will say, because only the caller knows
/// whether four `RemoveSegment`s were a delete or the tail of a paste.
pub fn timeline_apply_many(
    state: &Arc<AppState>,
    commands: Vec<EditCommand>,
    label: String,
) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        let command = compose_edits(project, &label, commands)?;
        state.history.write().apply(project, command)?;
    }
    respond(&state)
}
pub fn timeline_split(
    state: &Arc<AppState>,
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

/// Split every unlocked clip under the playhead, across all tracks, as one
/// undo step. See `ops::split_all_at` for the cluster rule that keeps a linked
/// pair from being cut twice.
pub fn timeline_split_all(state: &Arc<AppState>, at: Micros) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        let command = split_all_at(project, at)?;
        state.history.write().apply(project, command)?;
    }
    respond(&state)
}

/// Break the link between a clip and whatever it moves with.
///
/// After this the two are ordinary clips: the mirroring in `History::apply`
/// finds no group and every later edit touches exactly what it names.
pub fn timeline_unlink(state: &Arc<AppState>, segment_id: String) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        let command = unlink(project, &segment_id)?;
        state.history.write().apply(project, command)?;
    }
    respond(&state)
}

/// Make several clips move, trim and delete as one.
pub fn timeline_link(
    state: &Arc<AppState>,
    segment_ids: Vec<String>,
) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        let command = link(project, &segment_ids)?;
        state.history.write().apply(project, command)?;
    }
    respond(&state)
}
pub fn timeline_undo(state: &Arc<AppState>) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        state.history.write().undo(project)?;
    }
    respond(&state)
}
pub fn timeline_redo(state: &Arc<AppState>) -> Result<EditResponse, String> {
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        state.history.write().redo(project)?;
    }
    respond(&state)
}

/// Hold the frame of `segment_id` at timeline time `at` for `duration` µs:
/// cut the clip there, put the still in between, and push everything after it
/// on the clip's lane and its linked lanes right. One undo step.
///
/// Blocking — it decodes a frame — so the app runs it off the UI thread. The
/// project lock is held only to read what to decode and, afterwards, to apply
/// the edit; never across the decode. The edit is built against the document
/// as it is *after* the decode, so a change made meanwhile is either respected
/// or refused, never overwritten.
pub fn timeline_freeze_frame(
    state: &Arc<AppState>,
    segment_id: String,
    at: Micros,
    duration: Micros,
) -> Result<EditResponse, String> {
    let source = {
        let guard = state.project.read();
        let project = guard.as_ref().ok_or("no project is open")?;
        super::freeze::freeze_source(project, &segment_id, at)?
    };
    let output = super::freeze::freeze_output_path_for(&source);
    let image = super::freeze::extract_frame(&source, &output)?;
    super::freeze::note_created(&output);
    {
        let mut project_guard = state.project.write();
        let project = project_guard.as_mut().ok_or("no project is open")?;
        let command = super::freeze::freeze_frame_edit(project, &segment_id, at, duration, image)?;
        state.history.write().apply(project, command)?;
    }
    respond(state)
}
