//! The sequence commands the app, the CLI and the MCP server call.
//!
//! Every mutation is one `EditCommand` built in [`super::build`] and applied
//! through `timeline_apply`, so it lands on the one undo stack, autosaves, and
//! answers with the same `EditResponse` every other edit does.

use std::sync::Arc;

use super::{build, SequenceInfo};
use crate::modules::project::{Id, Project};
use crate::modules::timeline::commands::{timeline_apply, EditResponse};
use crate::modules::timeline::ops::EditCommand;
use crate::state::AppState;

/// Build an edit against the open project and apply it.
fn edit<T>(
    state: &Arc<AppState>,
    make: impl FnOnce(&Project) -> Result<(EditCommand, T), String>,
) -> Result<(EditResponse, T), String> {
    let (command, extra) = state.with_project(make)??;
    Ok((timeline_apply(state, command)?, extra))
}

fn plain(
    state: &Arc<AppState>,
    make: impl FnOnce(&Project) -> Result<EditCommand, String>,
) -> Result<EditResponse, String> {
    edit(state, |p| make(p).map(|c| (c, ()))).map(|(r, ())| r)
}

/// Every sequence in tab order — timelines and compound clips — with the
/// active one marked.
pub fn sequence_list(state: &Arc<AppState>) -> Result<Vec<SequenceInfo>, String> {
    state.with_project(super::list)
}

/// The top-level timelines, for the tabs.
pub fn sequence_timelines(state: &Arc<AppState>) -> Result<Vec<SequenceInfo>, String> {
    state.with_project(super::timelines)
}

/// `(id, name)` from the root timeline down to the open compound clip.
pub fn sequence_breadcrumbs(state: &Arc<AppState>) -> Result<Vec<(Id, String)>, String> {
    state.with_project(super::breadcrumbs)
}

/// What [`sequence_compound_create`] made.
pub struct CompoundCreated {
    pub response: EditResponse,
    pub sequence_id: Id,
    pub segment_id: Id,
}

/// Move the clips `segment_ids` (and their link partners) into a new
/// compound clip in their place.
pub fn sequence_compound_create(
    state: &Arc<AppState>,
    segment_ids: Vec<String>,
    name: Option<String>,
) -> Result<CompoundCreated, String> {
    let (response, (sequence_id, segment_id)) = edit(state, |p| {
        let made = build::create_compound(p, &segment_ids, name)?;
        Ok((made.command, (made.sequence_id, made.segment_id)))
    })?;
    Ok(CompoundCreated {
        response,
        sequence_id,
        segment_id,
    })
}

/// Put a compound clip's contents back on the timeline in its place.
pub fn sequence_compound_flatten(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<EditResponse, String> {
    plain(state, |p| build::flatten(p, &segment_id))
}

/// Open a compound clip for editing.
pub fn sequence_compound_open(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<EditResponse, String> {
    plain(state, |p| build::open(p, &segment_id))
}

/// Close the innermost open compound clip.
pub fn sequence_compound_close(state: &Arc<AppState>) -> Result<EditResponse, String> {
    plain(state, build::close)
}

/// Go out to breadcrumb `level` (0 is the timeline).
pub fn sequence_compound_close_to(
    state: &Arc<AppState>,
    level: usize,
) -> Result<EditResponse, String> {
    plain(state, |p| build::close_to(p, level))
}

/// A new empty timeline, opened. Answers its id with the response.
pub fn sequence_timeline_new(
    state: &Arc<AppState>,
    name: Option<String>,
) -> Result<(EditResponse, Id), String> {
    edit(state, |p| build::new_timeline(p, name))
}

/// Rename a timeline or a compound clip's sequence.
pub fn sequence_rename(
    state: &Arc<AppState>,
    id: String,
    name: String,
) -> Result<EditResponse, String> {
    plain(state, |p| build::rename(p, &id, &name))
}

/// Delete a timeline (never the last one).
pub fn sequence_timeline_delete(state: &Arc<AppState>, id: String) -> Result<EditResponse, String> {
    plain(state, |p| build::delete_timeline(p, &id))
}

/// Copy a timeline into a new tab. Answers the copy's id.
pub fn sequence_timeline_duplicate(
    state: &Arc<AppState>,
    id: String,
) -> Result<(EditResponse, Id), String> {
    edit(state, |p| build::duplicate_timeline(p, &id))
}

/// Make another timeline the one being edited.
pub fn sequence_timeline_switch(state: &Arc<AppState>, id: String) -> Result<EditResponse, String> {
    plain(state, |p| build::switch(p, &id))
}
