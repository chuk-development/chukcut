//! Background bakes of optical-flow frames, the way `matting::commands` runs
//! its mattes: one job per media file at a time, polled by id, cancelled by
//! id, remembered when it failed so an edit does not start it again and
//! again. The shell-facing names are in `speed::commands`.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::Serialize;

use super::bake::{self, FlowJob, FlowOutcome, FlowProgress};
use super::FlowSample;
use crate::modules::project::document::{Micros, Project, Segment, Track};
use crate::state::AppState;

/// A bake's state, as `speed_flow_status` reports it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FlowStatus {
    pub segment_id: String,
    pub progress: FlowProgress,
    /// The CPU's time warning while the bake runs there.
    pub warning: Option<String>,
    /// Set once the job has ended.
    pub finished: Option<Result<FlowOutcome, String>>,
}

/// How many of a clip's in-between frames are baked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct FlowCoverage {
    pub baked: u32,
    pub total: u32,
}

struct Job {
    cancel: AtomicBool,
    status: Mutex<FlowStatus>,
    key: String,
}

fn jobs() -> &'static Mutex<HashMap<u64, Arc<Job>>> {
    static JOBS: OnceLock<Mutex<HashMap<u64, Arc<Job>>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

/// Keys whose last bake failed or was stopped, with why: the automatic
/// re-bake after an edit leaves them alone; an explicit bake tries again.
fn failed() -> &'static Mutex<HashMap<String, String>> {
    static FAILED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    FAILED.get_or_init(Default::default)
}

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// The bake of `segment`'s in-between frames at timeline instants `times`.
pub fn job_at(
    project: &Project,
    segment: &Segment,
    times: impl IntoIterator<Item = Micros>,
) -> Result<FlowJob, String> {
    let video = project
        .materials
        .video(&segment.material_id)
        .ok_or("optical flow works on video clips")?;
    if !super::is_on(&project.materials, segment) {
        return Err("the clip does not have optical flow on".into());
    }
    Ok(FlowJob {
        path: video.path.clone(),
        fps: video.fps,
        source_size: (video.width, video.height),
        samples: super::samples_at(&project.materials, segment, times),
    })
}

/// The bake of what the preview plays of `segment`.
pub fn job_for(project: &Project, segment: &Segment) -> Result<FlowJob, String> {
    job_at(project, segment, super::preview_times(project, segment))
}

/// How much of `job` is in the cache.
pub fn coverage(job: &FlowJob) -> Result<FlowCoverage, String> {
    let present = job.best()?.map(|(_, s)| s).unwrap_or_default();
    let total = job.samples.len() as u32;
    let missing = job.missing(&present).len() as u32;
    Ok(FlowCoverage {
        baked: total - missing,
        total,
    })
}

fn segment_job(state: &Arc<AppState>, segment_id: &str) -> Result<FlowJob, String> {
    state.with_project(|project| {
        let (_, segment) = project
            .segment(segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        job_for(project, segment)
    })?
}

/// Start (or join) the bake of `segment_id`'s missing frames; `None` when
/// none are missing.
pub fn bake(state: &Arc<AppState>, segment_id: String) -> Result<Option<u64>, String> {
    let job = segment_job(state, &segment_id)?;
    start(job, segment_id)
}

fn start(job: FlowJob, segment_id: String) -> Result<Option<u64>, String> {
    let key = job.key()?;
    if job.samples.is_empty() || coverage(&job)?.baked >= job.samples.len() as u32 {
        return Ok(None);
    }
    {
        let jobs = jobs().lock();
        if let Some((id, _)) = jobs.iter().find(|(_, j)| {
            j.key == key && j.status.lock().finished.is_none() && !j.cancel.load(Ordering::Relaxed)
        }) {
            // One bake per file at a time; the frames of a second clip of
            // the same file are picked up by the next re-bake when it ends.
            return Ok(Some(*id));
        }
    }
    failed().lock().remove(&key);
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    let handle = Arc::new(Job {
        cancel: AtomicBool::new(false),
        status: Mutex::new(FlowStatus {
            segment_id,
            ..FlowStatus::default()
        }),
        key,
    });
    jobs().lock().insert(id, Arc::clone(&handle));
    std::thread::Builder::new()
        .name("chukcut-flow".into())
        .spawn(move || {
            let result = bake::run(&job, &handle.cancel, |p| {
                let mut status = handle.status.lock();
                status.progress = p.clone();
                status.warning = p.cpu_warning();
            });
            match &result {
                Ok(outcome) => tracing::info!(
                    written = outcome.written,
                    seconds = outcome.seconds,
                    provider = ?outcome.provider,
                    model_ms = ?outcome.model_millis_per_frame,
                    "optical-flow frames baked"
                ),
                Err(error) => {
                    tracing::warn!(%error, "optical flow failed");
                    failed().lock().insert(handle.key.clone(), error.clone());
                }
            }
            let mut status = handle.status.lock();
            status.warning = None;
            status.finished = Some(result);
        })
        .map_err(|e| format!("could not start the optical-flow thread: {e}"))?;
    Ok(Some(id))
}

pub fn status(job: u64) -> Option<FlowStatus> {
    jobs().lock().get(&job).map(|j| j.status.lock().clone())
}

/// Wait for `job` to end and answer how it ended. For a one-shot CLI run,
/// whose process would otherwise end before the bake.
pub fn wait(job: u64, mut tick: impl FnMut(&FlowStatus)) -> Result<FlowOutcome, String> {
    loop {
        let Some(now) = status(job) else {
            return Err(format!("no optical-flow job {job}"));
        };
        tick(&now);
        if let Some(result) = now.finished {
            // The waiter owns the job's record; nothing polls it after this.
            jobs().lock().remove(&job);
            return result;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Stop a bake; the frames written so far stay.
pub fn cancel(job: u64) {
    if let Some(job) = jobs().lock().get(&job) {
        job.cancel.store(true, Ordering::Relaxed);
        failed().lock().insert(job.key.clone(), "stopped".into());
    }
}

/// The bakes running now: (job, segment id, progress, CPU warning).
pub fn running() -> Vec<(u64, String, FlowProgress, Option<String>)> {
    let mut running: Vec<_> = jobs()
        .lock()
        .iter()
        .filter_map(|(id, j)| {
            let status = j.status.lock();
            status.finished.is_none().then(|| {
                (
                    *id,
                    status.segment_id.clone(),
                    status.progress.clone(),
                    status.warning.clone(),
                )
            })
        })
        .collect();
    running.sort_by_key(|(id, ..)| *id);
    running
}

pub fn segment_coverage(state: &Arc<AppState>, segment_id: &str) -> Result<FlowCoverage, String> {
    coverage(&segment_job(state, segment_id)?)
}

/// Every lane the export renders: the timeline and the inside of compound
/// clips (whose inner times are the outer ones mapped through the compound
/// clip, which `samples_at` does not follow; their frames come from the
/// preview's grid instead).
fn lanes(project: &Project) -> impl Iterator<Item = (&Track, bool)> {
    let compounds = project
        .materials
        .sequences
        .iter()
        .filter(|s| s.kind == crate::modules::sequence::SequenceKind::Compound)
        .flat_map(|s| &s.tracks);
    project
        .tracks
        .iter()
        .map(|t| (t, false))
        .chain(compounds.map(|t| (t, true)))
}

/// Bake every in-between frame an export of `project` at timeline instants
/// `times` needs, now, on this thread. Fails in words when they cannot be
/// made: a file that shows the plain mix where the user chose optical flow
/// is not what they asked for.
pub fn ensure(
    project: &Project,
    times: &[Micros],
    progress: &dyn Fn(&str, f32),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut todo: Vec<(FlowJob, u32)> = Vec::new();
    for (track, nested) in lanes(project) {
        for segment in &track.segments {
            if !super::is_on(&project.materials, segment) {
                continue;
            }
            let job = if nested {
                job_for(project, segment)?
            } else {
                job_at(project, segment, times.iter().copied())?
            };
            let present = job.best()?.map(|(_, s)| s).unwrap_or_default();
            let missing = job.missing(&present).len() as u32;
            if missing > 0 {
                todo.push((job, missing));
            }
        }
    }
    if todo.is_empty() {
        return Ok(());
    }
    progress("Preparing optical flow", 0.0);
    let total: u32 = todo.iter().map(|(_, n)| *n).sum::<u32>().max(1);
    let mut before = 0u32;
    for (job, frames) in &todo {
        let outcome = bake::run(job, cancel, |p| {
            let share = p.done as f32 / p.total.max(1) as f32 * *frames as f32;
            let what = match p.cpu_warning() {
                Some(_) => "Making slow-motion frames on the CPU",
                None => "Making slow-motion frames",
            };
            progress(what, ((before as f32 + share) / total as f32).min(1.0));
        })
        .map_err(|e| {
            format!(
                "Optical flow is on for a clip, but its frames cannot be made: {e}. \
                 Switch the clip to Frame blend to export without it"
            )
        })?;
        if outcome.cancelled {
            return Err("cancelled".into());
        }
        before += frames;
    }
    Ok(())
}

/// Start a bake for every clip of the open project whose optical-flow frames
/// are missing (a speed change, a trim, an undo, a cleaned cache). Reads
/// media heads and lists the cache: call it off the UI thread.
pub fn queue_missing(state: &Arc<AppState>) -> Result<Vec<u64>, String> {
    let wanted: Vec<(String, FlowJob)> = state.with_project(|project| {
        lanes(project)
            .flat_map(|(t, _)| &t.segments)
            .filter(|s| super::is_on(&project.materials, s))
            .filter_map(|s| Some((s.id.clone(), job_for(project, s).ok()?)))
            .collect()
    })?;
    let mut started = Vec::new();
    for (segment_id, job) in wanted {
        let Ok(key) = job.key() else { continue };
        if failed().lock().contains_key(&key) {
            continue;
        }
        match start(job, segment_id) {
            Ok(Some(job)) => started.push(job),
            Ok(None) => {}
            Err(error) => tracing::debug!(%error, "no optical-flow bake"),
        }
    }
    started.sort_unstable();
    started.dedup();
    Ok(started)
}

/// The samples of `job` present in its best directory, for tests and the
/// CLI's report.
pub fn present(job: &FlowJob) -> BTreeSet<FlowSample> {
    job.best()
        .ok()
        .flatten()
        .map(|(_, s)| s)
        .unwrap_or_default()
}
