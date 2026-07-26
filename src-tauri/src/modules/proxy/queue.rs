//! The background queue that builds proxies.
//!
//! ## The rule that shapes everything else
//!
//! **An import must never wait for a proxy.** A 4K clip dropped on the timeline
//! is editable in the same second, with the original, and the proxy swaps
//! itself in when it exists. Anything else turns "add a clip" into a progress
//! dialog, which is the single most common complaint about every editor that
//! has this feature.
//!
//! So [`ProxyQueue::enqueue`] does no work at all: it consults the decision
//! rule, consults the cache, and either answers "already there", "not worth
//! it", or pushes a job and returns an id. The transcode happens on a worker
//! thread.
//!
//! ## One at a time
//!
//! The worker is a single thread and takes one job at a time. This is a choice,
//! not a simplification: transcoding is CPU-bound and memory-hungry, and *n*
//! concurrent 4K transcodes on a machine the user is also editing on make the
//! editing worse in exchange for finishing the last proxy sooner. The queue is
//! FIFO, so the clip the user imported first — which is the one they are
//! looking at — is proxied first.
//!
//! "Survives the app being busy" falls out of the same choice. The worker owns
//! no lock anything else wants: it takes the queue mutex to pull a job and
//! releases it before the transcode, so an enqueue, a cancel or a status query
//! from the UI thread never waits behind a running transcode.
//!
//! ## Cancellation
//!
//! Two shapes, and both matter. A **queued** job is removed from the deque and
//! reported cancelled immediately. A **running** job has its
//! `Arc<AtomicBool>` set; [`super::generate`] checks it once per frame, deletes
//! its partial file and unwinds. Cancelling something that already finished is
//! `false` rather than an error — closing a panel on the last frame is a race
//! nobody can win, and reporting it would put a pointless message in front of
//! the user.
//!
//! ## Progress
//!
//! Over [`ProxyProgressSink`], as `export::job` does it, with the Tauri
//! `Channel` adapter living in [`super::commands`] so nothing here knows about
//! IPC. Unlike an export there is one stream for all proxy activity rather than
//! one per job: proxies are ambient background work and the UI for them is a
//! single indicator, not a dialog per file.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};

use super::cache::{ProxyCache, SourceKey};
use super::decision::decide;
use super::generate::{self, ProxySpec};
use super::Result;

/// Where a job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyStage {
    Queued,
    Building,
    /// Finished, and the proxy is in the cache.
    Ready,
    Cancelled,
    Failed,
}

/// One progress message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyEvent {
    pub job_id: String,
    /// The file being proxied, so the UI can mark the right item in the media
    /// panel without keeping a job-id map of its own.
    pub source_path: String,
    pub stage: ProxyStage,
    pub frame: u64,
    pub total_frames: u64,
    /// `frame / total_frames`, clamped to 0..1.
    pub fraction: f32,
    /// Set on the terminal message when the proxy exists.
    pub proxy_path: Option<String>,
    /// User-facing prose on failure, and the decision's reason on success.
    pub message: Option<String>,
}

/// Where progress goes. A trait rather than Tauri's channel so the queue can be
/// driven from a test and so nothing here has to know about IPC.
pub trait ProxyProgressSink: Send + Sync {
    fn send(&self, event: ProxyEvent);
}

impl ProxyProgressSink for () {
    fn send(&self, _: ProxyEvent) {}
}

/// A closure as a sink.
pub struct FnSink<F>(pub F);

impl<F> ProxyProgressSink for FnSink<F>
where
    F: Fn(ProxyEvent) + Send + Sync,
{
    fn send(&self, event: ProxyEvent) {
        (self.0)(event)
    }
}

/// What [`ProxyQueue::enqueue`] decided to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "outcome")]
pub enum EnqueueOutcome {
    /// A valid proxy is already in the cache.
    Cached { proxy_path: String },
    /// The rule says this file plays fine as it is.
    NotNeeded { reason: String },
    /// Already queued or building; the existing job's id.
    AlreadyQueued { job_id: String },
    /// Queued. `job_id` cancels it.
    Queued {
        job_id: String,
        reason: String,
        total_frames: u64,
    },
}

/// A job as the UI sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyJobView {
    pub job_id: String,
    pub source_path: String,
    pub stage: ProxyStage,
    pub frame: u64,
    pub total_frames: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueStatus {
    pub running: Option<ProxyJobView>,
    pub pending: Vec<ProxyJobView>,
}

/// The transcode step, behind a trait.
///
/// The real implementation is [`FfmpegTranscoder`] and is what the app uses.
/// It is a seam because the queue's own behaviour — ordering, cancellation,
/// what happens when a job fails — has to be tested without a four-second
/// video transcode in the middle of it, and because a test needs a job it can
/// hold open for as long as it likes.
pub trait Transcoder: Send + Sync + 'static {
    fn transcode(
        &self,
        source: &Path,
        dest: &Path,
        spec: &ProxySpec,
        cancel: &AtomicBool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<generate::GeneratedProxy>;
}

/// The real one.
pub struct FfmpegTranscoder;

impl Transcoder for FfmpegTranscoder {
    fn transcode(
        &self,
        source: &Path,
        dest: &Path,
        spec: &ProxySpec,
        cancel: &AtomicBool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<generate::GeneratedProxy> {
        generate::generate(source, dest, spec, cancel, progress)
    }
}

struct Job {
    id: String,
    source: PathBuf,
    key: SourceKey,
    spec: ProxySpec,
    reason: String,
    cancel: Arc<AtomicBool>,
}

struct Running {
    id: String,
    source: PathBuf,
    total: u64,
    done: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    pending: VecDeque<Job>,
    running: Option<Running>,
    shutdown: bool,
}

struct Inner {
    state: Mutex<State>,
    wake: Condvar,
    sinks: Mutex<Vec<Arc<dyn ProxyProgressSink>>>,
    cache: Arc<ProxyCache>,
    transcoder: Arc<dyn Transcoder>,
    next_id: AtomicU64,
    /// Bumped every time a proxy becomes available.
    ///
    /// `preview::commands` caches its `MediaSourceProvider` against a
    /// fingerprint of the material pool, and a proxy appearing does not change
    /// the pool — so without this the preview would keep decoding the original
    /// until the next edit. Folding this counter into that fingerprint is the
    /// whole of what the preview has to do to pick a proxy up.
    generation: AtomicU64,
}

/// The queue. Cheap to clone; every clone is the same queue.
#[derive(Clone)]
pub struct ProxyQueue {
    inner: Arc<Inner>,
}

impl ProxyQueue {
    /// Start a queue against `cache`, with a worker thread.
    pub fn new(cache: Arc<ProxyCache>) -> Self {
        Self::with_transcoder(cache, Arc::new(FfmpegTranscoder))
    }

    pub fn with_transcoder(cache: Arc<ProxyCache>, transcoder: Arc<dyn Transcoder>) -> Self {
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            sinks: Mutex::new(Vec::new()),
            cache,
            transcoder,
            next_id: AtomicU64::new(1),
            generation: AtomicU64::new(0),
        });

        let worker = Arc::clone(&inner);
        // Named, because a stuck transcode should be identifiable in a stack
        // dump without guessing.
        let spawned = std::thread::Builder::new()
            .name("chukcut-proxy".into())
            .spawn(move || run_worker(worker));
        if let Err(error) = spawned {
            tracing::error!(%error, "could not start the proxy worker; proxies are disabled");
        }

        Self { inner }
    }

    /// The process-wide queue, against the process-wide cache.
    pub fn shared() -> &'static ProxyQueue {
        static SHARED: std::sync::OnceLock<ProxyQueue> = std::sync::OnceLock::new();
        SHARED.get_or_init(|| ProxyQueue::new(ProxyCache::shared()))
    }

    pub fn cache(&self) -> &ProxyCache {
        &self.inner.cache
    }

    /// How many proxies have become available since the app started.
    ///
    /// A caller that caches a decision made against the cache — the preview's
    /// provider, principally — folds this into its own key so that a proxy
    /// arriving invalidates it. See the field's documentation.
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Relaxed)
    }

    /// Start receiving progress. Every subscriber sees every job.
    pub fn subscribe(&self, sink: Arc<dyn ProxyProgressSink>) {
        self.inner.sinks.lock().push(sink);
    }

    /// The proxy for `source` if the cache has a valid one. Never blocks on a
    /// transcode and never starts one.
    pub fn lookup(&self, source: &Path) -> Option<PathBuf> {
        self.inner.cache.lookup(source)
    }

    /// Consider `source` for a proxy. Returns immediately, always.
    ///
    /// This is the function the import path calls. It probes the file — cheap,
    /// no decoder is opened — applies the decision rule, and queues a job only
    /// if the rule says so.
    pub fn enqueue(&self, source: &Path) -> Result<EnqueueOutcome> {
        self.enqueue_with(source, None)
    }

    /// [`Self::enqueue`] with a decode cost the caller has measured, which
    /// overrules the model. See [`super::decision::decide`].
    pub fn enqueue_with(&self, source: &Path, measured_ms: Option<f64>) -> Result<EnqueueOutcome> {
        let key = SourceKey::of(source)?;

        if let Some(existing) = self.inner.cache.lookup_by_key(&key) {
            return Ok(EnqueueOutcome::Cached {
                proxy_path: existing.to_string_lossy().into_owned(),
            });
        }

        {
            let state = self.inner.state.lock();
            if let Some(id) = find_job(&state, source) {
                return Ok(EnqueueOutcome::AlreadyQueued { job_id: id });
            }
        }

        let (profile, spec) = generate::plan(source)?;
        let decision = decide(&profile, measured_ms);
        if !decision.build {
            return Ok(EnqueueOutcome::NotNeeded {
                reason: decision.reason,
            });
        }

        let job_id = format!("proxy-{}", self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let total_frames = spec.total_frames();
        let job = Job {
            id: job_id.clone(),
            source: source.to_path_buf(),
            key,
            spec,
            reason: decision.reason.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        };

        self.emit(ProxyEvent {
            job_id: job_id.clone(),
            source_path: source.to_string_lossy().into_owned(),
            stage: ProxyStage::Queued,
            frame: 0,
            total_frames,
            fraction: 0.0,
            proxy_path: None,
            message: Some(decision.reason.clone()),
        });

        {
            let mut state = self.inner.state.lock();
            state.pending.push_back(job);
        }
        self.inner.wake.notify_all();

        Ok(EnqueueOutcome::Queued {
            job_id,
            reason: decision.reason,
            total_frames,
        })
    }

    /// Ask a job to stop, whether it is queued or running.
    ///
    /// `false` when there is no such job, which includes one that has already
    /// finished.
    pub fn cancel(&self, job_id: &str) -> bool {
        let cancelled = {
            let mut state = self.inner.state.lock();

            if let Some(position) = state.pending.iter().position(|job| job.id == job_id) {
                let job = state.pending.remove(position).expect("just found");
                Some((job.id, job.source, job.spec.total_frames()))
            } else if let Some(running) = state.running.as_ref() {
                if running.id == job_id {
                    // The worker notices within one frame.
                    running.cancel.store(true, Ordering::Relaxed);
                    // It reports its own terminal event when it unwinds, so
                    // nothing is emitted here.
                    return true;
                }
                None
            } else {
                None
            }
        };

        match cancelled {
            Some((id, source, total)) => {
                self.emit(ProxyEvent {
                    job_id: id,
                    source_path: source.to_string_lossy().into_owned(),
                    stage: ProxyStage::Cancelled,
                    frame: 0,
                    total_frames: total,
                    fraction: 0.0,
                    proxy_path: None,
                    message: None,
                });
                true
            }
            None => false,
        }
    }

    /// Cancel everything queued and whatever is running. For a project close,
    /// and for shutdown.
    pub fn cancel_all(&self) {
        let drained: Vec<_> = {
            let mut state = self.inner.state.lock();
            if let Some(running) = state.running.as_ref() {
                running.cancel.store(true, Ordering::Relaxed);
            }
            state
                .pending
                .drain(..)
                .map(|job| (job.id, job.source, job.spec.total_frames()))
                .collect()
        };

        for (id, source, total) in drained {
            self.emit(ProxyEvent {
                job_id: id,
                source_path: source.to_string_lossy().into_owned(),
                stage: ProxyStage::Cancelled,
                frame: 0,
                total_frames: total,
                fraction: 0.0,
                proxy_path: None,
                message: None,
            });
        }
    }

    pub fn status(&self) -> QueueStatus {
        let state = self.inner.state.lock();
        QueueStatus {
            running: state.running.as_ref().map(|running| ProxyJobView {
                job_id: running.id.clone(),
                source_path: running.source.to_string_lossy().into_owned(),
                stage: ProxyStage::Building,
                frame: running.done.load(Ordering::Relaxed),
                total_frames: running.total,
            }),
            pending: state
                .pending
                .iter()
                .map(|job| ProxyJobView {
                    job_id: job.id.clone(),
                    source_path: job.source.to_string_lossy().into_owned(),
                    stage: ProxyStage::Queued,
                    frame: 0,
                    total_frames: job.spec.total_frames(),
                })
                .collect(),
        }
    }

    /// Stop the worker after the current job. For tests and for shutdown.
    pub fn shutdown(&self) {
        {
            let mut state = self.inner.state.lock();
            state.shutdown = true;
            if let Some(running) = state.running.as_ref() {
                running.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.inner.wake.notify_all();
    }

    fn emit(&self, event: ProxyEvent) {
        emit_to(&self.inner, event)
    }
}

fn find_job(state: &State, source: &Path) -> Option<String> {
    if let Some(running) = state.running.as_ref() {
        if running.source == source {
            return Some(running.id.clone());
        }
    }
    state
        .pending
        .iter()
        .find(|job| job.source == source)
        .map(|job| job.id.clone())
}

fn emit_to(inner: &Inner, event: ProxyEvent) {
    for sink in inner.sinks.lock().iter() {
        sink.send(event.clone());
    }
}

fn run_worker(inner: Arc<Inner>) {
    loop {
        let job = {
            let mut state = inner.state.lock();
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(job) = state.pending.pop_front() {
                    state.running = Some(Running {
                        id: job.id.clone(),
                        source: job.source.clone(),
                        total: job.spec.total_frames(),
                        done: Arc::new(AtomicU64::new(0)),
                        cancel: Arc::clone(&job.cancel),
                    });
                    break job;
                }
                // Nothing to do. The lock is released while waiting, so an
                // enqueue from the UI thread never blocks behind the worker.
                inner.wake.wait(&mut state);
            }
        };

        run_one(&inner, job);

        inner.state.lock().running = None;
    }
}

fn run_one(inner: &Inner, job: Job) {
    let source_path = job.source.to_string_lossy().into_owned();
    let total = job.spec.total_frames();
    let dest = inner.cache.path_for(&job.key);

    let done = {
        let state = inner.state.lock();
        state
            .running
            .as_ref()
            .map(|running| Arc::clone(&running.done))
            .unwrap_or_else(|| Arc::new(AtomicU64::new(0)))
    };

    emit_to(
        inner,
        ProxyEvent {
            job_id: job.id.clone(),
            source_path: source_path.clone(),
            stage: ProxyStage::Building,
            frame: 0,
            total_frames: total,
            fraction: 0.0,
            proxy_path: None,
            message: Some(job.reason.clone()),
        },
    );

    let progress = {
        let inner_ref: &Inner = inner;
        let job_id = job.id.clone();
        let source_path = source_path.clone();
        let done = Arc::clone(&done);
        move |frame: u64, total: u64| {
            done.store(frame, Ordering::Relaxed);
            // Once every half second of output rather than every frame: a
            // thousand IPC messages for a thousand frames is a stutter in the
            // webview, and no progress bar needs that resolution.
            let step = progress_step(total);
            if frame % step != 0 && frame != total {
                return;
            }
            emit_to(
                inner_ref,
                ProxyEvent {
                    job_id: job_id.clone(),
                    source_path: source_path.clone(),
                    stage: ProxyStage::Building,
                    frame,
                    total_frames: total,
                    fraction: fraction(frame, total),
                    proxy_path: None,
                    message: None,
                },
            );
        }
    };

    let outcome =
        inner
            .transcoder
            .transcode(&job.source, &dest, &job.spec, &job.cancel, &progress);

    let event = match outcome {
        Ok(generated) => {
            match inner
                .cache
                .insert(job.key.clone(), generated.width, generated.height)
            {
                Ok(evicted) if evicted > 0 => {
                    tracing::info!(evicted, "the proxy cache evicted entries to make room");
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "a proxy was built but could not be recorded");
                }
            }
            // After the insert, so anything that re-resolves on seeing a new
            // generation finds the proxy already in the index.
            inner.generation.fetch_add(1, Ordering::Relaxed);
            ProxyEvent {
                job_id: job.id.clone(),
                source_path,
                stage: ProxyStage::Ready,
                frame: generated.frames,
                total_frames: total,
                fraction: 1.0,
                proxy_path: Some(generated.path.to_string_lossy().into_owned()),
                message: None,
            }
        }
        Err(error) if error.is_cancellation() => ProxyEvent {
            job_id: job.id.clone(),
            source_path,
            stage: ProxyStage::Cancelled,
            frame: done.load(Ordering::Relaxed),
            total_frames: total,
            fraction: fraction(done.load(Ordering::Relaxed), total),
            proxy_path: None,
            message: None,
        },
        Err(error) => {
            tracing::warn!(%error, source = %job.source.display(), "a proxy could not be built");
            // A failed transcode may have left a partial file that the cache
            // does not know about. Nothing indexes it, but it would sit on the
            // disk forever.
            let _ = std::fs::remove_file(&dest);
            ProxyEvent {
                job_id: job.id.clone(),
                source_path,
                stage: ProxyStage::Failed,
                frame: done.load(Ordering::Relaxed),
                total_frames: total,
                fraction: fraction(done.load(Ordering::Relaxed), total),
                proxy_path: None,
                message: Some(error.to_string()),
            }
        }
    };

    emit_to(inner, event);
}

fn fraction(frame: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (frame as f32 / total as f32).clamp(0.0, 1.0)
}

/// Report roughly two hundred times over a job, and at least every frame for a
/// very short one.
fn progress_step(total: u64) -> u64 {
    (total / 200).max(1)
}

/// The queue's view of one source, for the media panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum ProxyState {
    /// A valid proxy exists.
    Ready { proxy_path: String },
    Building { job_id: String, fraction: f32 },
    Queued { job_id: String },
    /// No proxy, and none is being built.
    None,
}

impl ProxyQueue {
    /// What the UI should show next to `source`.
    pub fn state_of(&self, source: &Path) -> ProxyState {
        if let Some(proxy) = self.inner.cache.lookup(source) {
            return ProxyState::Ready {
                proxy_path: proxy.to_string_lossy().into_owned(),
            };
        }
        let state = self.inner.state.lock();
        if let Some(running) = state.running.as_ref() {
            if running.source == source {
                return ProxyState::Building {
                    job_id: running.id.clone(),
                    fraction: fraction(running.done.load(Ordering::Relaxed), running.total),
                };
            }
        }
        if let Some(job) = state.pending.iter().find(|job| job.source == source) {
            return ProxyState::Queued {
                job_id: job.id.clone(),
            };
        }
        ProxyState::None
    }

    /// Resolve `source` into what the **preview** should decode.
    ///
    /// The other half of the switch. Note the return type: a
    /// [`super::PreviewSource`], which nothing in the export path accepts.
    pub fn preview_source(&self, source: &Path) -> super::PreviewSource {
        match self.inner.cache.lookup(source) {
            Some(proxy) => super::PreviewSource::with_proxy(source, proxy),
            None => super::PreviewSource::original_only(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::ProxyError;
    use crate::modules::export::Quality;
    use crate::modules::export::presets::Fps;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "chukcut-proxy-queue-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A transcoder that blocks until it is told to stop, and honours the
    /// cancel flag the way the real one does.
    struct SlowTranscoder {
        started: mpsc::Sender<()>,
    }

    impl Transcoder for SlowTranscoder {
        fn transcode(
            &self,
            _source: &Path,
            _dest: &Path,
            _spec: &ProxySpec,
            cancel: &AtomicBool,
            progress: &dyn Fn(u64, u64),
        ) -> Result<generate::GeneratedProxy> {
            let _ = self.started.send(());
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut frame = 0;
            while Instant::now() < deadline {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ProxyError::Cancelled);
                }
                frame += 1;
                progress(frame, 1_000_000);
                std::thread::sleep(Duration::from_millis(2));
            }
            // Ten seconds without a cancel means the test hung; failing is
            // more useful than a proxy nobody asked for.
            Err(ProxyError::Invalid("the slow transcoder timed out".into()))
        }
    }

    fn spec() -> ProxySpec {
        ProxySpec {
            width: 1280,
            height: 720,
            fps: Fps::THIRTY,
            quality: Quality::Crf(23),
            duration_micros: 4_000_000,
        }
    }

    fn job(id: &str, source: &Path) -> Job {
        Job {
            id: id.to_string(),
            source: source.to_path_buf(),
            key: SourceKey {
                path: source.to_string_lossy().into_owned(),
                size: 1,
                mtime_nanos: 1,
            },
            spec: spec(),
            reason: "because".into(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn collector() -> (Arc<dyn ProxyProgressSink>, mpsc::Receiver<ProxyEvent>) {
        let (tx, rx) = mpsc::channel();
        let sink: Arc<dyn ProxyProgressSink> = Arc::new(FnSink(move |event: ProxyEvent| {
            let _ = tx.send(event);
        }));
        (sink, rx)
    }

    fn wait_for(
        rx: &mpsc::Receiver<ProxyEvent>,
        predicate: impl Fn(&ProxyEvent) -> bool,
    ) -> ProxyEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(event) if predicate(&event) => return event,
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(other) => panic!("the event stream closed: {other}"),
            }
        }
        panic!("no matching event arrived within five seconds");
    }

    /// Cancelling a job that is **running**: the flag reaches the transcoder
    /// and it unwinds.
    #[test]
    fn a_running_job_can_be_cancelled() {
        let scratch = Scratch::new("cancel-running");
        let cache = Arc::new(ProxyCache::open(scratch.path().join("cache"), 1 << 30));
        let (started_tx, started_rx) = mpsc::channel();
        let queue = ProxyQueue::with_transcoder(
            cache,
            Arc::new(SlowTranscoder {
                started: started_tx,
            }),
        );
        let (sink, events) = collector();
        queue.subscribe(sink);

        // Pushed directly rather than through `enqueue`, which would need a
        // real media file to probe. What is under test is the queue.
        let source = scratch.path().join("clip.mp4");
        queue.inner.state.lock().pending.push_back(job("j1", &source));
        queue.inner.wake.notify_all();

        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the worker should have picked the job up");

        assert!(queue.cancel("j1"), "cancelling a running job reports true");

        let terminal = wait_for(&events, |e| e.stage == ProxyStage::Cancelled);
        assert_eq!(terminal.job_id, "j1");

        // And the queue is idle again, so the next job is not blocked.
        let deadline = Instant::now() + Duration::from_secs(5);
        while queue.status().running.is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(queue.status().running.is_none());
        queue.shutdown();
    }

    /// Cancelling a job that is still **queued**: it never runs at all.
    #[test]
    fn a_queued_job_can_be_cancelled_before_it_starts() {
        let scratch = Scratch::new("cancel-queued");
        let cache = Arc::new(ProxyCache::open(scratch.path().join("cache"), 1 << 30));
        let (started_tx, started_rx) = mpsc::channel();
        let queue = ProxyQueue::with_transcoder(
            cache,
            Arc::new(SlowTranscoder {
                started: started_tx,
            }),
        );
        let (sink, events) = collector();
        queue.subscribe(sink);

        let first = scratch.path().join("first.mp4");
        let second = scratch.path().join("second.mp4");
        {
            let mut state = queue.inner.state.lock();
            state.pending.push_back(job("j1", &first));
            state.pending.push_back(job("j2", &second));
        }
        queue.inner.wake.notify_all();

        // The worker takes them one at a time, so the second is still queued.
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first job starts");
        assert_eq!(
            queue.status().pending.len(),
            1,
            "the second job must still be waiting — one at a time is the point"
        );

        assert!(queue.cancel("j2"));
        let cancelled = wait_for(&events, |e| e.stage == ProxyStage::Cancelled);
        assert_eq!(cancelled.job_id, "j2");
        assert!(queue.status().pending.is_empty());

        // And it never started.
        assert!(
            started_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "a cancelled job must never reach the transcoder"
        );

        queue.cancel_all();
        queue.shutdown();
    }

    #[test]
    fn cancelling_a_job_that_does_not_exist_is_false_not_an_error() {
        let scratch = Scratch::new("cancel-unknown");
        let cache = Arc::new(ProxyCache::open(scratch.path().join("cache"), 1 << 30));
        let queue = ProxyQueue::new(cache);
        assert!(!queue.cancel("no-such-job"));
        queue.shutdown();
    }

    #[test]
    fn cancel_all_drains_the_queue() {
        let scratch = Scratch::new("cancel-all");
        let cache = Arc::new(ProxyCache::open(scratch.path().join("cache"), 1 << 30));
        let (started_tx, started_rx) = mpsc::channel();
        let queue = ProxyQueue::with_transcoder(
            cache,
            Arc::new(SlowTranscoder {
                started: started_tx,
            }),
        );
        let (sink, events) = collector();
        queue.subscribe(sink);

        {
            let mut state = queue.inner.state.lock();
            for n in 1..=4 {
                let source = scratch.path().join(format!("clip{n}.mp4"));
                state.pending.push_back(job(&format!("j{n}"), &source));
            }
        }
        queue.inner.wake.notify_all();
        started_rx.recv_timeout(Duration::from_secs(5)).expect("start");

        queue.cancel_all();
        assert!(queue.status().pending.is_empty());
        wait_for(&events, |e| e.stage == ProxyStage::Cancelled && e.job_id == "j1");
        queue.shutdown();
    }

    #[test]
    fn the_queue_is_first_in_first_out() {
        let scratch = Scratch::new("fifo");
        let cache = Arc::new(ProxyCache::open(scratch.path().join("cache"), 1 << 30));
        let (started_tx, _started_rx) = mpsc::channel();
        let queue = ProxyQueue::with_transcoder(
            cache,
            Arc::new(SlowTranscoder {
                started: started_tx,
            }),
        );

        {
            let mut state = queue.inner.state.lock();
            for n in 1..=3 {
                let source = scratch.path().join(format!("clip{n}.mp4"));
                state.pending.push_back(job(&format!("j{n}"), &source));
            }
        }
        queue.inner.wake.notify_all();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let status = queue.status();
            if let Some(running) = status.running {
                assert_eq!(running.job_id, "j1", "the first import is proxied first");
                assert_eq!(
                    status.pending.iter().map(|j| j.job_id.as_str()).collect::<Vec<_>>(),
                    vec!["j2", "j3"]
                );
                break;
            }
            assert!(Instant::now() < deadline, "the worker never started");
            std::thread::sleep(Duration::from_millis(5));
        }

        queue.cancel_all();
        queue.shutdown();
    }

    #[test]
    fn a_status_query_does_not_wait_behind_a_running_transcode() {
        // The property "survives the app being busy": the worker holds no lock
        // the UI thread wants while it transcodes.
        let scratch = Scratch::new("responsive");
        let cache = Arc::new(ProxyCache::open(scratch.path().join("cache"), 1 << 30));
        let (started_tx, started_rx) = mpsc::channel();
        let queue = ProxyQueue::with_transcoder(
            cache,
            Arc::new(SlowTranscoder {
                started: started_tx,
            }),
        );

        let source = scratch.path().join("clip.mp4");
        queue.inner.state.lock().pending.push_back(job("j1", &source));
        queue.inner.wake.notify_all();
        started_rx.recv_timeout(Duration::from_secs(5)).expect("start");

        let began = Instant::now();
        for _ in 0..100 {
            let _ = queue.status();
            let _ = queue.state_of(&source);
        }
        assert!(
            began.elapsed() < Duration::from_millis(500),
            "two hundred status queries took {:?} — the worker is holding a lock",
            began.elapsed()
        );

        queue.cancel_all();
        queue.shutdown();
    }

    #[test]
    fn progress_is_reported_but_not_once_per_frame() {
        // A thousand IPC messages for a thousand frames is a stutter in the
        // webview.
        assert_eq!(progress_step(1_000), 5);
        assert_eq!(progress_step(40_000), 200);
        // A very short job still reports every frame rather than none.
        assert_eq!(progress_step(10), 1);
        assert_eq!(progress_step(0), 1);
    }

    #[test]
    fn fractions_stay_inside_the_unit_interval() {
        assert_eq!(fraction(0, 0), 0.0);
        assert_eq!(fraction(5, 10), 0.5);
        assert_eq!(fraction(20, 10), 1.0);
    }

    #[test]
    fn the_preview_source_carries_the_proxy_and_the_export_type_cannot() {
        let scratch = Scratch::new("switch");
        let cache = Arc::new(ProxyCache::open(scratch.path().join("cache"), 1 << 30));
        let queue = ProxyQueue::new(Arc::clone(&cache));

        let source = scratch.path().join("clip.mp4");
        std::fs::write(&source, b"pretend this is footage").expect("write");

        // Nothing cached yet: the preview decodes the original.
        let preview = queue.preview_source(&source);
        assert!(!preview.is_proxied());
        assert_eq!(preview.decode_path(), source.as_path());

        // Cache one, and the preview switches.
        let key = SourceKey::of(&source).expect("key");
        std::fs::write(cache.path_for(&key), vec![0u8; 16]).expect("proxy");
        cache.insert(key.clone(), 1280, 720).expect("insert");

        let preview = queue.preview_source(&source);
        assert!(preview.is_proxied());
        assert_eq!(preview.decode_path(), cache.path_for(&key));
        assert_eq!(preview.original(), source.as_path());

        queue.shutdown();
    }
}
