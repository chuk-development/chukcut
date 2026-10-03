//! Commands for titles.
//!
//! Three of them, and between them they are the whole of "add a title to a
//! video" as far as Rust is concerned: list the fonts this machine can actually
//! draw, put a title on the timeline, and change one that is already there.
//!
//! ## The one place this deviates from "every mutation is an `EditCommand`"
//!
//! [`text_add`] obeys the rule for the part that matters: the segment is
//! inserted through `History::apply`, so a title appears and disappears with
//! Ctrl+Z like any other clip. The *material* is pushed into the pool directly,
//! exactly as `project_import_media` does and for the same stated reason — a
//! material nothing references is inert, and putting library additions in the
//! undo stack means Ctrl+Z after a cut silently empties the panel.
//!
//! [`text_set`] is the deviation, and it is worth naming rather than hiding.
//! There is no `EditCommand` variant that carries a `TextMaterial`, so changing
//! the words in a title is not undoable. Two things about that:
//!
//! - It is not obviously wrong. The inspector debounces typing, but even so a
//!   sentence typed one character at a time would be forty undo steps, and no
//!   editor makes Ctrl+Z walk backwards through a title letter by letter.
//! - It is still a gap, and the fix is small and known: an
//!   `EditCommand::SetTextMaterial { id, before, after }`, mirroring
//!   `SetTransition` exactly — which is the variant `transitions` already has
//!   for precisely this shape of edit. It is not here because
//!   `timeline/ops.rs` was owned by other work when this landed.
//!
//! [`text_set_content`] is the undoable path for the words alone: one
//! `EditCommand::SetTextMaterial` per finished edit.
//!
//! All paths schedule an autosave, so none is lost to a restart.

use std::sync::Arc;

use super::edit;
use crate::modules::project::document::{Micros, TextMaterial};
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

/// Every font family this machine can draw, sorted.
///
/// From the same `fontique` collection the rasteriser resolves against, so a
/// name offered here is a name that will render. Building the shared renderer
/// scans the system's fonts — tens of milliseconds, once per process — which is
/// why this goes off the main thread even though every call after the first is
/// a clone of a `Vec<String>`.
pub async fn text_fonts() -> Result<Vec<String>, String> {
    crate::shell::spawn_blocking(|| super::TextRenderer::shared().font_families())
        .await
        .map_err(|error| format!("listing the fonts failed: {error}"))
}

/// What the frontend needs after a title has been added: the document, the
/// history state, and enough identity to select what was just created.
#[derive(serde::Serialize)]
pub struct TextAdded {
    #[serde(flatten)]
    pub edit: EditResponse,
    pub material_id: String,
    pub segment_id: String,
    pub track_id: String,
    /// Where it actually landed, which is not `at` when that instant was taken.
    pub start: Micros,
}

/// Put a new title on the timeline at `at`.
///
/// `content` is optional: the button sends nothing and gets the placeholder,
/// while a preset can send its own words. `duration` likewise defaults to
/// [`edit::DEFAULT_DURATION`].
pub fn text_add(
    state: &Arc<AppState>,
    at: Micros,
    content: Option<String>,
    duration: Option<Micros>,
) -> Result<TextAdded, String> {
    let (material_id, placement) = {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;

        let material = edit::default_material(project, content);
        let material_id = material.id.clone();
        let placement = edit::insert_command(
            project,
            &material_id,
            at,
            duration.unwrap_or(edit::DEFAULT_DURATION),
        )?;

        // Before the command, because `InsertSegment` produces a document that
        // `validate()` reads: a segment naming a material that is not in the
        // pool is an error there, and it would be a real one for the moment
        // between the two writes if an autosave landed in between.
        project.materials.texts.push(material);
        state
            .history
            .write()
            .apply(project, placement.command.clone())?;

        (material_id, placement)
    };

    Ok(TextAdded {
        edit: respond(&state)?,
        material_id,
        segment_id: placement.segment_id,
        track_id: placement.track_id,
        start: placement.start,
    })
}

/// Replace a title's parameters wholesale, keeping its identity.
///
/// Wholesale rather than a patch of changed fields for the reason the rest of
/// this boundary gives: the document is server state, the frontend holds a
/// complete copy of it, and a patch is a second description of the same object
/// that can disagree with the first.
pub fn text_set(state: &Arc<AppState>, material: TextMaterial) -> Result<EditResponse, String> {
    // Before the lock, because a rejected edit should cost nothing and because
    // the message is the user's.
    edit::check_material(&material)?;

    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let slot = project
            .materials
            .texts
            .iter_mut()
            .find(|m| m.id == material.id)
            .ok_or_else(|| format!("no title with the id {}", material.id))?;
        *slot = material;
    }

    respond(&state)
}

/// Change the words of a title, as one undo step.
///
/// Unlike [`text_set`], this goes through the history as
/// `EditCommand::SetTextMaterial`: it is what a finished edit sends (the
/// timeline's inline editor commits once, on Enter or a click elsewhere), not
/// what each keystroke sends. Only the content changes; the style stays.
pub fn text_set_content(
    state: &Arc<AppState>,
    material_id: &str,
    content: &str,
) -> Result<EditResponse, String> {
    let before = state
        .with_project(|p| p.materials.text(material_id).cloned())?
        .ok_or_else(|| format!("no title with the id {material_id}"))?;
    let mut after = before.clone();
    after.content = content.to_string();
    crate::modules::timeline::commands::timeline_apply(
        state,
        crate::modules::timeline::ops::EditCommand::SetTextMaterial { before, after },
    )
}

/// Copy a title's material under a fresh id and return that id.
///
/// What a pasted or duplicated title needs: two clips naming one material
/// would make editing one title's words edit the other's. The material goes
/// into the pool directly, for the reason [`text_add`] gives — a material
/// nothing references is inert — and the clip that names it is inserted
/// through `History::apply` by the caller, so the paste itself undoes.
pub fn text_duplicate(state: &Arc<AppState>, material_id: String) -> Result<String, String> {
    let mut guard = state.project.write();
    let project = guard.as_mut().ok_or("no project is open")?;
    let mut copy = project
        .materials
        .texts
        .iter()
        .find(|m| m.id == material_id)
        .cloned()
        .ok_or_else(|| format!("no title with the id {material_id}"))?;
    copy.id = crate::modules::project::new_id();
    let id = copy.id.clone();
    project.materials.texts.push(copy);
    Ok(id)
}

/// The reply to an edit, with the working copy written on the way out.
///
/// A copy of `timeline::commands::respond`, which is private to that module.
/// Duplicated rather than exported because the two will diverge the moment
/// `EditCommand::SetTextMaterial` exists and `text_set` starts going through
/// the history like everything else.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Project};

    /// A title's new words are one undo step, and undo brings the old ones
    /// back with the style untouched.
    #[test]
    fn setting_a_titles_content_is_one_undo_step() {
        let state = AppState::new();
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut original = super::edit::default_material(&project, Some("Hello".into()));
        original.font_size = 77.0;
        let id = original.id.clone();
        project.materials.texts.push(original);
        *state.project.write() = Some(project);

        let content = |state: &Arc<AppState>| {
            state
                .with_project(|p| p.materials.text(&id).cloned())
                .unwrap()
                .unwrap()
        };
        let response = text_set_content(&state, &id, "Hello\nworld").expect("set");
        assert!(response.can_undo);
        assert_eq!(content(&state).content, "Hello\nworld");
        assert_eq!(content(&state).font_size, 77.0);

        crate::modules::timeline::commands::timeline_undo(&state).expect("undo");
        assert_eq!(content(&state).content, "Hello");
        assert!(!state.history.read().can_undo());
        assert!(text_set_content(&state, "nope", "x").is_err());
    }

    /// A duplicated title is a second, equal material under its own id, so a
    /// pasted title can be edited without editing the one it came from.
    #[test]
    fn duplicating_a_title_mints_an_equal_material_under_a_new_id() {
        let state = AppState::new();
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut original = super::edit::default_material(&project, Some("Hello".into()));
        original.font_size = 77.0;
        let id = original.id.clone();
        project.materials.texts.push(original);
        *state.project.write() = Some(project);

        let copy = text_duplicate(&state, id.clone()).expect("duplicated");
        assert_ne!(copy, id);
        let texts = state.with_project(|p| p.materials.texts.clone()).unwrap();
        assert_eq!(texts.len(), 2);
        let made = texts.iter().find(|m| m.id == copy).expect("in the pool");
        assert_eq!(made.content, "Hello");
        assert_eq!(made.font_size, 77.0);
        assert!(text_duplicate(&state, "nope".into()).is_err());
    }

    /// `TextAdded` is flattened, and the frontend's type says so.
    ///
    /// `src/modules/text/lib/api.ts` declares `interface TextAdded extends
    /// EditResponse`, which is only true if `can_undo` and friends sit at the
    /// top level rather than under an `edit` key. A `#[serde(flatten)]` is easy
    /// to drop while refactoring and nothing else would notice until the undo
    /// button stopped updating after a title was added.
    #[test]
    fn the_add_response_carries_the_edit_response_at_the_top_level() {
        let added = TextAdded {
            edit: EditResponse {
                project: Project::new("t", CanvasConfig::default(), 30.0),
                can_undo: true,
                can_redo: false,
                undo_label: Some("Add title".into()),
                redo_label: None,
            },
            material_id: "m1".into(),
            segment_id: "s1".into(),
            track_id: "t1".into(),
            start: 1_500_000,
        };

        let json: serde_json::Value = serde_json::to_value(&added).expect("serialize");
        let object = json.as_object().expect("an object");

        for key in [
            "project",
            "can_undo",
            "can_redo",
            "undo_label",
            "material_id",
        ] {
            assert!(object.contains_key(key), "{key} is missing from {json}");
        }
        assert!(
            !object.contains_key("edit"),
            "the EditResponse was nested instead of flattened: {json}"
        );
        assert_eq!(object["can_undo"], serde_json::Value::Bool(true));
        assert_eq!(object["start"], serde_json::Value::from(1_500_000));
        assert_eq!(object["segment_id"], serde_json::Value::from("s1"));
    }
}
