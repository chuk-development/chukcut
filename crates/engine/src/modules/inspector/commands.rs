//! Commands for the inspector's crop and colour panels.
//!
//! Two commands, both thin: `edit.rs` builds the `EditCommand` and everything
//! goes through `History::apply`, so both edits sit on the ordinary undo
//! stack. The commands live here rather than the webview sending the composite
//! itself because the composite is built *from the document* — the segment's
//! current snapshot, its index on its track, which of its extras resolve as a
//! colour material — and a stale panel must be refused against the real
//! document, not against what it last saw.

use std::sync::Arc;

use super::edit::{self, ClipAttributes, ColorEdit};
use crate::modules::project::document::Crop;
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

/// Set or clear a clip's crop rectangle. `None` clears it; so does the
/// full-frame rectangle, which keeps "uncropped" at one spelling.
pub fn inspector_set_crop(
    state: &Arc<AppState>,
    segment_id: String,
    crop: Option<Crop>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let command = edit::set_crop_command(project, &segment_id, crop)?;
        state.history.write().apply(project, command)?;
    }
    respond(&state)
}

/// Set or clear a clip's colour adjustment. `None` — and the identity values —
/// clear it.
pub fn inspector_set_color(
    state: &Arc<AppState>,
    segment_id: String,
    color: Option<ColorEdit>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (material, command) = edit::set_color_command(project, &segment_id, color)?;

        // The material goes into the pool before the command runs, the
        // ordering `text_add` documents: the document must never, even between
        // two writes, hold a segment referencing a material the pool lacks.
        // Direct rather than through a command for the reason given there — an
        // unreferenced material is inert — but unlike `text_add` this one is
        // taken back out if the edit is refused, because nothing else will
        // ever point at it.
        if let Some(material) = material {
            let id = material.id.clone();
            project.materials.color_adjusts.push(material);
            if let Err(error) = state.history.write().apply(project, command) {
                project.materials.color_adjusts.retain(|m| m.id != id);
                return Err(error);
            }
        } else {
            state.history.write().apply(project, command)?;
        }
    }
    respond(&state)
}

/// Apply a copied clip's transform, speed, volume, crop and colour grade to
/// every clip in `segment_ids`, as one undo step.
///
/// The attribute values come from the webview's clipboard rather than from a
/// segment id, because the clipboard outlives the document it copied from; the
/// composite itself is built here against the real document, so a stale
/// selection is skipped and the grade lands as one shared immutable material.
pub fn inspector_paste_attributes(
    state: &Arc<AppState>,
    attributes: ClipAttributes,
    segment_ids: Vec<String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (material, command) =
            edit::paste_attributes_command(project, &attributes, &segment_ids)?;

        // Pool before command, with the take-back on failure: the same
        // contract as `inspector_set_color` above, for the same reasons.
        if let Some(material) = material {
            let id = material.id.clone();
            project.materials.color_adjusts.push(material);
            if let Err(error) = state.history.write().apply(project, command) {
                project.materials.color_adjusts.retain(|m| m.id != id);
                return Err(error);
            }
        } else {
            state.history.write().apply(project, command)?;
        }
    }
    respond(&state)
}

/// Name a clip, or clear its name. The name lives in `MaterialPool::extras`
/// and is resolved by the webview's label code; see `edit::rename_clip_command`
/// for why a segment grows no field for it.
pub fn inspector_rename_clip(
    state: &Arc<AppState>,
    segment_id: String,
    name: Option<String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (entry, command) = edit::rename_clip_command(project, &segment_id, name)?;

        if let Some((id, value)) = entry {
            project.materials.extras.insert(id.clone(), value);
            if let Err(error) = state.history.write().apply(project, command) {
                project.materials.extras.remove(&id);
                return Err(error);
            }
        } else {
            state.history.write().apply(project, command)?;
        }
    }
    respond(&state)
}

/// What the panel wants to know about a .cube file before attaching it.
#[derive(serde::Serialize)]
pub struct LutInfo {
    /// The file's `TITLE`, when it declares one.
    pub title: Option<String>,
    /// Edge length `N` of the 3D table.
    pub size: u32,
}

/// Read and parse a .cube file, without touching the document.
///
/// The one place a LUT file is validated *eagerly*: the picker calls this so
/// a malformed file is refused with the parser's line-numbered message at the
/// moment the user chooses it. `inspector_set_color` itself never does IO —
/// re-committing an intensity change must keep working after the file has
/// gone missing, because "missing LUT" is a warning state, not an error one.
pub fn inspector_lut_probe(path: String) -> Result<LutInfo, String> {
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read {path}: {error}"))?;
    let cube = crate::modules::render::lut::parse(&text)?;
    Ok(LutInfo {
        title: cube.title,
        size: cube.size,
    })
}

/// The reply to an edit, with the working copy written on the way out.
///
/// A copy of `timeline::commands::respond`, which is private to that module —
/// the same duplication `text::commands` carries, for the same reason given
/// there.
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
