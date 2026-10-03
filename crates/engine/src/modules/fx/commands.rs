//! Commands for built-in effects and layouts: what the UI, a CLI and an MCP
//! server call.
//!
//! Each mutating command builds its edit in `edit.rs` against the document as
//! it is, puts any minted material into the pool first, and pushes the edit
//! through the one history — the contract `inspector_set_color` spells out —
//! then answers with the whole updated project.

use std::path::PathBuf;
use std::sync::Arc;

use super::catalog::{catalog, EffectDescriptor};
use super::edit::{self, ClipPlacement, Corner, SplitLayout};
use crate::modules::project::document::{Micros, Project};
use crate::modules::project::effects::{EffectMaterial, EffectValue};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::state::AppState;

/// Every effect the renderer implements, with its parameters.
pub fn fx_catalog() -> &'static [EffectDescriptor] {
    catalog()
}

/// Apply an edit that minted a material: pool first, then history, and the
/// material back out if the edit is refused.
fn commit(
    state: &Arc<AppState>,
    build: impl FnOnce(&Project) -> Result<(Option<EffectMaterial>, EditCommand), String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (material, command) = build(project)?;
        match material {
            Some(material) => {
                let id = material.id.clone();
                project.materials.effects.push(material);
                if let Err(error) = state.history.write().apply(project, command) {
                    project.materials.effects.retain(|m| m.id != id);
                    return Err(error);
                }
            }
            None => state.history.write().apply(project, command)?,
        }
    }
    respond(state)
}

fn minted(
    built: Result<(EffectMaterial, EditCommand), String>,
) -> Result<(Option<EffectMaterial>, EditCommand), String> {
    built.map(|(m, c)| (Some(m), c))
}

/// Add an effect, at its defaults, to the end of a clip's stack.
pub fn fx_add(
    state: &Arc<AppState>,
    segment_id: String,
    kind: String,
) -> Result<EditResponse, String> {
    commit(state, |p| minted(edit::add_command(p, &segment_id, &kind)))
}

/// Take an effect off a clip.
pub fn fx_remove(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        edit::remove_command(p, &segment_id, &effect_id).map(|c| (None, c))
    })
}

/// Replace an effect's values wholesale, keeping its place in the stack.
pub fn fx_set(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
    effect: EffectMaterial,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        minted(edit::replace_command(
            p,
            &segment_id,
            &effect_id,
            effect,
            "Change effect",
        ))
    })
}

/// Set one parameter. `at` is the playhead on the timeline; an animated
/// parameter is set there.
pub fn fx_set_param(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
    param: String,
    value: EffectValue,
    at: Option<Micros>,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        let source = at.and_then(|t| p.segment(&segment_id).map(|(_, s)| edit::source_time(s, t)));
        minted(edit::set_param_command(
            p,
            &segment_id,
            &effect_id,
            &param,
            value,
            source,
        ))
    })
}

/// Add or remove the keyframe of `param` at the playhead `at`.
pub fn fx_toggle_keyframe(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
    param: String,
    at: Micros,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        let (_, segment) = p
            .segment(&segment_id)
            .ok_or_else(|| format!("unknown segment {segment_id}"))?;
        let source = edit::source_time(segment, at);
        // Half a frame, in source time: the keyframe "at" the playhead.
        let tolerance = (500_000.0 / p.fps.max(1.0) * segment.speed.max(0.01) as f64) as Micros;
        minted(edit::toggle_keyframe_command(
            p,
            &segment_id,
            &effect_id,
            &param,
            source,
            tolerance,
        ))
    })
}

/// Switch an effect off or on.
pub fn fx_set_enabled(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
    enabled: bool,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        minted(edit::set_enabled_command(
            p,
            &segment_id,
            &effect_id,
            enabled,
        ))
    })
}

/// Every parameter of an effect back to its default.
pub fn fx_reset(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        minted(edit::reset_command(p, &segment_id, &effect_id))
    })
}

/// Move an effect to position `to` in the clip's stack.
pub fn fx_move(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
    to: usize,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        edit::move_command(p, &segment_id, &effect_id, to).map(|c| (None, c))
    })
}

/// What `fx_add_clip` answers: the edit, and where the clip went.
pub struct ClipAdded {
    pub edit: EditResponse,
    pub segment_id: String,
    pub track_id: String,
    pub start: Micros,
}

/// Put an effect clip on an effect lane at `at`, applying to everything
/// beneath it for `duration` (three seconds when `None`).
pub fn fx_add_clip(
    state: &Arc<AppState>,
    kind: String,
    at: Micros,
    duration: Option<Micros>,
    lane: Option<String>,
) -> Result<ClipAdded, String> {
    let mut placed: Option<ClipPlacement> = None;
    let edit = commit(state, |p| {
        let placement = edit::clip_command(
            p,
            &kind,
            at,
            duration.unwrap_or(edit::DEFAULT_CLIP_DURATION),
            lane.as_deref(),
        )?;
        let result = (Some(placement.material.clone()), placement.command.clone());
        placed = Some(placement);
        Ok(result)
    })?;
    let placed = placed.expect("set when the edit succeeded");
    Ok(ClipAdded {
        edit,
        segment_id: placed.segment_id,
        track_id: placed.track_id,
        start: placed.start,
    })
}

/// Arrange clips into a split screen, in the order given.
pub fn fx_layout_split(
    state: &Arc<AppState>,
    segment_ids: Vec<String>,
    layout: SplitLayout,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        edit::split_command(p, &segment_ids, layout).map(|c| (None, c))
    })
}

/// Make a clip a picture in picture in `corner`, framed.
pub fn fx_layout_pip(
    state: &Arc<AppState>,
    segment_id: String,
    corner: Corner,
) -> Result<EditResponse, String> {
    commit(state, |p| minted(edit::pip_command(p, &segment_id, corner)))
}

/// The preview tile of an effect, rendered once by the compositor from the
/// sample frame and cached on disk. Blocking: call it off the UI thread.
pub fn fx_tile(kind: String, size: (u32, u32)) -> Result<PathBuf, String> {
    super::tiles::effect_tile(&kind, size)
}

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
