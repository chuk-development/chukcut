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

/// Bake the optical-flow frames a clip with "Optical flow (AI)" on is
/// missing, in the background; the preview shows them as they land and the
/// plain frame blend until then. `None` when nothing is missing. A bake of
/// the same file that is already running is joined. See `flow`.
pub fn speed_flow_bake(state: &Arc<AppState>, segment_id: String) -> Result<Option<u64>, String> {
    super::flow::jobs::bake(state, segment_id)
}

/// A bake's progress, its CPU time warning, and how it ended.
pub fn speed_flow_status(job: u64) -> Option<super::flow::jobs::FlowStatus> {
    super::flow::jobs::status(job)
}

/// Wait for bake `job` to end. For a one-shot CLI run, whose process would
/// otherwise end before its bake; `tick` sees the status every 100 ms.
pub fn speed_flow_wait(
    job: u64,
    tick: impl FnMut(&super::flow::jobs::FlowStatus),
) -> Result<super::flow::bake::FlowOutcome, String> {
    super::flow::jobs::wait(job, tick)
}

/// Stop a bake; the frames made so far stay.
pub fn speed_flow_cancel(job: u64) {
    super::flow::jobs::cancel(job)
}

/// How many of a clip's in-between frames are baked.
pub fn speed_flow_coverage(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<super::flow::jobs::FlowCoverage, String> {
    super::flow::jobs::segment_coverage(state, &segment_id)
}

/// The optical-flow bakes running now, for the app's status line:
/// (job, segment id, progress, CPU time warning).
pub fn speed_flow_running() -> Vec<(u64, String, super::flow::bake::FlowProgress, Option<String>)> {
    super::flow::jobs::running()
}

/// Start a bake for every clip whose optical-flow frames are missing after
/// an edit (a speed change, a trim, an undo, a cleaned cache). Reads files:
/// call it off the UI thread.
pub fn speed_flow_queue_missing(state: &Arc<AppState>) -> Result<Vec<u64>, String> {
    super::flow::jobs::queue_missing(state)
}

/// Bake every optical-flow frame an export of `project` at the timeline
/// instants `times` needs, on this thread, before the first frame renders.
pub fn speed_flow_ensure(
    project: &crate::modules::project::Project,
    times: &[crate::modules::project::Micros],
    progress: &dyn Fn(&str, f32),
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    super::flow::jobs::ensure(project, times, progress, cancel)
}

/// The speed "Smooth slow-mo" sets on a clip that is not slowed down yet.
pub const SMOOTH_SLOW_MO_SPEED: f32 = 0.5;

/// What "Smooth slow-mo" did.
#[derive(serde::Serialize)]
pub struct SlowMoResponse {
    /// `None` when the clip was already slowed with optical flow on.
    #[serde(flatten)]
    pub edit: Option<EditResponse>,
    /// The bake of its in-between frames, when any were missing.
    pub job: Option<u64>,
}

/// "Smooth slow-mo", one click: switch on "Optical flow (AI)", slow the clip
/// down when it is not slowed yet (to [`SMOOTH_SLOW_MO_SPEED`]; a speed
/// below 1 or a speed curve stays as it is) or to `speed` when one is
/// given, and start baking its frames. One undo step for both edits.
pub fn speed_smooth_slow_mo(
    state: &Arc<AppState>,
    segment_id: String,
    speed: Option<f32>,
) -> Result<SlowMoResponse, String> {
    use super::blend::{frame_blend_of, set_frame_blend_command, FrameBlend};
    let edited = {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or_else(|| format!("unknown segment {segment_id}"))?;
        if project.materials.video(&segment.material_id).is_none() {
            return Err("Smooth slow-mo works on video clips".into());
        }
        let slowed = segment.speed < 1.0 || project.materials.speed_curve_of(segment).is_some();
        let flow = frame_blend_of(&project.materials, segment) == FrameBlend::Flow;
        let mut commands = Vec::new();
        let mut entry = None;
        // The blend edit replaces the segment as it is after the speed
        // change, so it is built against a copy with that change made.
        let mut after = project.clone();
        // A speed asked for is set; without one, only a clip that is not
        // slowed yet gets the default.
        let target = match speed {
            Some(speed) => (speed != segment.speed).then_some(speed),
            None => (!slowed).then_some(SMOOTH_SLOW_MO_SPEED),
        };
        if let Some(speed) = target {
            if !(speed > 0.0 && speed < 1.0) {
                return Err("Smooth slow-mo slows a clip down: give a speed below 1".into());
            }
            let command =
                crate::modules::inspector::edit::set_speed_command(&after, &segment_id, speed)?;
            let mut scratch = crate::modules::timeline::History::new();
            scratch.apply(&mut after, command.clone())?;
            commands.push(command);
        }
        if !flow {
            let (made, command) = set_frame_blend_command(&after, &segment_id, FrameBlend::Flow)?;
            entry = made;
            commands.push(command);
        }
        if commands.is_empty() {
            false
        } else {
            if let Some((id, value)) = &entry {
                project.materials.extras.insert(id.clone(), value.clone());
            }
            let command = crate::modules::timeline::ops::EditCommand::Composite {
                label: "Smooth slow-mo".into(),
                commands,
            };
            if let Err(error) = state.history.write().apply(project, command) {
                if let Some((id, _)) = entry {
                    project.materials.extras.remove(&id);
                }
                return Err(error);
            }
            true
        }
    };
    let edit = if edited {
        Some(crate::modules::voice::commands::respond(state)?)
    } else {
        None
    };
    let job = speed_flow_bake(state, segment_id)?;
    Ok(SlowMoResponse { edit, job })
}
