//! The job runner for queued providers: submit, poll, fetch the result.
//!
//! A desktop app has no public URL for webhooks, so every queued job is
//! polled (`docs/research/integrations.md` §7.4). The runner knows nothing
//! about any vendor: a provider implements [`QueueApi`] and the runner owns
//! the loop — backoff, progress events, the cancel flag. Cancelling asks the
//! provider to cancel too, so a job the user gave up on is not billed when
//! the vendor can still stop it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// What a running job reports, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum JobEvent {
    /// Sending the user's media to the provider.
    Uploading { sent: u64, total: u64 },
    /// Waiting for a worker; `position` requests are ahead.
    Queued { position: Option<u32> },
    /// A worker has it. `log` is the provider's newest log line.
    Running { log: Option<String> },
    /// Fetching the result.
    Downloading { bytes: u64 },
    /// Finished; the result is on disk.
    Done,
}

/// The handles a provider gave back for a submitted job.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Submitted {
    pub request_id: String,
    pub status_url: String,
    pub response_url: String,
    pub cancel_url: String,
}

/// One status poll's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Queued(Option<u32>),
    Running(Option<String>),
    Completed,
    Failed(String),
}

/// A vendor's queue, for one job.
pub trait QueueApi {
    fn submit(&self) -> Result<Submitted, String>;
    fn status(&self, job: &Submitted) -> Result<JobStatus, String>;
    fn result(&self, job: &Submitted) -> Result<serde_json::Value, String>;
    fn cancel(&self, job: &Submitted) -> Result<(), String>;
}

/// How often to ask.
#[derive(Debug, Clone, Copy)]
pub struct PollPolicy {
    /// The first wait; it grows by half each time up to `max`.
    pub first: Duration,
    pub max: Duration,
    /// Give up after this long. The provider may still finish and bill; the
    /// message says so.
    pub timeout: Duration,
}

impl Default for PollPolicy {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(1),
            max: Duration::from_secs(10),
            timeout: Duration::from_secs(30 * 60),
        }
    }
}

/// Sleep `total`, waking every 50 ms to look at the cancel flag.
fn nap(total: Duration, cancel: &AtomicBool) -> bool {
    let until = Instant::now() + total;
    while Instant::now() < until {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50).min(until - Instant::now()));
    }
    !cancel.load(Ordering::Relaxed)
}

/// Run one job to its result. `Err("cancelled")` when the flag was raised,
/// after the provider has been asked to cancel.
pub fn run(
    api: &dyn QueueApi,
    policy: PollPolicy,
    events: &dyn Fn(JobEvent),
    cancel: &AtomicBool,
) -> Result<(Submitted, serde_json::Value), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_string());
    }
    let job = api.submit()?;
    events(JobEvent::Queued { position: None });
    let started = Instant::now();
    let mut wait = policy.first;
    let mut failures = 0;
    loop {
        if !nap(wait, cancel) {
            if let Err(error) = api.cancel(&job) {
                tracing::warn!(%error, "the provider did not take the cancel");
            }
            return Err("cancelled".to_string());
        }
        match api.status(&job) {
            Ok(JobStatus::Queued(position)) => events(JobEvent::Queued { position }),
            Ok(JobStatus::Running(log)) => events(JobEvent::Running { log }),
            Ok(JobStatus::Completed) => break,
            Ok(JobStatus::Failed(message)) => return Err(message),
            // A dropped connection mid-job is not the job failing: try a few
            // more times before giving up on it.
            Err(error) => {
                failures += 1;
                if failures >= 5 {
                    return Err(error);
                }
                tracing::info!(%error, "status poll failed; trying again");
            }
        }
        if started.elapsed() > policy.timeout {
            return Err(format!(
                "the job did not finish in {} minutes; it may still complete at the provider (request {})",
                policy.timeout.as_secs() / 60,
                job.request_id
            ));
        }
        wait = (wait + wait / 2).min(policy.max);
    }
    let result = api.result(&job)?;
    Ok((job, result))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    struct Scripted {
        statuses: RefCell<Vec<JobStatus>>,
        cancelled: RefCell<bool>,
    }

    impl QueueApi for Scripted {
        fn submit(&self) -> Result<Submitted, String> {
            Ok(Submitted {
                request_id: "r1".into(),
                ..Submitted::default()
            })
        }
        fn status(&self, _: &Submitted) -> Result<JobStatus, String> {
            let mut statuses = self.statuses.borrow_mut();
            Ok(if statuses.len() > 1 {
                statuses.remove(0)
            } else {
                statuses[0].clone()
            })
        }
        fn result(&self, _: &Submitted) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({"ok": true}))
        }
        fn cancel(&self, _: &Submitted) -> Result<(), String> {
            *self.cancelled.borrow_mut() = true;
            Ok(())
        }
    }

    fn quick() -> PollPolicy {
        PollPolicy {
            first: Duration::from_millis(1),
            max: Duration::from_millis(2),
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn a_job_reports_its_way_to_the_result() {
        let api = Scripted {
            statuses: RefCell::new(vec![
                JobStatus::Queued(Some(2)),
                JobStatus::Running(Some("step 1".into())),
                JobStatus::Completed,
            ]),
            cancelled: RefCell::new(false),
        };
        let seen = RefCell::new(Vec::new());
        let (job, result) = run(
            &api,
            quick(),
            &|e| seen.borrow_mut().push(e),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(job.request_id, "r1");
        assert_eq!(result["ok"], true);
        assert_eq!(
            seen.into_inner(),
            vec![
                JobEvent::Queued { position: None },
                JobEvent::Queued { position: Some(2) },
                JobEvent::Running {
                    log: Some("step 1".into())
                },
            ]
        );
    }

    #[test]
    fn a_failed_job_says_why() {
        let api = Scripted {
            statuses: RefCell::new(vec![JobStatus::Failed("NSFW".into())]),
            cancelled: RefCell::new(false),
        };
        let error = run(&api, quick(), &|_| {}, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(error, "NSFW");
    }

    #[test]
    fn cancelling_asks_the_provider_to_stop() {
        let api = Scripted {
            statuses: RefCell::new(vec![JobStatus::Running(None)]),
            cancelled: RefCell::new(false),
        };
        let cancel = AtomicBool::new(false);
        let polls = RefCell::new(0);
        let error = run(
            &api,
            quick(),
            &|_| {
                *polls.borrow_mut() += 1;
                if *polls.borrow() == 3 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error, "cancelled");
        assert!(*api.cancelled.borrow());
    }
}
