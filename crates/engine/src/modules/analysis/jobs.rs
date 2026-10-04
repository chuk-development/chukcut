//! Background analysis jobs: one registry for every kind, so the status line
//! and a CLI can ask "what is running" in one place.
//!
//! The design is the tracking module's (`tracking/commands.rs`), which copied
//! the proxy queue's: starting a job does no work on the caller's thread, the
//! worker holds no lock the UI wants while it decodes, progress is a value the
//! UI polls (or a [`Channel`] it listens on), and cancelling is a flag the
//! worker checks between frames. When a job ends, **one** edit puts its result
//! into the document — the undo history gets one step per analysis.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::modules::project::document::Id;
use crate::shell::Channel;

/// What a job analyses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Scenes,
    Stabilise,
    Beats,
    Reframe,
    /// Face landmarks for retouch and face-follow (`modules::landmarks`).
    Landmarks,
    /// Body landmarks for following a body part (`modules::body`).
    Body,
}

impl JobKind {
    /// What the status line says while the job runs.
    pub fn busy(self) -> &'static str {
        match self {
            JobKind::Scenes => "Detecting scenes",
            JobKind::Stabilise => "Analysing camera shake",
            JobKind::Beats => "Detecting beats",
            JobKind::Reframe => "Finding the subject",
            JobKind::Landmarks => "Finding faces",
            JobKind::Body => "Finding people",
        }
    }
}

/// A job's state, as [`status`] reports it.
#[derive(Debug, Clone, Serialize)]
pub struct JobStatus {
    pub id: u64,
    pub kind: JobKind,
    /// The clip the job was started on.
    pub segment_id: Id,
    /// `0..=1`.
    pub fraction: f32,
    /// Set once the job has ended: a sentence for the status line, or why it
    /// failed. A cancelled job ends with `Err`.
    pub finished: Option<Result<String, String>>,
}

/// One message on a job's channel.
#[derive(Debug, Clone, Serialize)]
pub struct JobEvent {
    pub id: u64,
    pub fraction: f32,
}

/// What the worker of a job is handed: where to report, and whether to stop.
pub struct JobContext {
    id: u64,
    job: Arc<Job>,
    channel: Option<Channel<JobEvent>>,
}

impl JobContext {
    /// Whether the user asked the job to stop. Workers check this per frame.
    pub fn cancelled(&self) -> bool {
        self.job.cancel.load(Ordering::Relaxed)
    }

    /// The flag itself, for helpers that take an `&AtomicBool`.
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.job.cancel
    }

    /// Report progress, `0..=1`. Cheap; call it per frame.
    pub fn progress(&self, fraction: f32) {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.job.status.lock().fraction = fraction;
        if let Some(channel) = &self.channel {
            let _ = channel.send(JobEvent {
                id: self.id,
                fraction,
            });
        }
    }
}

struct Job {
    cancel: AtomicBool,
    status: Mutex<JobStatus>,
}

fn jobs() -> &'static Mutex<HashMap<u64, Arc<Job>>> {
    static JOBS: OnceLock<Mutex<HashMap<u64, Arc<Job>>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// The error a worker returns when it saw the cancel flag.
pub const CANCELLED: &str = "cancelled";

/// Start `work` on its own thread and return its id at once.
///
/// `work` returns the sentence the status line shows when it is done. A job of
/// the same kind already running on the same clip is refused: two analyses
/// racing to commit into one clip would leave whichever finished last.
pub fn spawn(
    kind: JobKind,
    segment_id: Id,
    channel: Option<Channel<JobEvent>>,
    work: impl FnOnce(&JobContext) -> Result<String, String> + Send + 'static,
) -> Result<u64, String> {
    {
        let running = jobs().lock();
        if running.values().any(|job| {
            let status = job.status.lock();
            status.kind == kind && status.segment_id == segment_id && status.finished.is_none()
        }) {
            return Err(format!("{} is already running on this clip", kind.busy()));
        }
    }
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    let job = Arc::new(Job {
        cancel: AtomicBool::new(false),
        status: Mutex::new(JobStatus {
            id,
            kind,
            segment_id,
            fraction: 0.0,
            finished: None,
        }),
    });
    jobs().lock().insert(id, Arc::clone(&job));
    let context = JobContext {
        id,
        job: Arc::clone(&job),
        channel,
    };
    std::thread::Builder::new()
        .name(format!("chukcut-analysis-{id}"))
        .spawn(move || {
            let started = std::time::Instant::now();
            // A panic in an analysis must end the job, not leave the status
            // line saying "Detecting…" forever.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&context)))
                .unwrap_or_else(|_| Err("the analysis crashed".into()));
            let result = match result {
                Err(error) if error == CANCELLED || context.cancelled() => {
                    Err("Analysis cancelled".to_string())
                }
                other => other,
            };
            tracing::info!(
                job = id,
                ?kind,
                seconds = started.elapsed().as_secs_f64(),
                ok = result.is_ok(),
                "analysis finished"
            );
            let mut status = job.status.lock();
            status.fraction = 1.0;
            status.finished = Some(result);
        })
        .map_err(|e| format!("could not start the analysis thread: {e}"))?;
    Ok(id)
}

/// A job's progress and, once it has ended, its result.
pub fn status(id: u64) -> Option<JobStatus> {
    jobs().lock().get(&id).map(|j| j.status.lock().clone())
}

/// Every job that has not been forgotten, oldest first.
pub fn all() -> Vec<JobStatus> {
    let mut list: Vec<JobStatus> = jobs()
        .lock()
        .values()
        .map(|j| j.status.lock().clone())
        .collect();
    list.sort_by_key(|s| s.id);
    list
}

/// Ask a job to stop. Nothing it found is committed.
pub fn cancel(id: u64) {
    if let Some(job) = jobs().lock().get(&id) {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

/// Drop a finished job's record.
pub fn forget(id: u64) {
    jobs().lock().remove(&id);
}

/// Block until a job has ended, for the CLI and tests. Returns its result.
pub fn wait(id: u64) -> Result<String, String> {
    loop {
        match status(id) {
            None => return Err("no such job".into()),
            Some(JobStatus {
                finished: Some(result),
                ..
            }) => return result,
            Some(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_reports_progress_and_its_result() {
        let id = spawn(JobKind::Beats, "jobs-test-a".into(), None, |ctx| {
            ctx.progress(0.5);
            Ok("done".into())
        })
        .unwrap();
        assert_eq!(wait(id), Ok("done".into()));
        let status = status(id).unwrap();
        assert_eq!(status.fraction, 1.0);
        forget(id);
        assert!(super::status(id).is_none());
    }

    #[test]
    fn a_cancelled_job_says_so() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let id = spawn(JobKind::Scenes, "jobs-test-b".into(), None, move |ctx| {
            rx.recv().ok();
            if ctx.cancelled() {
                return Err(CANCELLED.into());
            }
            Ok("not cancelled".into())
        })
        .unwrap();
        cancel(id);
        tx.send(()).unwrap();
        assert_eq!(wait(id), Err("Analysis cancelled".into()));
        forget(id);
    }

    #[test]
    fn the_same_analysis_does_not_run_twice_on_one_clip() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let id = spawn(JobKind::Stabilise, "jobs-test-c".into(), None, move |_| {
            rx.recv().ok();
            Ok(String::new())
        })
        .unwrap();
        assert!(
            spawn(JobKind::Stabilise, "jobs-test-c".into(), None, |_| Ok(
                String::new()
            ))
            .is_err()
        );
        tx.send(()).unwrap();
        wait(id).unwrap();
        forget(id);
    }

    #[test]
    fn a_panicking_analysis_ends_the_job() {
        let id = spawn(JobKind::Reframe, "jobs-test-d".into(), None, |_| {
            panic!("boom");
        })
        .unwrap();
        assert!(wait(id).is_err());
        forget(id);
    }
}
