//! Background bakes of remade frames, the way optical-flow frames run
//! theirs (`speed::flow::jobs`): one job per (file, chain) at a time,
//! polled by id, cancelled by id, remembered when it failed so an edit does
//! not start it again and again. The shell-facing names are in
//! [`super::commands`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::Serialize;

use super::bake::{self, EnhanceJob, EnhanceOutcome, EnhanceProgress};
use super::Chain;
use crate::modules::project::document::{Project, Segment, Track};
use crate::state::AppState;

/// A bake's state, as `enhance_status` reports it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnhanceStatus {
    pub segment_id: String,
    pub progress: EnhanceProgress,
    /// The CPU's time warning while the bake runs there.
    pub warning: Option<String>,
    /// Set once the job has ended.
    pub finished: Option<Result<EnhanceOutcome, String>>,
}

/// How many of a clip's frames are made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct EnhanceCoverage {
    pub baked: u32,
    pub total: u32,
}

struct Job {
    cancel: AtomicBool,
    status: Mutex<EnhanceStatus>,
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

/// The bake of `segment`: its source range of its video file, with its
/// chain. `Err` in words when the clip has nothing to remake.
pub fn job_for(project: &Project, segment: &Segment) -> Result<EnhanceJob, String> {
    let video = project
        .materials
        .video(&segment.material_id)
        .ok_or("remove object and enhance quality work on video clips")?;
    let chain = Chain::of(&project.materials, segment)
        .ok_or("the clip has neither Remove object nor Enhance quality on")?;
    Ok(EnhanceJob {
        path: video.path.clone(),
        fps: video.fps,
        source_size: (video.width, video.height),
        range: (
            segment.source_range.start,
            (segment.source_range.end() - 1).max(segment.source_range.start),
        ),
        chain,
    })
}

/// How much of `job` is in the cache.
pub fn coverage(job: &EnhanceJob) -> Result<EnhanceCoverage, String> {
    let times = job.best()?.map(|(_, t)| t).unwrap_or_default();
    let total = job.frame_times(job.range.0, job.range.1).len() as u32;
    let missing = job.missing(&times).len() as u32;
    Ok(EnhanceCoverage {
        baked: total.saturating_sub(missing),
        total,
    })
}

fn segment_job(state: &Arc<AppState>, segment_id: &str) -> Result<EnhanceJob, String> {
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
    if let Some(why) = job.refusal() {
        return Err(why);
    }
    start(job, segment_id)
}

pub(crate) fn start(job: EnhanceJob, segment_id: String) -> Result<Option<u64>, String> {
    let key = job.key()?;
    let cover = coverage(&job)?;
    if cover.baked >= cover.total {
        return Ok(None);
    }
    {
        let jobs = jobs().lock();
        if let Some((id, _)) = jobs.iter().find(|(_, j)| {
            j.key == key && j.status.lock().finished.is_none() && !j.cancel.load(Ordering::Relaxed)
        }) {
            // One bake per file and chain at a time; a second clip of the
            // same file is picked up by the next re-bake when it ends.
            return Ok(Some(*id));
        }
    }
    failed().lock().remove(&key);
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    let handle = Arc::new(Job {
        cancel: AtomicBool::new(false),
        status: Mutex::new(EnhanceStatus {
            segment_id,
            ..EnhanceStatus::default()
        }),
        key,
    });
    jobs().lock().insert(id, Arc::clone(&handle));
    std::thread::Builder::new()
        .name("chukcut-enhance".into())
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
                    "remade frames baked"
                ),
                Err(error) => {
                    tracing::warn!(%error, "remaking frames failed");
                    failed().lock().insert(handle.key.clone(), error.clone());
                }
            }
            let mut status = handle.status.lock();
            status.warning = None;
            status.finished = Some(result);
        })
        .map_err(|e| format!("could not start the enhance thread: {e}"))?;
    Ok(Some(id))
}

/// Whether a bake of `key`'s frames (`EnhanceJob::key`) is running: an
/// optical-flow bake of a remade clip waits for it rather than making the
/// same frames twice.
pub fn busy(key: &str) -> bool {
    jobs().lock().values().any(|j| {
        j.key == key && j.status.lock().finished.is_none() && !j.cancel.load(Ordering::Relaxed)
    })
}

pub fn status(job: u64) -> Option<EnhanceStatus> {
    jobs().lock().get(&job).map(|j| j.status.lock().clone())
}

/// Wait for `job` to end and answer how it ended. For a one-shot CLI run.
pub fn wait(job: u64, mut tick: impl FnMut(&EnhanceStatus)) -> Result<EnhanceOutcome, String> {
    loop {
        let Some(now) = status(job) else {
            return Err(format!("no enhance job {job}"));
        };
        tick(&now);
        if let Some(result) = now.finished {
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
pub fn running() -> Vec<(u64, String, EnhanceProgress, Option<String>)> {
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

pub fn segment_coverage(
    state: &Arc<AppState>,
    segment_id: &str,
) -> Result<EnhanceCoverage, String> {
    coverage(&segment_job(state, segment_id)?)
}

/// Every lane the export renders: the timeline and the inside of compound
/// clips.
fn lanes(project: &Project) -> impl Iterator<Item = &Track> {
    let compounds = project
        .materials
        .sequences
        .iter()
        .filter(|s| s.kind == crate::modules::sequence::SequenceKind::Compound)
        .flat_map(|s| &s.tracks);
    project.tracks.iter().chain(compounds)
}

/// Bake every frame an export of `project` needs, now, on this thread.
/// Fails in words when they cannot be made: a file that shows the object
/// the user removed is not what they asked for.
pub fn ensure(
    project: &Project,
    progress: &dyn Fn(&str, f32),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut todo: Vec<(EnhanceJob, u32)> = Vec::new();
    for track in lanes(project) {
        for segment in &track.segments {
            if Chain::of(&project.materials, segment).is_none() {
                continue;
            }
            let job = job_for(project, segment)?;
            let cover = coverage(&job)?;
            if cover.baked < cover.total {
                todo.push((job, cover.total - cover.baked));
            }
        }
    }
    if todo.is_empty() {
        return Ok(());
    }
    progress("Preparing the AI frames", 0.0);
    let total: u32 = todo.iter().map(|(_, n)| *n).sum::<u32>().max(1);
    let mut before = 0u32;
    for (job, frames) in &todo {
        let outcome = bake::run(job, cancel, |p| {
            let share = p.done as f32 / p.total.max(1) as f32 * *frames as f32;
            let what = match (p.cpu_warning(), p.stage.as_str()) {
                (_, "") => "Making the AI frames".to_string(),
                (Some(_), stage) => format!("{stage} on the CPU"),
                (None, stage) => stage.to_string(),
            };
            progress(&what, ((before as f32 + share) / total as f32).min(1.0));
        })
        .map_err(|e| {
            format!(
                "Remove object or Enhance quality is on for a clip, but its frames cannot be \
                 made: {e}. Switch it off to export without it"
            )
        })?;
        if outcome.cancelled {
            return Err("cancelled".into());
        }
        before += frames;
    }
    Ok(())
}

/// Start a bake for every clip of the open project whose frames are
/// missing (a trim, an undo, a cleaned cache). Reads media heads and lists
/// the cache: call it off the UI thread.
pub fn queue_missing(state: &Arc<AppState>) -> Result<Vec<u64>, String> {
    let wanted: Vec<(String, EnhanceJob)> = state.with_project(|project| {
        lanes(project)
            .flat_map(|t| &t.segments)
            .filter_map(|s| Some((s.id.clone(), job_for(project, s).ok()?)))
            .collect()
    })?;
    let mut started = Vec::new();
    for (segment_id, job) in wanted {
        if job.refusal().is_some() {
            continue;
        }
        let Ok(key) = job.key() else { continue };
        if failed().lock().contains_key(&key) {
            continue;
        }
        match start(job, segment_id) {
            Ok(Some(job)) => started.push(job),
            Ok(None) => {}
            Err(error) => tracing::debug!(%error, "no enhance bake"),
        }
    }
    started.sort_unstable();
    started.dedup();
    Ok(started)
}

/// Stop every running bake (before the cache is cleared).
pub fn cancel_all() {
    for job in jobs().lock().values() {
        job.cancel.store(true, Ordering::Relaxed);
    }
}
