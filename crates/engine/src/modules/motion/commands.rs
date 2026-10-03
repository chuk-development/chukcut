//! The motion commands: what the app, a CLI and an MCP server call.
//!
//! Each builds its `EditCommand` in `edit.rs` against the real document and
//! applies it through `History`, so every one is a single undo step.

use std::sync::Arc;

use super::catalog::{self, PresetDescriptor, TextPresetDescriptor};
use super::edit;
use crate::modules::project::animation::{
    AnimationSlot, ClipAnimation, Ease, PunchZoom, TextAnimator, TextSlot,
};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::state::AppState;

/// Everything a UI needs to offer animations.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MotionCatalog {
    pub clip: Vec<PresetDescriptor>,
    pub text: Vec<TextPresetDescriptor>,
    pub easings: Vec<(Ease, &'static str)>,
}

pub fn motion_catalog() -> MotionCatalog {
    MotionCatalog {
        clip: catalog::clip_presets(),
        text: catalog::text_presets(),
        easings: Ease::ALL.iter().map(|e| (*e, e.label())).collect(),
    }
}

/// Set or clear a clip's In, Out or Combo animation.
pub fn motion_set_animation(
    state: &Arc<AppState>,
    segment_id: String,
    slot: AnimationSlot,
    animation: Option<ClipAnimation>,
) -> Result<EditResponse, String> {
    apply(state, |project| {
        edit::set_slot_command(project, &segment_id, slot, animation)
    })
}

/// Set or clear a text clip's per-letter, word or line animation.
pub fn motion_set_text_animation(
    state: &Arc<AppState>,
    segment_id: String,
    slot: TextSlot,
    animator: Option<TextAnimator>,
) -> Result<EditResponse, String> {
    apply(state, |project| {
        edit::set_text_command(project, &segment_id, slot, animator)
    })
}

/// Set or clear a clip's punch-in zoom.
pub fn motion_set_zoom(
    state: &Arc<AppState>,
    segment_id: String,
    zoom: Option<PunchZoom>,
) -> Result<EditResponse, String> {
    apply(state, |project| {
        edit::set_zoom_command(project, &segment_id, zoom)
    })
}

/// Alternate 100 % and `amount` across the jump cuts around `segment_id`.
pub fn motion_auto_zoom(
    state: &Arc<AppState>,
    segment_id: String,
    amount: f32,
) -> Result<EditResponse, String> {
    apply(state, |project| {
        edit::auto_zoom_command(project, &segment_id, amount)
    })
}

fn apply(
    state: &Arc<AppState>,
    build: impl FnOnce(&crate::modules::project::Project) -> Result<EditCommand, String>,
) -> Result<EditResponse, String> {
    let command = {
        let guard = state.project.read();
        let project = guard.as_ref().ok_or("no project is open")?;
        build(project)?
    };
    crate::modules::timeline::commands::timeline_apply(state, command)
}
