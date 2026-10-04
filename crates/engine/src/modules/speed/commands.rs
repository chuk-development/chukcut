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

/// The Speed › "Speed effects" tiles: a ramp and the frame smoothing that
/// suits it, in one click. A ramp alone (the Curve tab) stutters in its slow
/// part, because a slowed clip holds each source frame; each effect here
/// switches on the smoothing that hides that. The heavy ones, whose slow
/// part is a tenth to a quarter of real time, get "Optical flow (AI)"; the
/// rest get frame blending, which is free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedEffect {
    Montage,
    Hero,
    Bullet,
    JumpCut,
    FlashIn,
    FlashOut,
}

impl SpeedEffect {
    pub const ALL: [SpeedEffect; 6] = [
        SpeedEffect::Montage,
        SpeedEffect::Hero,
        SpeedEffect::Bullet,
        SpeedEffect::JumpCut,
        SpeedEffect::FlashIn,
        SpeedEffect::FlashOut,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SpeedEffect::Montage => "Smooth montage",
            SpeedEffect::Hero => "Hero moment",
            SpeedEffect::Bullet => "Bullet time",
            SpeedEffect::JumpCut => "Smooth jump",
            SpeedEffect::FlashIn => "Flash in",
            SpeedEffect::FlashOut => "Flash out",
        }
    }

    /// The ramp it lays over the clip.
    pub fn preset(self) -> SpeedPreset {
        match self {
            SpeedEffect::Montage => SpeedPreset::Montage,
            SpeedEffect::Hero => SpeedPreset::Hero,
            SpeedEffect::Bullet => SpeedPreset::Bullet,
            SpeedEffect::JumpCut => SpeedPreset::JumpCut,
            SpeedEffect::FlashIn => SpeedPreset::FlashIn,
            SpeedEffect::FlashOut => SpeedPreset::FlashOut,
        }
    }

    /// The smoothing it switches on.
    pub fn smoothing(self) -> super::blend::FrameBlend {
        use super::blend::FrameBlend;
        match self {
            SpeedEffect::Hero | SpeedEffect::Bullet => FrameBlend::Flow,
            _ => FrameBlend::Blend,
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let wanted = text.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Self::ALL
            .into_iter()
            .find(|e| {
                serde_json::to_value(e)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    == Some(wanted.clone())
            })
            .ok_or_else(|| {
                format!(
                    "there is no speed effect {text}; one of montage, hero, bullet, jump_cut, \
                     flash_in, flash_out"
                )
            })
    }
}

/// One speed effect as a UI offers it: its name, the ramp's shape for the
/// thumbnail, and the smoothing it turns on.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpeedEffectDescriptor {
    pub effect: SpeedEffect,
    pub label: &'static str,
    pub shape: Vec<(f32, f32)>,
    pub smoothing: &'static str,
}

/// Every speed effect, in the order the Speed effects tab shows them.
pub fn speed_effects() -> Vec<SpeedEffectDescriptor> {
    SpeedEffect::ALL
        .iter()
        .map(|&effect| SpeedEffectDescriptor {
            effect,
            label: effect.label(),
            shape: effect.preset().shape().to_vec(),
            smoothing: effect.smoothing().name(),
        })
        .collect()
}

/// Which speed effect `segment_id` wears now: its curve is the effect's
/// untouched preset and its smoothing is the effect's. `None` otherwise.
pub fn speed_effect_of(
    project: &crate::modules::project::document::Project,
    segment_id: &str,
) -> Option<SpeedEffect> {
    let (_, segment) = project.segment(segment_id)?;
    let preset = project.materials.speed_curve_of(segment)?.preset?;
    let blend = super::blend::frame_blend_of(&project.materials, segment);
    SpeedEffect::ALL
        .into_iter()
        .find(|e| e.preset() == preset && e.smoothing() == blend)
}

/// Put a speed effect on a video clip: its ramp (on every clip linked to it,
/// as the Curve tab does) and its smoothing, as one undo step, then start
/// baking the optical-flow frames when the effect uses them. What is
/// already in place is left alone, so applying an effect twice is not an
/// error and changes nothing.
pub fn speed_apply_effect(
    state: &Arc<AppState>,
    segment_id: String,
    effect: SpeedEffect,
) -> Result<SlowMoResponse, String> {
    use super::blend::{frame_blend_of, set_frame_blend_command, FrameBlend};
    let edited = {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or_else(|| format!("unknown segment {segment_id}"))?;
        if project.materials.video(&segment.material_id).is_none() {
            return Err("speed effects work on video clips".into());
        }
        let has_ramp = project
            .materials
            .speed_curve_of(segment)
            .is_some_and(|c| c.preset == Some(effect.preset()));
        let has_smoothing = frame_blend_of(&project.materials, segment) == effect.smoothing();
        let mut commands = Vec::new();
        let mut entry = None;
        // The blend edit replaces the segment as it is after the ramp, so it
        // is built against a copy with the ramp in place.
        let mut after = project.clone();
        if !has_ramp {
            let command = edit::set_curve_command(
                &after,
                &segment_id,
                CurveChange::Preset {
                    preset: effect.preset(),
                },
            )?;
            let mut scratch = crate::modules::timeline::History::new();
            scratch.apply(&mut after, command.clone())?;
            commands.push(command);
        }
        if !has_smoothing && effect.smoothing() != FrameBlend::None {
            let (made, command) = set_frame_blend_command(&after, &segment_id, effect.smoothing())?;
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
                label: effect.label().into(),
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
    let job = if effect.smoothing() == super::blend::FrameBlend::Flow {
        speed_flow_bake(state, segment_id)?
    } else {
        None
    };
    Ok(SlowMoResponse { edit, job })
}

/// Take a speed effect off: the ramp and the smoothing, one undo step. The
/// clip plays at its constant speed again.
pub fn speed_remove_effect(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<EditResponse, String> {
    use super::blend::{frame_blend_of, set_frame_blend_command, FrameBlend};
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or_else(|| format!("unknown segment {segment_id}"))?;
        let has_curve = project.materials.speed_curve_of(segment).is_some();
        let blended = frame_blend_of(&project.materials, segment) != FrameBlend::None;
        let mut after = project.clone();
        let mut commands = Vec::new();
        if has_curve {
            let command = edit::set_curve_command(&after, &segment_id, CurveChange::Remove)?;
            let mut scratch = crate::modules::timeline::History::new();
            scratch.apply(&mut after, command.clone())?;
            commands.push(command);
        }
        if blended {
            let (_, command) = set_frame_blend_command(&after, &segment_id, FrameBlend::None)?;
            commands.push(command);
        }
        if commands.is_empty() {
            return Err("the clip has no speed effect".into());
        }
        let command = crate::modules::timeline::ops::EditCommand::Composite {
            label: "Remove speed effect".into(),
            commands,
        };
        state.history.write().apply(project, command)?;
    }
    crate::modules::voice::commands::respond(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
    };
    use crate::modules::speed::blend::{frame_blend_of, FrameBlend};
    use crate::modules::timeline::commands::timeline_undo;

    fn state() -> Arc<AppState> {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "v".into(),
            path: "/nowhere.mp4".into(),
            width: 64,
            height: 64,
            duration: 4_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(Segment {
            id: "s".into(),
            material_id: "v".into(),
            target_range: TimeRange::new(0, 2_000_000),
            source_range: TimeRange::new(0, 2_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        p.tracks.push(track);
        let state = AppState::new();
        *state.project.write() = Some(p);
        state
    }

    fn worn(state: &AppState) -> (Option<SpeedEffect>, bool, FrameBlend) {
        state
            .with_project(|p| {
                let (_, s) = p.segment("s").unwrap();
                (
                    speed_effect_of(p, "s"),
                    p.materials.speed_curve_of(s).is_some(),
                    frame_blend_of(&p.materials, s),
                )
            })
            .unwrap()
    }

    #[test]
    fn a_speed_effect_is_a_ramp_and_its_smoothing_in_one_undo_step() {
        let state = state();
        let done = speed_apply_effect(&state, "s".into(), SpeedEffect::Montage).unwrap();
        let edit = done.edit.expect("the first apply edits");
        assert_eq!(edit.undo_label.as_deref(), Some("Smooth montage"));
        assert_eq!(done.job, None, "frame blending bakes nothing");
        assert_eq!(
            worn(&state),
            (Some(SpeedEffect::Montage), true, FrameBlend::Blend)
        );

        // Again: nothing left to do, and not an error.
        let again = speed_apply_effect(&state, "s".into(), SpeedEffect::Montage).unwrap();
        assert!(again.edit.is_none());

        timeline_undo(&state).unwrap();
        assert_eq!(worn(&state), (None, false, FrameBlend::None));
    }

    #[test]
    fn removing_a_speed_effect_takes_the_ramp_and_the_smoothing() {
        let state = state();
        speed_apply_effect(&state, "s".into(), SpeedEffect::FlashOut).unwrap();
        let length_with = state
            .with_project(|p| p.segment("s").unwrap().1.target_range.duration)
            .unwrap();
        assert!(length_with < 2_000_000, "a flash out shortens the clip");
        speed_remove_effect(&state, "s".into()).unwrap();
        assert_eq!(worn(&state), (None, false, FrameBlend::None));
        let length = state
            .with_project(|p| p.segment("s").unwrap().1.target_range.duration)
            .unwrap();
        assert_eq!(length, 2_000_000);
        assert!(speed_remove_effect(&state, "s".into()).is_err());
    }

    #[test]
    fn effects_parse_by_name_and_name_their_smoothing() {
        assert_eq!(SpeedEffect::parse("jump-cut"), Ok(SpeedEffect::JumpCut));
        assert_eq!(SpeedEffect::parse("Bullet"), Ok(SpeedEffect::Bullet));
        assert!(SpeedEffect::parse("warp").is_err());
        let flow: Vec<_> = speed_effects()
            .into_iter()
            .filter(|d| d.smoothing == "flow")
            .map(|d| d.effect)
            .collect();
        assert_eq!(flow, [SpeedEffect::Hero, SpeedEffect::Bullet]);
    }
}
