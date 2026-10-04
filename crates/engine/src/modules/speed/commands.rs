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

/// A clip's frame blending; `None` for a clip that has none.
pub fn speed_frame_blend(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<super::blend::FrameBlend, String> {
    state.with_project(|project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or_else(|| format!("unknown segment {segment_id}"))?;
        Ok(super::blend::frame_blend_of(&project.materials, segment))
    })?
}

/// Switch a video clip's frame blending: a slowed-down clip then mixes the two
/// source frames either side of each instant instead of holding one. One undo
/// step. See `blend`.
pub fn speed_set_frame_blend(
    state: &Arc<AppState>,
    segment_id: String,
    mode: super::blend::FrameBlend,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (entry, command) = super::blend::set_frame_blend_command(project, &segment_id, mode)?;
        // Into the pool before the command runs, out again if it is refused:
        // the order `audiofx::commands` keeps.
        match entry {
            Some((id, value)) => {
                project.materials.extras.insert(id.clone(), value);
                if let Err(error) = state.history.write().apply(project, command) {
                    project.materials.extras.remove(&id);
                    return Err(error);
                }
            }
            None => state.history.write().apply(project, command)?,
        }
    }
    crate::modules::voice::commands::respond(state)
}
