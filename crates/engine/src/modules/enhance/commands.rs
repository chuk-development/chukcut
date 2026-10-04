//! "Remove object" and "Enhance quality": what the app, the CLI and the MCP
//! server call. See the module docs (`enhance`) and decision 0029.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Serialize;

use super::bake::{EnhanceOutcome, EnhanceProgress};
use super::jobs::{self, EnhanceCoverage, EnhanceStatus};
use super::{set_removal_command, set_upscale_command, ObjectRemoval, Stroke, Upscale};
use crate::modules::matting::commands::CanvasPoint;
use crate::modules::project::compositing::{ObjectPrompt, PromptPoint};
use crate::modules::project::document::{Micros, Project, Segment};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::state::AppState;

/// What an edit of the settings did: the edit (none when nothing changed)
/// and the bake it started (none when every frame is made already, or the
/// setting was switched off).
#[derive(Serialize)]
pub struct EnhanceResponse {
    #[serde(flatten)]
    pub edit: Option<EditResponse>,
    pub job: Option<u64>,
}

/// A clip's settings, as the inspector and the CLI show them.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnhanceSettings {
    pub removal: Option<ObjectRemoval>,
    pub upscale: Option<Upscale>,
    /// Why "Enhance quality" cannot be used on this clip, when it cannot.
    pub upscale_refusal: Option<String>,
}

fn with_segment<T>(
    state: &Arc<AppState>,
    segment_id: &str,
    f: impl FnOnce(&Project, &Segment) -> Result<T, String>,
) -> Result<T, String> {
    state.with_project(|project| {
        let (_, segment) = project
            .segment(segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        f(project, segment)
    })?
}

/// A clip's "Remove object" and "Enhance quality".
pub fn enhance_settings(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<EnhanceSettings, String> {
    with_segment(state, &segment_id, |project, segment| {
        let video = project
            .materials
            .video(&segment.material_id)
            .ok_or("remove object and enhance quality work on video clips")?;
        Ok(EnhanceSettings {
            removal: super::removal_of(&project.materials, segment),
            upscale: super::upscale_of(&project.materials, segment),
            upscale_refusal: super::upscale_refusal(video.width, video.height),
        })
    })
}

/// Apply a block edit: the entry into the pool first, out again if the
/// command is refused (the order `audiofx::commands` keeps).
fn apply(
    state: &Arc<AppState>,
    made: impl FnOnce(&Project) -> Result<(Option<(String, serde_json::Value)>, EditCommand), String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (entry, command) = made(project)?;
        if let Some((id, value)) = &entry {
            project.materials.extras.insert(id.clone(), value.clone());
        }
        if let Err(error) = state.history.write().apply(project, command) {
            if let Some((id, _)) = entry {
                project.materials.extras.remove(&id);
            }
            return Err(error);
        }
    }
    crate::modules::voice::commands::respond(state)
}

/// Set a clip's "Remove object" (`None` switches it off) and bake its frames
/// in the background. The same setting again bakes what is missing. One
/// undo step.
pub fn enhance_set_removal(
    state: &Arc<AppState>,
    segment_id: String,
    removal: Option<ObjectRemoval>,
) -> Result<EnhanceResponse, String> {
    let current = with_segment(state, &segment_id, |project, segment| {
        Ok(super::removal_of(&project.materials, segment))
    })?;
    let edit = if current == removal {
        None
    } else {
        Some(apply(state, |project| {
            set_removal_command(project, &segment_id, removal.as_ref())
        })?)
    };
    finish(state, segment_id, edit)
}

/// The bake after an edit: started (or joined) when the clip still has
/// something to remake.
fn finish(
    state: &Arc<AppState>,
    segment_id: String,
    edit: Option<EditResponse>,
) -> Result<EnhanceResponse, String> {
    let on = with_segment(state, &segment_id, |project, segment| {
        Ok(super::Chain::of(&project.materials, segment).is_some())
    })?;
    let job = if on {
        enhance_bake(state, segment_id)?
    } else {
        None
    };
    Ok(EnhanceResponse { edit, job })
}

/// The removal a clip has, or a fresh one with this build's model.
fn removal_or_new(state: &Arc<AppState>, segment_id: &str) -> Result<ObjectRemoval, String> {
    with_segment(state, segment_id, |project, segment| {
        Ok(super::removal_of(&project.materials, segment).unwrap_or_default())
    })
}

/// Canvas fractions (0..1, y down) at timeline `time` as fractions of the
/// clip's source picture.
fn to_source(
    project: &Project,
    segment: &Segment,
    time: Micros,
    x: f32,
    y: f32,
) -> Result<[f32; 2], String> {
    crate::modules::tracking::follow::canvas_to_source(
        project,
        segment,
        time,
        [x * 2.0 - 1.0, 1.0 - y * 2.0],
    )
    .ok_or_else(|| "the clip is not visible on the canvas at the playhead".to_string())
}

/// The clicks `points` (canvas fractions, y down, on the frame at timeline
/// `time`) as a selection on the clip's source: what "Remove object ›
/// Select" records. Fails in words when the playhead is off the clip or a
/// click misses its picture.
pub fn enhance_prompt_at(
    state: &Arc<AppState>,
    segment_id: String,
    time: Micros,
    points: Vec<CanvasPoint>,
) -> Result<ObjectPrompt, String> {
    if !points.iter().any(|p| p.keep) {
        return Err("click at least once on the object to remove".into());
    }
    with_segment(state, &segment_id, |project, segment| {
        if !segment.target_range.contains(time) {
            return Err(
                "the playhead is not on the clip; move it onto the clip and click again".into(),
            );
        }
        let source_time = project
            .materials
            .time_map(segment)
            .clamped_source_time(time);
        let mut prompt = ObjectPrompt {
            time: source_time,
            points: Vec::new(),
        };
        for p in &points {
            let at = to_source(project, segment, time, p.x, p.y)?;
            if !(0.0..=1.0).contains(&at[0]) || !(0.0..=1.0).contains(&at[1]) {
                return Err("a click is outside the clip's picture".to_string());
            }
            prompt.points.push(PromptPoint {
                x: at[0],
                y: at[1],
                keep: p.keep,
            });
        }
        Ok(prompt)
    })
}

/// "Remove object › Select": the object under `points` (canvas fractions,
/// on the frame at timeline `time`) is removed from the whole clip. It is
/// followed over the clip like "Select object" follows it. Strokes and
/// boxes the clip has stay. One undo step; bakes in the background.
pub fn enhance_select_object(
    state: &Arc<AppState>,
    segment_id: String,
    time: Micros,
    points: Vec<CanvasPoint>,
) -> Result<EnhanceResponse, String> {
    let prompt = enhance_prompt_at(state, segment_id.clone(), time, points)?;
    let removal = ObjectRemoval {
        prompt: Some(prompt),
        ..removal_or_new(state, &segment_id)?
    };
    enhance_set_removal(state, segment_id, Some(removal))
}

/// "Remove object › Paint": one stroke through `points` (canvas fractions,
/// y down, on the frame at timeline `time`), `radius` wide on each side (a
/// fraction of the canvas's shorter side), added to what the clip removes.
/// The stroke covers the same part of the picture in every frame. One undo
/// step; bakes in the background.
pub fn enhance_paint(
    state: &Arc<AppState>,
    segment_id: String,
    time: Micros,
    points: Vec<[f32; 2]>,
    radius: f32,
) -> Result<EnhanceResponse, String> {
    if points.is_empty() {
        return Err("a stroke needs at least one point".into());
    }
    let stroke = with_segment(state, &segment_id, |project, segment| {
        let video = project
            .materials
            .video(&segment.material_id)
            .ok_or("remove object works on video clips")?;
        let mut source = Vec::with_capacity(points.len());
        for p in &points {
            let at = to_source(project, segment, time, p[0], p[1])?;
            source.push([at[0].clamp(0.0, 1.0), at[1].clamp(0.0, 1.0)]);
        }
        // The brush's width on the canvas, measured in the source: the
        // first point and one a radius to its right, both mapped.
        let (cw, ch) = (
            project.canvas.width.max(1) as f32,
            project.canvas.height.max(1) as f32,
        );
        let dx = radius * cw.min(ch) / cw;
        let a = to_source(project, segment, time, points[0][0], points[0][1])?;
        let b = to_source(project, segment, time, points[0][0] + dx, points[0][1])?;
        let (sw, sh) = (video.width.max(1) as f32, video.height.max(1) as f32);
        let px = ((b[0] - a[0]) * sw).hypot((b[1] - a[1]) * sh);
        Ok(Stroke {
            points: source,
            radius: (px / sw.min(sh)).clamp(0.001, 0.5),
        })
    })?;
    let mut removal = removal_or_new(state, &segment_id)?;
    removal.strokes.push(stroke);
    enhance_set_removal(state, segment_id, Some(removal))
}

/// Set a clip's "Enhance quality" to `scale` (2 or 4; `None` switches it
/// off) and bake its frames in the background. One undo step.
pub fn enhance_set_upscale(
    state: &Arc<AppState>,
    segment_id: String,
    scale: Option<u32>,
) -> Result<EnhanceResponse, String> {
    let upscale = scale.map(Upscale::new).transpose()?;
    let current = with_segment(state, &segment_id, |project, segment| {
        Ok(super::upscale_of(&project.materials, segment))
    })?;
    let edit = if current == upscale {
        None
    } else {
        Some(apply(state, |project| {
            set_upscale_command(project, &segment_id, upscale.as_ref())
        })?)
    };
    finish(state, segment_id, edit)
}

/// Bake whatever is missing of a clip's remade frames, in the background;
/// the preview shows them as they land and the decoded frames until then.
/// `None` when nothing is missing. A bake of the same frames that runs
/// already is joined.
pub fn enhance_bake(state: &Arc<AppState>, segment_id: String) -> Result<Option<u64>, String> {
    jobs::bake(state, segment_id)
}

/// A bake's progress, its CPU time warning, and how it ended.
pub fn enhance_status(job: u64) -> Option<EnhanceStatus> {
    jobs::status(job)
}

/// Wait for bake `job` to end. For a one-shot CLI run, whose process would
/// otherwise end before its bake; `tick` sees the status every 100 ms.
pub fn enhance_wait(job: u64, tick: impl FnMut(&EnhanceStatus)) -> Result<EnhanceOutcome, String> {
    jobs::wait(job, tick)
}

/// Stop a bake; the frames made so far stay.
pub fn enhance_cancel(job: u64) {
    jobs::cancel(job)
}

/// How many of a clip's frames are made.
pub fn enhance_coverage(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<EnhanceCoverage, String> {
    jobs::segment_coverage(state, &segment_id)
}

/// The bakes running now, for the app's status line: (job, segment id,
/// progress, CPU time warning).
pub fn enhance_running() -> Vec<(u64, String, EnhanceProgress, Option<String>)> {
    jobs::running()
}

/// Start a bake for every clip whose remade frames are missing after an
/// edit (a trim, an undo, a cleaned cache). Reads files: call it off the UI
/// thread.
pub fn enhance_queue_missing(state: &Arc<AppState>) -> Result<Vec<u64>, String> {
    jobs::queue_missing(state)
}

/// Bake every remade frame an export of `project` needs, on this thread,
/// before the first frame renders.
pub fn enhance_ensure(
    project: &Project,
    progress: &dyn Fn(&str, f32),
    cancel: &AtomicBool,
) -> Result<(), String> {
    jobs::ensure(project, progress, cancel)
}

/// What a clip's bake will take, before it runs: the frames, and the
/// time on a GPU and on the CPU at the speeds measured on the reference
/// machines (`docs/STATUS.md`, "Remove object and enhance quality").
#[derive(Debug, Clone, Serialize)]
pub struct EnhanceEstimate {
    pub frames: u32,
    pub missing: u32,
    pub seconds_gpu: f64,
    pub seconds_cpu: f64,
}

/// Estimate a clip's bake. Reads the media file's head and the cache.
pub fn enhance_estimate(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<EnhanceEstimate, String> {
    let job = with_segment(state, &segment_id, jobs::job_for)?;
    let frames = job.frame_times(job.range.0, job.range.1).len() as u32;
    let missing = jobs::coverage(&job).map(|c| c.total - c.baked)?;
    let (gpu, cpu) = super::seconds_per_frame(&job.chain, job.source_size);
    Ok(EnhanceEstimate {
        frames,
        missing,
        seconds_gpu: gpu * missing as f64,
        seconds_cpu: cpu * missing as f64,
    })
}

/// The size of the cache of remade frames, for Settings.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnhanceCacheInfo {
    pub bytes: u64,
    /// Directories: one per clip source, chain, provider and size.
    pub sets: u32,
}

/// The size of the remade-frame cache. Walks it: call it off the UI thread.
pub fn enhance_cache_info() -> EnhanceCacheInfo {
    let root = super::cache::root();
    let sets = std::fs::read_dir(&root)
        .map(|entries| entries.flatten().filter(|e| e.path().is_dir()).count() as u32)
        .unwrap_or(0);
    EnhanceCacheInfo {
        bytes: crate::modules::ml::download::size_of(&root),
        sets,
    }
}

/// Delete every remade frame. Running bakes are stopped first; a clip with
/// the setting on bakes again when it is next shown or exported.
pub fn enhance_cache_clear() -> Result<(), String> {
    jobs::cancel_all();
    let root = super::cache::root();
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .map_err(|e| format!("could not delete {}: {e}", root.display()))?;
    }
    Ok(())
}
