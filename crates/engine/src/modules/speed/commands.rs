//! The speed-curve commands: what the app, a CLI and an MCP server call.

use std::sync::Arc;

use super::edit::{self, CurveChange};
use crate::modules::project::speed::SpeedPreset;
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

/// One preset as a UI offers it: its name and its shape, as
/// `(fraction of the clip, speed)` pairs for drawing a thumbnail.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetDescriptor {
    pub preset: SpeedPreset,
    pub label: &'static str,
    pub shape: Vec<(f32, f32)>,
}

/// Every speed-ramp preset, in the order the Curve tab shows them.
pub fn speed_presets() -> Vec<PresetDescriptor> {
    SpeedPreset::ALL
        .iter()
        .map(|&preset| PresetDescriptor {
            preset,
            label: preset.label(),
            shape: preset.shape().to_vec(),
        })
        .collect()
}

/// Give a clip (and every clip linked to it) a speed curve, change it, or
/// remove it. The clip's length follows the curve and the clips after it on
/// its lanes move with its end. One undo step. See `edit::set_curve_command`.
pub fn speed_set_curve(
    state: &Arc<AppState>,
    segment_id: String,
    change: CurveChange,
) -> Result<EditResponse, String> {
    let command = {
        let guard = state.project.read();
        let project = guard.as_ref().ok_or("no project is open")?;
        edit::set_curve_command(project, &segment_id, change)?
    };
    crate::modules::timeline::commands::timeline_apply(state, command)
}
