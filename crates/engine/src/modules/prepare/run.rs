//! The preparation's thread: scan the cache, then bake one clip at a time.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::commands::PrepareStatus;
use crate::modules::enhance::bake::EnhanceJob;
use crate::modules::matting::bake::BakeJob;
use crate::modules::project::Project;
use crate::modules::speed::flow::bake::FlowJob;
use crate::state::AppState;

/// How often a running bake's progress is read.
const POLL: Duration = Duration::from_millis(200);

/// One preparation, for one opened project.
pub(super) struct Run {
    pub cancel: AtomicBool,
    pub status: Mutex<PrepareStatus>,
    /// The module bake running now, so Stop can cancel it.
    current: Mutex<Option<Bake>>,
}

/// A module's background bake, by its job number.
#[derive(Debug, Clone, Copy)]
enum Bake {
    Matte(u64),
    Flow(u64),
    Enhance(u64),
    /// An analysis job (`analysis::jobs`): face landmarks.
    Analysis(u64),
}

/// What one clip is missing.
pub(super) enum Item {
    /// A compound clip's mix-down, by its sequence.
    Sound { sequence_id: String },
    Matte {
        job: BakeJob,
        segment_id: String,
        missing: u32,
    },
    Flow {
        job: FlowJob,
        segment_id: String,
        missing: u32,
    },
    Enhance {
        job: EnhanceJob,
        segment_id: String,
        missing: u32,
    },
    /// Face landmarks a retouch effect reads.
    Landmarks { segment_id: String, missing: u32 },
    /// A clip's isolated voice.
    Voice { segment_id: String },
}

fn slot() -> &'static Mutex<Option<Arc<Run>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<Run>>>> = OnceLock::new();
    SLOT.get_or_init(Default::default)
}

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

pub(super) fn current() -> Option<Arc<Run>> {
    slot().lock().clone()
}

/// Cancel the module bake `run` follows, if one runs.
pub(super) fn cancel_current(run: &Run) {
    match *run.current.lock() {
        Some(Bake::Matte(job)) => crate::modules::matting::commands::matting_cancel(job),
        Some(Bake::Flow(job)) => crate::modules::speed::flow::jobs::cancel(job),
        Some(Bake::Enhance(job)) => crate::modules::enhance::jobs::cancel(job),
        Some(Bake::Analysis(job)) => crate::modules::analysis::jobs::cancel(job),
        None => {}
    }
}

pub(super) fn start(state: Arc<AppState>, grace: Duration) -> u64 {
    let id = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    let run = Arc::new(Run {
        cancel: AtomicBool::new(false),
        status: Mutex::new(PrepareStatus {
            run: id,
            ..PrepareStatus::default()
        }),
        current: Mutex::new(None),
    });
    if let Some(old) = slot().lock().replace(Arc::clone(&run)) {
        // The project it prepared is no longer the open one.
        old.cancel.store(true, Ordering::Relaxed);
        cancel_current(&old);
    }
    let thread = Arc::clone(&run);
    let spawned = std::thread::Builder::new()
        .name("chukcut-prepare".into())
        .spawn(move || {
            work(&state, &thread, grace);
            thread.status.lock().finished = true;
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start preparing the project");
        run.status.lock().finished = true;
    }
    id
}

/// The open project's id, or `None` when none is open.
fn open_id(state: &AppState) -> Option<String> {
    state.project.read().as_ref().map(|p| p.id.clone())
}

/// Whether the run should go on: not stopped, and its project still open.
fn going(state: &AppState, run: &Run, project_id: &str) -> bool {
    !run.cancel.load(Ordering::Relaxed) && open_id(state).as_deref() == Some(project_id)
}

fn work(state: &Arc<AppState>, run: &Run, grace: Duration) {
    let Some(project_id) = open_id(state) else {
        return;
    };
    let until = Instant::now() + grace;
    while Instant::now() < until {
        if !going(state, run, &project_id) {
            return;
        }
        std::thread::sleep(POLL.min(until.saturating_duration_since(Instant::now())));
    }
    let Some(project) = state.project.read().clone() else {
        return;
    };
    let items = scan(&project);
    {
        let mut status = run.status.lock();
        count(&items, &mut status);
        status.scanned = true;
    }
    if !items.is_empty() {
        tracing::info!(clips = items.len(), "preparing the opened project");
    }
    for item in items {
        if !going(state, run, &project_id) {
            break;
        }
        execute(state, run, &project, &project_id, item);
    }
    run.status.lock().stage = None;
}

/// Add what `items` stand for to `status`'s totals.
pub(super) fn count(items: &[Item], status: &mut PrepareStatus) {
    for item in items {
        match item {
            Item::Sound { .. } => status.sounds += 1,
            Item::Matte { missing, .. }
            | Item::Flow { missing, .. }
            | Item::Enhance { missing, .. }
            | Item::Landmarks { missing, .. } => status.frames += missing,
            Item::Voice { .. } => status.voices += 1,
        }
    }
}

/// Every lane of every sequence: the open one, the other timelines and the
/// compound clips' contents.
fn lanes(project: &Project) -> impl Iterator<Item = &crate::modules::project::Track> {
    crate::modules::sequence::all_tracks(project)
}

/// What `project`'s clips are missing, sound first (cheapest), then mattes,
/// flow frames, remade frames, face landmarks and isolated voices. Reads
/// the cache's directories.
pub(super) fn scan(project: &Project) -> Vec<Item> {
    use crate::modules::{enhance, matting, sequence, speed};
    let pool = &project.materials;
    let segments: Vec<&crate::modules::project::Segment> =
        lanes(project).flat_map(|t| t.segments.iter()).collect();
    let mut items = Vec::new();

    let mut sounds: Vec<String> = Vec::new();
    for s in &segments {
        if pool.sequence(&s.material_id).is_none()
            || sounds.contains(&s.material_id)
            || !sequence::bounce::processes_sound(project, s)
        {
            continue;
        }
        let missing = sequence::bounce::source_of(project, &s.material_id)
            .is_some_and(|(path, _)| !path.is_file());
        if missing {
            sounds.push(s.material_id.clone());
        }
    }
    items.extend(
        sounds
            .into_iter()
            .map(|sequence_id| Item::Sound { sequence_id }),
    );

    for s in &segments {
        let Ok(job) = matting::commands::job_for(project, s) else {
            continue;
        };
        if job.kind().is_err() {
            continue;
        }
        if let Ok(c) = matting::commands::coverage(&job) {
            if c.baked < c.total {
                items.push(Item::Matte {
                    job,
                    segment_id: s.id.clone(),
                    missing: c.total - c.baked,
                });
            }
        }
    }
    for s in &segments {
        if !speed::flow::is_on(pool, s) {
            continue;
        }
        let Ok(job) = speed::flow::jobs::job_for(project, s) else {
            continue;
        };
        if let Ok(c) = speed::flow::jobs::coverage(&job) {
            if c.baked < c.total {
                items.push(Item::Flow {
                    job,
                    segment_id: s.id.clone(),
                    missing: c.total - c.baked,
                });
            }
        }
    }
    for s in &segments {
        let Ok(job) = enhance::jobs::job_for(project, s) else {
            continue;
        };
        if job.refusal().is_some() {
            continue;
        }
        if let Ok(c) = enhance::jobs::coverage(&job) {
            if c.baked < c.total {
                items.push(Item::Enhance {
                    job,
                    segment_id: s.id.clone(),
                    missing: c.total - c.baked,
                });
            }
        }
    }
    for (segment_id, _, missing) in crate::modules::landmarks::commands::missing(project) {
        items.push(Item::Landmarks {
            segment_id,
            missing: missing as u32,
        });
    }
    for segment_id in crate::modules::voice::commands::isolation_missing(project) {
        items.push(Item::Voice { segment_id });
    }
    items
}

fn execute(state: &Arc<AppState>, run: &Run, project: &Project, project_id: &str, item: Item) {
    use crate::modules::{enhance, matting, speed};
    let set_stage = |stage: &str| run.status.lock().stage = Some(stage.to_string());
    match item {
        Item::Sound { sequence_id } => {
            set_stage("Mixing down compound clips");
            match crate::modules::sequence::bounce::render(project, &sequence_id, &run.cancel) {
                Ok(_) => {
                    run.status.lock().sounds_done += 1;
                    crate::modules::audiofx::cache::notify_landed();
                }
                Err(error) if !run.cancel.load(Ordering::Relaxed) => {
                    fail(run, "A compound clip's sound", &error)
                }
                Err(_) => {}
            }
        }
        Item::Matte {
            job,
            segment_id,
            missing,
        } => {
            set_stage("Removing backgrounds");
            let started = matting::commands::start_job(job, segment_id);
            follow(
                state,
                run,
                project_id,
                missing,
                "Remove background",
                started,
                |id| {
                    let status = matting::commands::matting_status(id)?;
                    let finished = status.finished.map(|r| r.map(|_| ()));
                    if finished.is_some() {
                        matting::commands::matting_forget(id);
                    }
                    Some((status.progress.done, finished))
                },
                Bake::Matte,
            );
        }
        Item::Flow {
            job,
            segment_id,
            missing,
        } => {
            set_stage("Making slow-motion frames");
            let started = speed::flow::jobs::start(job, segment_id);
            follow(
                state,
                run,
                project_id,
                missing,
                "Optical flow",
                started,
                |id| {
                    let status = speed::flow::jobs::status(id)?;
                    Some((status.progress.done, status.finished.map(|r| r.map(|_| ()))))
                },
                Bake::Flow,
            );
        }
        Item::Enhance {
            job,
            segment_id,
            missing,
        } => {
            set_stage("Remaking frames");
            let started = enhance::jobs::start(job, segment_id);
            follow(
                state,
                run,
                project_id,
                missing,
                "Remove object / Enhance quality",
                started,
                |id| {
                    let status = enhance::jobs::status(id)?;
                    Some((status.progress.done, status.finished.map(|r| r.map(|_| ()))))
                },
                Bake::Enhance,
            );
        }
        Item::Landmarks {
            segment_id,
            missing,
        } => {
            use crate::modules::analysis::jobs::{self, JobKind};
            set_stage("Finding faces to retouch");
            let started = crate::modules::landmarks::commands::landmarks_analyse(
                state,
                segment_id.clone(),
                None,
            )
            .map(Some)
            .or_else(|error| {
                // Already being analysed (an edit queued it): follow
                // that job instead.
                jobs::all()
                    .into_iter()
                    .find(|j| {
                        j.kind == JobKind::Landmarks
                            && j.segment_id == segment_id
                            && j.finished.is_none()
                    })
                    .map(|j| Some(j.id))
                    .ok_or(error)
            });
            follow(
                state,
                run,
                project_id,
                missing,
                "Retouch",
                started,
                |id| {
                    let status = jobs::status(id)?;
                    let done = (status.fraction.clamp(0.0, 1.0) * missing as f32) as u32;
                    Some((done, status.finished.map(|r| r.map(|_| ()))))
                },
                Bake::Analysis,
            );
        }
        Item::Voice { segment_id } => {
            set_stage("Isolating voices");
            let rendered = crate::modules::voice::commands::voice_isolation_render(
                state,
                segment_id,
                &run.cancel,
                &|_| {},
            );
            match rendered {
                Ok(_) => {
                    run.status.lock().voices_done += 1;
                    // The mixers resolve a clip's sound per project
                    // snapshot; the app takes a fresh one when the run ends.
                    crate::modules::audiofx::cache::notify_landed();
                }
                Err(error) if !run.cancel.load(Ordering::Relaxed) => {
                    fail(run, "Isolate voice", &error)
                }
                Err(_) => {}
            }
        }
    }
}

fn fail(run: &Run, what: &str, error: &str) {
    tracing::warn!(%error, "{what} could not be prepared");
    run.status.lock().failures.push(format!("{what}: {error}"));
}

/// Follow bake `started` until it ends, counting up to `missing` frames
/// into the status. `poll` reads (frames done, how it ended) for a job.
#[allow(clippy::too_many_arguments)] // one call per bake kind, all in `execute`
fn follow(
    state: &AppState,
    run: &Run,
    project_id: &str,
    missing: u32,
    what: &str,
    started: Result<Option<u64>, String>,
    poll: impl Fn(u64) -> Option<(u32, Option<Result<(), String>>)>,
    kind: fn(u64) -> Bake,
) {
    let base = run.status.lock().frames_done;
    let job = match started {
        // Nothing was missing any more: another bake made it meanwhile.
        Ok(None) => {
            run.status.lock().frames_done = base + missing;
            return;
        }
        Ok(Some(job)) => job,
        Err(error) => {
            fail(run, what, &error);
            return;
        }
    };
    *run.current.lock() = Some(kind(job));
    let ended = loop {
        if !going(state, run, project_id) {
            cancel_current(run);
            break None;
        }
        match poll(job) {
            // The record is gone: someone else followed it to the end.
            None => break Some(Ok(())),
            Some((done, finished)) => {
                run.status.lock().frames_done = base + done.min(missing);
                if let Some(result) = finished {
                    break Some(result);
                }
            }
        }
        std::thread::sleep(POLL);
    };
    *run.current.lock() = None;
    match ended {
        Some(Ok(())) => run.status.lock().frames_done = base + missing,
        Some(Err(error)) if !run.cancel.load(Ordering::Relaxed) => fail(run, what, &error),
        _ => {}
    }
}
