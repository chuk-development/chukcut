//! The export queue: several exports, one after another, in the background.
//!
//! An item is a project snapshot plus an [`ExportRequest`], so the queue can
//! hold the same cut in three presets, three ranges of it, or exports of
//! different projects — what is exported is what the timeline was when the
//! item was added, whatever happens to the document afterwards.
//!
//! One worker thread runs the items in list order, one at a time: exports are
//! already as parallel as the machine allows inside (decode threads, the
//! encoder's own threads, the GPU), so two at once would only make both
//! slower and the first one later. The thread exists only while there is
//! work, and the queue lives in the engine, not in a dialog — closing the
//! export dialog or the window that started an item does not stop it.
//!
//! Listeners hear about every change ([`QueueEvent`]); the app turns
//! `Finished` into a notification. The runner is a trait so the ordering,
//! cancel and reorder rules can be tested without a GPU.
//!
//! **Surviving a restart.** The app asks the queue to keep itself in a file
//! ([`ExportQueue::persist_to`]): every item with its project snapshot,
//! written after each change of the list or of an item's status (not on
//! progress). On the next start the file is read back. An item that was
//! queued or running comes back queued, and the queue is *held*: nothing runs
//! until [`ExportQueue::resume`] or a new item, so an export that crashed the
//! app cannot crash it again at start-up. The CLI never persists: its queue
//! lives for one call.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};

use crate::modules::project::document::Project;

use super::job::{ExportProgress, ExportRequest, ProgressSink};

/// Where an item is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl QueueStatus {
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            QueueStatus::Done | QueueStatus::Failed | QueueStatus::Cancelled
        )
    }
}

/// One export in the queue, as the UI and the CLI see it.
#[derive(Debug, Clone, Serialize)]
pub struct QueueItem {
    pub id: String,
    /// "TikTok · My project", or what the caller named it.
    pub label: String,
    pub project_name: String,
    pub request: ExportRequest,
    pub status: QueueStatus,
    /// The newest progress message while running.
    pub progress: Option<ExportProgress>,
    /// The file written, once done.
    pub output_path: Option<String>,
    pub bytes: Option<u64>,
    pub elapsed_seconds: Option<f64>,
    /// Prose, when it failed.
    pub error: Option<String>,
}

/// What listeners are told.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum QueueEvent {
    /// Items were added, removed, reordered, or one made progress.
    Changed,
    /// An item reached Done, Failed or Cancelled.
    Finished { item: Box<QueueItem> },
    /// The last queued item finished and the worker stopped.
    Idle,
}

/// What a finished export reports back to the queue.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub output_path: String,
    pub cancelled: bool,
    pub elapsed_seconds: f64,
}

/// Runs one export to the end. Blocking; the queue owns the thread.
pub trait QueueRunner: Send + Sync {
    fn run(
        &self,
        project: &Project,
        request: &ExportRequest,
        cancel: Arc<AtomicBool>,
        sink: &dyn ProgressSink,
    ) -> Result<RunResult, String>;
}

type Listener = Arc<dyn Fn(&QueueEvent) + Send + Sync>;

struct Entry {
    item: QueueItem,
    project: Arc<Project>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    worker: bool,
    next_listener: u64,
    /// Restored items wait for [`ExportQueue::resume`] or a new item.
    held: bool,
    /// Bumped on every saved change, so a slow write of an older list never
    /// lands after a newer one.
    generation: u64,
}

struct Inner {
    state: Mutex<State>,
    idle: Condvar,
    listeners: Mutex<Vec<(u64, Listener)>>,
    runner: Box<dyn QueueRunner>,
    /// The file the queue keeps itself in, once [`ExportQueue::persist_to`]
    /// was called; and the generation last written to it.
    store: Mutex<(Option<PathBuf>, u64)>,
}

/// One item as the queue file holds it.
#[derive(Serialize, Deserialize)]
struct SavedItem {
    id: String,
    label: String,
    project_name: String,
    request: ExportRequest,
    status: QueueStatus,
    #[serde(default)]
    output_path: Option<String>,
    #[serde(default)]
    bytes: Option<u64>,
    #[serde(default)]
    elapsed_seconds: Option<f64>,
    #[serde(default)]
    error: Option<String>,
    /// What it exports. Not kept for a finished item, which never runs again.
    #[serde(default)]
    project: Option<Project>,
}

/// The queue file.
#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    items: Vec<SavedItem>,
}

const SAVED_VERSION: u32 = 1;

/// The queue. Cheap to clone; every clone is the same queue.
#[derive(Clone)]
pub struct ExportQueue {
    inner: Arc<Inner>,
}

impl ExportQueue {
    pub fn new(runner: impl QueueRunner + 'static) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                idle: Condvar::new(),
                listeners: Mutex::new(Vec::new()),
                runner: Box::new(runner),
                store: Mutex::new((None, 0)),
            }),
        }
    }

    /// Keep the queue in `path` from now on, and read back what an earlier
    /// run left there. Returns how many items came back unfinished: they are
    /// queued again and held until [`Self::resume`] or a new item. A file
    /// that cannot be read is set aside (`.bad`) rather than blocking the
    /// queue. Call it once, before anything is added.
    pub fn persist_to(&self, path: PathBuf) -> usize {
        let restored = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Saved>(&bytes) {
                Ok(saved) => saved.items,
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "the export queue file is unreadable; starting empty");
                    let _ = std::fs::rename(&path, path.with_extension("bad"));
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        let waiting = {
            let mut state = self.inner.state.lock();
            for saved in restored {
                let unfinished = !saved.status.is_finished();
                // An unfinished item without its project cannot run; it
                // comes back failed, saying so.
                let (status, error) = match (unfinished, &saved.project) {
                    (true, Some(_)) => (QueueStatus::Queued, None),
                    (true, None) => (
                        QueueStatus::Failed,
                        Some("its project was not saved with the queue".to_string()),
                    ),
                    (false, _) => (saved.status, saved.error),
                };
                state.entries.push(Entry {
                    item: QueueItem {
                        id: saved.id,
                        label: saved.label,
                        project_name: saved.project_name,
                        request: saved.request,
                        status,
                        progress: None,
                        output_path: saved.output_path,
                        bytes: saved.bytes,
                        elapsed_seconds: saved.elapsed_seconds,
                        error,
                    },
                    project: Arc::new(saved.project.unwrap_or_else(|| {
                        Project::new(
                            "",
                            crate::modules::project::document::CanvasConfig::default(),
                            30.0,
                        )
                    })),
                    cancel: Arc::new(AtomicBool::new(false)),
                });
            }
            let waiting = state
                .entries
                .iter()
                .filter(|e| e.item.status == QueueStatus::Queued)
                .count();
            state.held = waiting > 0;
            waiting
        };
        self.inner.store.lock().0 = Some(path);
        if waiting > 0 {
            tracing::info!(waiting, "export queue restored; held until resumed");
        }
        self.emit(&QueueEvent::Changed);
        waiting
    }

    /// Whether restored items are waiting for [`Self::resume`].
    pub fn is_held(&self) -> bool {
        let state = self.inner.state.lock();
        state.held
            && state
                .entries
                .iter()
                .any(|e| e.item.status == QueueStatus::Queued)
    }

    /// Run what is queued: the restored items a start-up held back.
    pub fn resume(&self) {
        let start = {
            let mut state = self.inner.state.lock();
            state.held = false;
            let queued = state
                .entries
                .iter()
                .any(|e| e.item.status == QueueStatus::Queued);
            queued && !std::mem::replace(&mut state.worker, true)
        };
        if start {
            self.start_worker();
        }
        self.emit(&QueueEvent::Changed);
    }

    /// For quitting: keep the queue file as it is (what was queued or running
    /// comes back at the next start), stop the running export without
    /// recording it as cancelled, start nothing new, and wait up to `wait`
    /// for the worker to end. An export left running while the process exits
    /// can hang the exit inside the GPU driver.
    pub fn shutdown(&self, wait: std::time::Duration) {
        self.inner.store.lock().0 = None;
        let deadline = std::time::Instant::now() + wait;
        let mut state = self.inner.state.lock();
        state.held = true;
        for entry in &state.entries {
            if entry.item.status == QueueStatus::Running {
                entry.cancel.store(true, Ordering::Relaxed);
            }
        }
        while state.worker {
            if self.inner.idle.wait_until(&mut state, deadline).timed_out() {
                tracing::warn!("the export queue did not stop in time; quitting anyway");
                break;
            }
        }
    }

    fn start_worker(&self) {
        let queue = self.clone();
        let spawned = std::thread::Builder::new()
            .name("chukcut-export-queue".into())
            .spawn(move || queue.work());
        if let Err(error) = spawned {
            tracing::error!(%error, "could not start the export queue thread");
            self.inner.state.lock().worker = false;
        }
    }

    /// Write the list to the queue file, when there is one. The list is
    /// copied under the lock and written outside it; the generation keeps an
    /// older copy from overwriting a newer one.
    fn save(&self) {
        if self.inner.store.lock().0.is_none() {
            return;
        }
        let (generation, saved) = {
            let mut state = self.inner.state.lock();
            state.generation += 1;
            let items = state
                .entries
                .iter()
                .map(|e| SavedItem {
                    id: e.item.id.clone(),
                    label: e.item.label.clone(),
                    project_name: e.item.project_name.clone(),
                    request: e.item.request.clone(),
                    status: e.item.status,
                    output_path: e.item.output_path.clone(),
                    bytes: e.item.bytes,
                    elapsed_seconds: e.item.elapsed_seconds,
                    error: e.item.error.clone(),
                    project: (!e.item.status.is_finished()).then(|| (*e.project).clone()),
                })
                .collect();
            (
                state.generation,
                Saved {
                    version: SAVED_VERSION,
                    items,
                },
            )
        };
        let mut store = self.inner.store.lock();
        if generation <= store.1 {
            return;
        }
        let Some(path) = store.0.clone() else {
            return;
        };
        let written = serde_json::to_vec(&saved)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                let temp = path.with_extension("json.part");
                std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
                std::fs::rename(&temp, &path).map_err(|e| e.to_string())
            });
        match written {
            Ok(()) => store.1 = generation,
            Err(error) => tracing::warn!(%error, "could not save the export queue"),
        }
    }

    /// Add an export to the end of the queue and start the worker when it is
    /// not running. Returns the item's id.
    pub fn add(&self, project: Project, request: ExportRequest, label: Option<String>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let label = label
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| default_label(&project, &request));
        let item = QueueItem {
            id: id.clone(),
            label,
            project_name: project.name.clone(),
            request,
            status: QueueStatus::Queued,
            progress: None,
            output_path: None,
            bytes: None,
            elapsed_seconds: None,
            error: None,
        };
        let start = {
            let mut state = self.inner.state.lock();
            state.entries.push(Entry {
                item,
                project: Arc::new(project),
                cancel: Arc::new(AtomicBool::new(false)),
            });
            // Adding is asking for the queue to run, restored items too.
            state.held = false;
            !std::mem::replace(&mut state.worker, true)
        };
        self.save();
        self.emit(&QueueEvent::Changed);
        if start {
            self.start_worker();
        }
        id
    }

    /// Every item, in run order, finished ones included.
    pub fn items(&self) -> Vec<QueueItem> {
        self.inner
            .state
            .lock()
            .entries
            .iter()
            .map(|e| e.item.clone())
            .collect()
    }

    pub fn item(&self, id: &str) -> Option<QueueItem> {
        self.inner
            .state
            .lock()
            .entries
            .iter()
            .find(|e| e.item.id == id)
            .map(|e| e.item.clone())
    }

    /// Whether anything is queued or running.
    pub fn is_busy(&self) -> bool {
        self.inner
            .state
            .lock()
            .entries
            .iter()
            .any(|e| !e.item.status.is_finished())
    }

    /// Stop an item: a queued one is marked cancelled and skipped, a running
    /// one is asked to stop and reports cancelled within a frame. `false`
    /// when there is no such item or it already finished.
    pub fn cancel(&self, id: &str) -> bool {
        let finished = {
            let mut state = self.inner.state.lock();
            let Some(entry) = state.entries.iter_mut().find(|e| e.item.id == id) else {
                return false;
            };
            match entry.item.status {
                QueueStatus::Queued => {
                    entry.item.status = QueueStatus::Cancelled;
                    Some(entry.item.clone())
                }
                QueueStatus::Running => {
                    entry.cancel.store(true, Ordering::Relaxed);
                    None
                }
                _ => return false,
            }
        };
        self.save();
        if let Some(item) = finished {
            self.emit(&QueueEvent::Finished {
                item: Box::new(item),
            });
        }
        self.emit(&QueueEvent::Changed);
        true
    }

    /// Cancel everything queued and running. For app shutdown.
    pub fn cancel_all(&self) {
        let ids: Vec<String> = self
            .items()
            .into_iter()
            .filter(|i| !i.status.is_finished())
            .map(|i| i.id)
            .collect();
        for id in ids {
            self.cancel(&id);
        }
    }

    /// Take an item out of the list. A running item is refused — cancel it
    /// first; removing it would hide an export that is still writing.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        {
            let mut state = self.inner.state.lock();
            let Some(index) = state.entries.iter().position(|e| e.item.id == id) else {
                return Err("there is no such export in the queue".into());
            };
            if state.entries[index].item.status == QueueStatus::Running {
                return Err("this export is running; cancel it before removing it".into());
            }
            state.entries.remove(index);
        }
        self.save();
        self.emit(&QueueEvent::Changed);
        Ok(())
    }

    /// Move an item to position `to` in the list (clamped). Only the order of
    /// items that have not started matters: the worker always takes the
    /// first queued one.
    pub fn move_to(&self, id: &str, to: usize) -> Result<(), String> {
        {
            let mut state = self.inner.state.lock();
            let Some(from) = state.entries.iter().position(|e| e.item.id == id) else {
                return Err("there is no such export in the queue".into());
            };
            let entry = state.entries.remove(from);
            let to = to.min(state.entries.len());
            state.entries.insert(to, entry);
        }
        self.save();
        self.emit(&QueueEvent::Changed);
        Ok(())
    }

    /// Drop every finished item from the list.
    pub fn clear_finished(&self) -> usize {
        let removed = {
            let mut state = self.inner.state.lock();
            let before = state.entries.len();
            state.entries.retain(|e| !e.item.status.is_finished());
            before - state.entries.len()
        };
        if removed > 0 {
            self.save();
            self.emit(&QueueEvent::Changed);
        }
        removed
    }

    /// Hear about every change. Returns an id for [`Self::unsubscribe`].
    pub fn subscribe(&self, listener: impl Fn(&QueueEvent) + Send + Sync + 'static) -> u64 {
        let id = {
            let mut state = self.inner.state.lock();
            state.next_listener += 1;
            state.next_listener
        };
        self.inner.listeners.lock().push((id, Arc::new(listener)));
        id
    }

    pub fn unsubscribe(&self, id: u64) {
        self.inner.listeners.lock().retain(|(i, _)| *i != id);
    }

    /// Block until nothing is queued or running. For the CLI, which must not
    /// exit while the queue still writes.
    pub fn wait_idle(&self) {
        let mut state = self.inner.state.lock();
        while state.worker {
            self.inner.idle.wait(&mut state);
        }
    }

    fn emit(&self, event: &QueueEvent) {
        // Cloned out of the lock: a listener may call back into the queue.
        let listeners: Vec<Listener> = self
            .inner
            .listeners
            .lock()
            .iter()
            .map(|(_, l)| Arc::clone(l))
            .collect();
        for listener in listeners {
            listener(event);
        }
    }

    /// The worker: take the first queued item, run it, repeat; stop when
    /// none is left.
    fn work(&self) {
        loop {
            let next = {
                let mut state = self.inner.state.lock();
                // Held (restored, or shutting down): start nothing new.
                let held = state.held;
                let found = state
                    .entries
                    .iter_mut()
                    .filter(|_| !held)
                    .find(|e| e.item.status == QueueStatus::Queued);
                match found {
                    Some(entry) => {
                        entry.item.status = QueueStatus::Running;
                        Some((
                            entry.item.id.clone(),
                            Arc::clone(&entry.project),
                            entry.item.request.clone(),
                            Arc::clone(&entry.cancel),
                        ))
                    }
                    None => {
                        state.worker = false;
                        None
                    }
                }
            };
            let Some((id, project, request, cancel)) = next else {
                self.inner.idle.notify_all();
                self.emit(&QueueEvent::Idle);
                return;
            };
            self.save();
            self.emit(&QueueEvent::Changed);

            let sink = QueueSink {
                queue: self.clone(),
                id: id.clone(),
            };
            let result = self.inner.runner.run(&project, &request, cancel, &sink);

            let finished = {
                let mut state = self.inner.state.lock();
                state
                    .entries
                    .iter_mut()
                    .find(|e| e.item.id == id)
                    .map(|entry| {
                        let item = &mut entry.item;
                        match result {
                            Ok(run) if run.cancelled => item.status = QueueStatus::Cancelled,
                            Ok(run) => {
                                item.status = QueueStatus::Done;
                                item.bytes =
                                    std::fs::metadata(&run.output_path).map(|m| m.len()).ok();
                                item.output_path = Some(run.output_path);
                                item.elapsed_seconds = Some(run.elapsed_seconds);
                            }
                            Err(error) => {
                                item.status = QueueStatus::Failed;
                                item.error = Some(error);
                            }
                        }
                        item.clone()
                    })
            };
            self.save();
            if let Some(item) = finished {
                tracing::info!(
                    id = %item.id,
                    label = %item.label,
                    status = ?item.status,
                    "queued export finished"
                );
                self.emit(&QueueEvent::Finished {
                    item: Box::new(item),
                });
            }
            self.emit(&QueueEvent::Changed);
        }
    }
}

/// Progress from the running export into its item.
struct QueueSink {
    queue: ExportQueue,
    id: String,
}

impl ProgressSink for QueueSink {
    fn send(&self, progress: ExportProgress) {
        {
            let mut state = self.queue.inner.state.lock();
            if let Some(entry) = state.entries.iter_mut().find(|e| e.item.id == self.id) {
                entry.item.progress = Some(progress);
            }
        }
        self.queue.emit(&QueueEvent::Changed);
    }
}

/// "TikTok · My project", "Range 0:10–0:25 · My project".
fn default_label(project: &Project, request: &ExportRequest) -> String {
    let preset = request
        .preset_id
        .as_deref()
        .and_then(super::job::find_preset)
        .map(|p| p.label)
        .unwrap_or_else(|| {
            std::path::Path::new(&request.output_path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_uppercase())
                .unwrap_or_else(|| "Custom".into())
        });
    let range = request.range.map(|(a, b)| {
        let clock = |t: i64| {
            let s = (t.max(0) as f64 / 1e6).round() as i64;
            format!("{}:{:02}", s / 60, s % 60)
        };
        format!(" {}–{}", clock(a), clock(b))
    });
    format!("{preset}{} · {}", range.unwrap_or_default(), project.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::export::job::ExportStage;
    use std::time::{Duration, Instant};

    /// Pretends to export: takes `steps` × 5 ms, checks the cancel flag
    /// every step, records the order it ran things in, and fails a request
    /// whose output path contains "fail".
    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
        steps: u32,
    }

    impl QueueRunner for Fake {
        fn run(
            &self,
            _: &Project,
            request: &ExportRequest,
            cancel: Arc<AtomicBool>,
            sink: &dyn ProgressSink,
        ) -> Result<RunResult, String> {
            self.log.lock().push(request.output_path.clone());
            for step in 0..self.steps {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(RunResult {
                        output_path: request.output_path.clone(),
                        cancelled: true,
                        elapsed_seconds: 0.0,
                    });
                }
                sink.send(ExportProgress {
                    job_id: "fake".into(),
                    stage: ExportStage::Encoding,
                    frame: u64::from(step),
                    total_frames: u64::from(self.steps),
                    fraction: step as f32 / self.steps as f32,
                    fps: 0.0,
                    elapsed_seconds: 0.0,
                    remaining_seconds: None,
                    output_path: None,
                    message: None,
                });
                std::thread::sleep(Duration::from_millis(5));
            }
            if request.output_path.contains("fail") {
                return Err("the encoder exploded".into());
            }
            Ok(RunResult {
                output_path: request.output_path.clone(),
                cancelled: false,
                elapsed_seconds: 0.1,
            })
        }
    }

    fn queue(steps: u32) -> (ExportQueue, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let queue = ExportQueue::new(Fake {
            log: Arc::clone(&log),
            steps,
        });
        (queue, log)
    }

    fn request(path: &str) -> ExportRequest {
        ExportRequest {
            output_path: path.into(),
            preset_id: Some("tiktok".into()),
            overrides: None,
            hardware: None,
            include_audio: true,
            range: None,
        }
    }

    fn project() -> Project {
        crate::modules::export::testing::project(1080, 1920, 2_000_000)
    }

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn items_run_one_at_a_time_in_order() {
        let (queue, log) = queue(3);
        let running = Arc::new(Mutex::new((0usize, 0usize)));
        {
            let queue2 = queue.clone();
            let running = Arc::clone(&running);
            queue.subscribe(move |_| {
                let now = queue2
                    .items()
                    .iter()
                    .filter(|i| i.status == QueueStatus::Running)
                    .count();
                let mut r = running.lock();
                r.0 = now;
                r.1 = r.1.max(now);
            });
        }
        for name in ["a", "b", "c"] {
            queue.add(project(), request(name), None);
        }
        queue.wait_idle();
        assert_eq!(*log.lock(), vec!["a", "b", "c"]);
        assert_eq!(running.lock().1, 1, "never more than one at a time");
        assert!(queue.items().iter().all(|i| i.status == QueueStatus::Done));
        assert_eq!(queue.items()[0].label, "TikTok · t");
    }

    #[test]
    fn a_queued_item_can_be_moved_ahead_before_it_starts() {
        let (queue, log) = queue(20);
        let first = queue.add(project(), request("first"), None);
        wait_until("the first item to start", || {
            queue.item(&first).unwrap().status == QueueStatus::Running
        });
        queue.add(project(), request("second"), None);
        let third = queue.add(project(), request("third"), None);
        queue.move_to(&third, 0).unwrap();
        queue.wait_idle();
        assert_eq!(*log.lock(), vec!["first", "third", "second"]);
    }

    #[test]
    fn cancelling_skips_a_queued_item_and_stops_a_running_one() {
        let (queue, log) = queue(400);
        let first = queue.add(project(), request("first"), None);
        let second = queue.add(project(), request("second"), None);
        let third = queue.add(project(), request("third"), None);
        assert!(queue.cancel(&second));
        wait_until("the first item to start", || {
            queue.item(&first).unwrap().status == QueueStatus::Running
        });
        assert!(queue.cancel(&first));
        // Third would take 2 s; cancel it once it runs.
        wait_until("the third item to start", || {
            queue.item(&third).unwrap().status == QueueStatus::Running
        });
        queue.cancel(&third);
        queue.wait_idle();
        assert_eq!(*log.lock(), vec!["first", "third"]);
        for id in [&first, &second, &third] {
            assert_eq!(queue.item(id).unwrap().status, QueueStatus::Cancelled);
        }
        assert!(!queue.cancel(&first), "a finished item cannot be cancelled");
    }

    #[test]
    fn a_failure_does_not_stop_the_queue_and_finishes_are_announced() {
        let (queue, _) = queue(1);
        let finished = Arc::new(Mutex::new(Vec::new()));
        let idle = Arc::new(AtomicBool::new(false));
        {
            let finished = Arc::clone(&finished);
            let idle = Arc::clone(&idle);
            queue.subscribe(move |event| match event {
                QueueEvent::Finished { item } => {
                    finished.lock().push((item.label.clone(), item.status))
                }
                QueueEvent::Idle => idle.store(true, Ordering::Relaxed),
                QueueEvent::Changed => {}
            });
        }
        let bad = queue.add(project(), request("fail.mp4"), Some("bad".into()));
        queue.add(project(), request("good.mp4"), Some("good".into()));
        queue.wait_idle();
        wait_until("the idle event", || idle.load(Ordering::Relaxed));
        assert_eq!(
            *finished.lock(),
            vec![
                ("bad".to_string(), QueueStatus::Failed),
                ("good".to_string(), QueueStatus::Done)
            ]
        );
        assert_eq!(
            queue.item(&bad).unwrap().error.as_deref(),
            Some("the encoder exploded")
        );
    }

    #[test]
    fn a_running_item_cannot_be_removed_but_finished_ones_can_be_cleared() {
        let (queue, _) = queue(40);
        let first = queue.add(project(), request("first"), None);
        wait_until("the first item to start", || {
            queue.item(&first).unwrap().status == QueueStatus::Running
        });
        assert!(queue.remove(&first).is_err());
        queue.wait_idle();
        assert_eq!(queue.clear_finished(), 1);
        assert!(queue.items().is_empty());
        assert!(queue.remove("nope").is_err());
    }

    #[test]
    fn progress_reaches_the_item_and_the_worker_restarts_for_new_work() {
        let (queue, log) = queue(3);
        let id = queue.add(project(), request("one"), None);
        queue.wait_idle();
        assert!(queue.item(&id).unwrap().progress.is_some());
        assert!(!queue.is_busy());
        // The worker stopped; adding more starts it again.
        queue.add(project(), request("two"), None);
        queue.wait_idle();
        assert_eq!(*log.lock(), vec!["one", "two"]);
    }

    #[test]
    fn a_saved_queue_comes_back_held_and_runs_when_resumed() {
        let dir = crate::modules::workspace::trim::test_scratch("export-queue");
        let file = dir.join("export-queue.json");

        // The first run: one export done, one queued behind a long one. The
        // process "quits" while the long one runs.
        let (first, _) = queue(400);
        first.persist_to(file.clone());
        let done = first.add(project(), request("done.mp4"), None);
        wait_until("the first export to finish", || {
            first.item(&done).unwrap().status == QueueStatus::Done
        });
        first.add(project(), request("long.mp4"), Some("Long".into()));
        let waiting = first.add(project(), request("waiting.mp4"), None);
        wait_until("the file to say the long export runs", || {
            saved_statuses(&file).get(1) == Some(&QueueStatus::Running)
        });
        let saved: Saved = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved.items.len(), 3);
        assert!(
            saved.items[0].project.is_none(),
            "a finished item keeps no project"
        );
        assert_eq!(saved.items[1].status, QueueStatus::Running);
        assert!(saved.items[2].project.is_some());

        // The next run: the unfinished two come back queued, and wait.
        let (second, log) = queue(2);
        assert_eq!(second.persist_to(file.clone()), 2);
        assert!(second.is_held());
        let items = second.items();
        assert_eq!(
            items.iter().map(|i| i.status).collect::<Vec<_>>(),
            [QueueStatus::Done, QueueStatus::Queued, QueueStatus::Queued]
        );
        assert_eq!(items[1].label, "Long");
        std::thread::sleep(Duration::from_millis(30));
        assert!(log.lock().is_empty(), "nothing runs before resume");

        second.resume();
        second.wait_idle();
        assert_eq!(*log.lock(), ["long.mp4", "waiting.mp4"]);
        assert!(!second.is_held());
        assert!(second.item(&waiting).is_some());
        first.cancel_all();
        first.wait_idle();
    }

    #[test]
    fn shutting_down_keeps_the_file_as_it_was_and_starts_nothing() {
        let dir = crate::modules::workspace::trim::test_scratch("export-queue-quit");
        let file = dir.join("export-queue.json");
        let (q, log) = queue(400);
        q.persist_to(file.clone());
        q.add(project(), request("running.mp4"), None);
        q.add(project(), request("next.mp4"), None);
        let in_flight = [QueueStatus::Running, QueueStatus::Queued];
        wait_until("the file to say the export runs", || {
            saved_statuses(&file) == in_flight
        });
        q.shutdown(Duration::from_secs(5));
        assert!(!q.inner.state.lock().worker, "the worker stopped");
        assert_eq!(*log.lock(), ["running.mp4"], "nothing new started");
        assert_eq!(
            saved_statuses(&file),
            in_flight,
            "the file still says what was in flight"
        );
    }

    fn saved_statuses(file: &std::path::Path) -> Vec<QueueStatus> {
        std::fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
            .map(|saved| saved.items.iter().map(|i| i.status).collect())
            .unwrap_or_default()
    }

    #[test]
    fn an_unreadable_queue_file_is_set_aside() {
        let dir = crate::modules::workspace::trim::test_scratch("export-queue-bad");
        let file = dir.join("export-queue.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, b"{ not json").unwrap();
        let (q, _) = queue(1);
        assert_eq!(q.persist_to(file.clone()), 0);
        assert!(q.items().is_empty());
        assert!(file.with_extension("bad").is_file());
    }

    #[test]
    fn a_range_is_named_in_the_default_label() {
        let mut r = request("x.mp4");
        r.range = Some((10_000_000, 25_000_000));
        assert_eq!(default_label(&project(), &r), "TikTok 0:10–0:25 · t");
        r.preset_id = None;
        r.range = None;
        assert_eq!(default_label(&project(), &r), "MP4 · t");
    }
}
