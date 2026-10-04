//! The shell-facing "Remove background" API: what the app, the CLI and MCP
//! call.
//!
//! `matting_remove_background` is the toggle: one undoable edit that records
//! the setting (model and version) on the clip's compositing material, and,
//! when turning it on, a bake job that fills the matte cache for the clip's
//! source range. The job is polled with `matting_status` and stopped with
//! `matting_cancel`; the preview shows each frame's matte as soon as it is
//! written, and the clip unmatted where none is yet. `matting_ensure` is the
//! export's blocking form: it bakes whatever is missing for the whole
//! project before the first frame is rendered, or fails saying why.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::Serialize;

use super::bake::{self, BakeJob, BakeOutcome, BakeProgress};
use super::cache;
use crate::modules::compositing::commands::compositing_set_background;
use crate::modules::ml::matte::{self, Matter};
use crate::modules::project::compositing::BackgroundRemoval;
use crate::modules::project::document::{Project, Segment};
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

/// What the toggle answers: the edited project, and the bake it started.
#[derive(Serialize)]
pub struct BackgroundResponse {
    #[serde(flatten)]
    pub edit: EditResponse,
    /// The bake job, when the matte had frames to make.
    pub job: Option<u64>,
}

/// A bake's state, as `matting_status` reports it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BakeStatus {
    pub segment_id: String,
    pub progress: BakeProgress,
    /// Set once the job has ended.
    pub finished: Option<Result<BakeOutcome, String>>,
}

/// How much of a clip's matte is baked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub baked: u32,
    pub total: u32,
}

struct Job {
    cancel: AtomicBool,
    status: Mutex<BakeStatus>,
    /// The cache directory it fills, so a second bake of the same frames
    /// joins the running one instead of racing it.
    dir: std::path::PathBuf,
}

fn jobs() -> &'static Mutex<HashMap<u64, Arc<Job>>> {
    static JOBS: OnceLock<Mutex<HashMap<u64, Arc<Job>>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// The bake for `segment` in `project`: its source range of its video file,
/// with the model its setting names. `Err` in words when the clip cannot
/// have its background removed.
pub fn job_for(project: &Project, segment: &Segment) -> Result<BakeJob, String> {
    let video = project
        .materials
        .video(&segment.material_id)
        .ok_or("Remove background works on video clips")?;
    let setting = project
        .materials
        .compositing_of(segment)
        .and_then(|m| m.background.clone())
        .ok_or("the clip does not have Remove background on")?;
    let range = (
        segment.source_range.start,
        (segment.source_range.end() - 1).max(segment.source_range.start),
    );
    Ok(BakeJob {
        path: video.path.clone(),
        fps: video.fps,
        source_size: (video.width, video.height),
        range,
        model: setting.model,
        version: setting.version,
    })
}

/// The setting this build writes when the user turns it on.
pub fn current_model() -> BackgroundRemoval {
    BackgroundRemoval {
        model: matte::MODEL.into(),
        version: matte::model_version().into(),
    }
}

/// Turn "Remove background" on or off for a video clip. On: one undoable
/// edit, then a bake of the clip's matte in the background (its job id is in
/// the answer; `None` when every frame was already baked). Off: the edit
/// only; the baked frames stay in the cache for an undo.
pub fn matting_remove_background(
    state: &Arc<AppState>,
    segment_id: String,
    enabled: bool,
) -> Result<BackgroundResponse, String> {
    state.with_project(|project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        project
            .materials
            .video(&segment.material_id)
            .map(|_| ())
            .ok_or_else(|| "Remove background works on video clips".to_string())
    })??;
    let edit = compositing_set_background(state, segment_id.clone(), enabled.then(current_model))?;
    let job = if enabled {
        matting_bake(state, segment_id)?
    } else {
        None
    };
    Ok(BackgroundResponse { edit, job })
}

/// Bake whatever is missing of `segment_id`'s matte, in the background.
/// `None` when nothing is; a bake already running over the same frames is
/// joined (its id is returned) rather than started twice.
pub fn matting_bake(state: &Arc<AppState>, segment_id: String) -> Result<Option<u64>, String> {
    let job = state.with_project(|project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        job_for(project, segment)
    })??;
    let dir = cache::dir_for(job.path.as_ref(), &job.model, &job.version)?;
    if job.missing(&cache::list(&dir)).is_empty() {
        return Ok(None);
    }
    {
        let jobs = jobs().lock();
        if let Some((id, _)) = jobs.iter().find(|(_, j)| {
            j.dir == dir && j.status.lock().finished.is_none() && !j.cancel.load(Ordering::Relaxed)
        }) {
            // One bake per file at a time: two would write the same frames
            // and share one worker anyway. The second clip's frames are
            // picked up by the next `matting_bake` the app makes when this
            // one ends.
            return Ok(Some(*id));
        }
    }
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    let handle = Arc::new(Job {
        cancel: AtomicBool::new(false),
        status: Mutex::new(BakeStatus {
            segment_id: segment_id.clone(),
            ..BakeStatus::default()
        }),
        dir,
    });
    jobs().lock().insert(id, Arc::clone(&handle));
    std::thread::Builder::new()
        .name("chukcut-matting".into())
        .spawn(move || {
            let result = Matter::prepare(&|_, _| {}, &handle.cancel)
                .map_err(|e| e.to_string())
                .and_then(|_| {
                    bake::run(&job, &handle.cancel, |p| {
                        handle.status.lock().progress = p.clone();
                    })
                });
            match &result {
                Ok(outcome) => tracing::info!(
                    written = outcome.written,
                    seconds = outcome.seconds,
                    provider = ?outcome.provider,
                    "matte baked"
                ),
                Err(error) => tracing::warn!(%error, "background removal failed"),
            }
            handle.status.lock().finished = Some(result);
        })
        .map_err(|e| format!("could not start the matting thread: {e}"))?;
    Ok(Some(id))
}

pub fn matting_status(job: u64) -> Option<BakeStatus> {
    jobs().lock().get(&job).map(|j| j.status.lock().clone())
}

/// Stop a bake. The frames written so far stay.
pub fn matting_cancel(job: u64) {
    if let Some(job) = jobs().lock().get(&job) {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

/// Drop a finished job's record.
pub fn matting_forget(job: u64) {
    jobs().lock().remove(&job);
}

/// How much of `segment_id`'s matte is in the cache.
pub fn matting_coverage(state: &Arc<AppState>, segment_id: String) -> Result<Coverage, String> {
    let job = state.with_project(|project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        job_for(project, segment)
    })??;
    coverage(&job)
}

fn coverage(job: &BakeJob) -> Result<Coverage, String> {
    let dir = cache::dir_for(job.path.as_ref(), &job.model, &job.version)?;
    let total = job.frame_times(job.range.0, job.range.1).len() as u32;
    let missing = job.missing(&cache::list(&dir)).len() as u32;
    Ok(Coverage {
        baked: total.saturating_sub(missing),
        total,
    })
}

/// Bake every missing matte `project` needs, now, on this thread: what an
/// export calls before rendering, so a delivered file never has a
/// background the preview did not. `progress` gets a sentence and a
/// fraction of the whole. Fails in words when a matte cannot be made (no
/// ML worker, a model version this build lacks) — an export must not quietly
/// ship the background the user removed.
pub fn matting_ensure(
    project: &Project,
    progress: &dyn Fn(&str, f32),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut todo = Vec::new();
    for track in &project.tracks {
        for segment in &track.segments {
            let on = project
                .materials
                .compositing_of(segment)
                .is_some_and(|m| m.background.is_some());
            if !on || project.materials.video(&segment.material_id).is_none() {
                continue;
            }
            let job = job_for(project, segment)?;
            let missing = coverage(&job)?;
            if missing.baked < missing.total {
                todo.push((job, missing.total - missing.baked));
            }
        }
    }
    if todo.is_empty() {
        return Ok(());
    }
    progress("Preparing background removal", 0.0);
    Matter::prepare(&|what, _| progress(what, 0.0), cancel).map_err(|e| {
        format!("Remove background is on for a clip, but its matte cannot be made: {e}")
    })?;
    let total: u32 = todo.iter().map(|(_, n)| *n).sum::<u32>().max(1);
    let mut before = 0u32;
    for (job, frames) in &todo {
        let outcome = bake::run(job, cancel, |p| {
            let share = p.done as f32 / p.total.max(1) as f32 * *frames as f32;
            progress(
                "Removing backgrounds",
                ((before as f32 + share) / total as f32).min(1.0),
            );
        })?;
        if outcome.cancelled {
            return Err("cancelled".into());
        }
        before += frames;
    }
    Ok(())
}
