//! The IPC surface for exporting.
//!
//! Ask what can be exported, check and estimate a request, start an export,
//! stop one; save and delete user presets; remember the dialog's settings;
//! and the queue. A single export works on its own thread, and progress comes
//! back over the caller's own `Channel` rather than a global event, so two
//! dialogs could never see each other's frames. The queue is the one shared
//! thing: there is one per process, and it outlives any dialog.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use serde::Serialize;

use crate::modules::project::document::{Micros, Project};

use crate::shell::Channel;

use crate::modules::render::{Compositor, CompositorConfig};
use crate::state::AppState;

use super::estimate::{self, SizeEstimate};
use super::job::{self, ExportJob, ExportOptions, ExportProgress, ExportRequest, ProgressSink};
use super::presets::{AudioCodec, Container, ExportPreset, Fps, VideoCodec};
use super::queue::{ExportQueue, QueueEvent, QueueItem, QueueRunner, RunResult};
use super::store::{self, ExportMemory};

/// Adapts Tauri's channel to the job's sink.
///
/// A wrapper rather than an impl on `Channel` itself so `job.rs` stays free of
/// Tauri and can be driven from a test.
struct ChannelSink(Channel<ExportProgress>);

impl ProgressSink for ChannelSink {
    fn send(&self, progress: ExportProgress) {
        // A failed send means the webview went away — the window closed, or
        // the dialog was torn down. That is not a reason to stop encoding: the
        // user may well want the file, and the job has its own cancel flag for
        // when they do not.
        if let Err(error) = self.0.send(progress) {
            tracing::debug!(%error, "export progress had nowhere to go");
        }
    }
}

/// Keeps the terminal message (done, failed, cancelled) back until the export
/// thread has dropped the job.
///
/// The job owns the decoders and the GPU textures they cache, and
/// `run_export` sends `Done` while the job is still alive. A caller that ends
/// the process when it hears `Done` — the CLI does, at once — then ran the
/// process exit while this thread was still freeing those textures in the
/// Vulkan driver, and crashed in it (docs/STATUS.md, "The crash after the
/// export").
struct HoldTerminal<'a> {
    sink: &'a dyn ProgressSink,
    held: parking_lot::Mutex<Option<ExportProgress>>,
}

impl<'a> HoldTerminal<'a> {
    fn new(sink: &'a dyn ProgressSink) -> Self {
        Self {
            sink,
            held: parking_lot::Mutex::new(None),
        }
    }

    fn take(&self) -> Option<ExportProgress> {
        self.held.lock().take()
    }
}

impl ProgressSink for HoldTerminal<'_> {
    fn send(&self, progress: ExportProgress) {
        if progress.stage.is_terminal() {
            *self.held.lock() = Some(progress);
        } else {
            self.sink.send(progress);
        }
    }
}

/// The export's pipeline, built once and shared by every export.
///
/// The *device* underneath it is the process's one device, from `gpu`, which
/// the preview's render thread is very likely already drawing with — an export
/// does not stop the user editing. This used to open a second one, and two live
/// Vulkan instances in one address space have been observed crashing the driver
/// on this machine; see the header of `modules/gpu`.
///
/// The pipeline is built lazily because a user who never exports should not pay
/// for it. `None` means this machine produced no usable adapter, which is a
/// real situation on a headless box without a software rasterizer.
static COMPOSITOR: OnceLock<Option<Arc<Compositor>>> = OnceLock::new();

fn compositor() -> Result<Arc<Compositor>, String> {
    COMPOSITOR
        .get_or_init(|| {
            let ctx = crate::modules::gpu::render_context()?;
            Some(Arc::new(Compositor::with_config(
                ctx,
                CompositorConfig {
                    // An export that quietly leaves a clip out is worse than
                    // one that fails: the hole is only discovered after the
                    // upload. The preview makes the opposite trade.
                    strict_sources: true,
                    ..Default::default()
                },
            )))
        })
        .clone()
        .ok_or_else(|| {
            "this machine has no GPU that can render frames, so nothing can be exported".to_string()
        })
}

/// Everything the export dialog needs to draw itself.
pub fn export_presets() -> ExportOptions {
    job::export_options()
}

/// The open document, cloned: what an export or an estimate works from.
fn snapshot(state: &Arc<AppState>) -> Result<Project, String> {
    state
        .project
        .read()
        .clone()
        .map(crate::modules::sequence::export_root)
        .ok_or_else(|| "no project is open, so there is nothing to export".to_string())
}

/// Everything one export needs, from a project snapshot and a request.
fn build_job(project: Project, request: &ExportRequest) -> Result<ExportJob, String> {
    // The whole timeline, even while a compound clip is open in the editor.
    let project = crate::modules::sequence::export_root(project);
    let settings = job::resolve_settings(&project, request).map_err(|e| e.to_string())?;
    let compositor = compositor()?;
    let job_id = uuid::Uuid::new_v4().to_string();
    // Built from this export's own snapshot, so a file imported since the last
    // export is present without anything having to re-register it.
    let sources: Arc<dyn crate::modules::render::SourceProvider> = Arc::new(
        crate::modules::media::MediaSourceProvider::from_project(&project),
    );
    Ok(ExportJob {
        job_id,
        project,
        settings,
        compositor,
        sources,
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    })
}

/// Start an export. Returns the job id to cancel it with.
///
/// Returns as soon as the settings are known to be valid — the encode itself
/// runs on its own thread and reports through `on_progress`.
pub fn export_start(
    state: &Arc<AppState>,
    request: ExportRequest,
    on_progress: Channel<ExportProgress>,
) -> Result<String, String> {
    // The snapshot. Taken under the lock, used outside it: an export must not
    // stop the user editing, and the cut they asked for is the one on screen
    // when they pressed the button, not whatever it becomes while it renders.
    let mut export = build_job(snapshot(state)?, &request)?;
    let job_id = export.job_id.clone();
    export.cancel = job::begin_job(&job_id);

    let sink = ChannelSink(on_progress);
    let id = job_id.clone();
    std::thread::Builder::new()
        .name(format!("chukcut-export-{job_id}"))
        .spawn(move || {
            let held = HoldTerminal::new(&sink);
            let outcome = job::run_export(&export, &held);
            // In this order: the job's resources go, then the job leaves
            // the registry `export_shutdown` waits on, then the caller hears
            // that it finished. See `HoldTerminal`.
            drop(export);
            job::end_job(&job_id);
            match outcome {
                Ok(result) if result.cancelled => {
                    tracing::info!(job = %job_id, "export cancelled");
                }
                Ok(result) => {
                    tracing::info!(
                        job = %job_id,
                        frames = result.frames,
                        seconds = result.elapsed.as_secs_f64(),
                        path = %result.output_path.display(),
                        "export finished"
                    );
                }
                Err(error) => {
                    // The terminal progress message already carried this to the
                    // user; the log is for the developer.
                    tracing::error!(job = %job_id, %error, "export failed");
                }
            }
            if let Some(last) = held.take() {
                sink.send(last);
            }
        })
        .map_err(|error| format!("could not start the export thread: {error}"))?;

    Ok(id)
}

// ---------------------------------------------------------------------------
// Checking and estimating
// ---------------------------------------------------------------------------

/// A request resolved against the open project: what it will produce, what
/// the user should know about it, and how big it will be.
#[derive(Debug, Clone, Serialize)]
pub struct ExportPlan {
    pub output_path: String,
    pub width: u32,
    pub height: u32,
    pub fps: Fps,
    pub video_codec: VideoCodec,
    pub audio_codec: AudioCodec,
    pub container: Container,
    pub audio_only: bool,
    pub has_audio: bool,
    pub loudness_target: Option<f32>,
    pub duration: Micros,
    pub total_frames: u64,
    pub warnings: Vec<String>,
    /// The instant estimate; see `export_estimate_sampled` for a measured one.
    pub estimate: SizeEstimate,
}

/// Resolve `request` without exporting: the settings it ends up with, its
/// warnings and the instant size estimate. Errors are the same prose the
/// export itself would fail with.
pub fn export_plan(state: &Arc<AppState>, request: &ExportRequest) -> Result<ExportPlan, String> {
    plan_for(&snapshot(state)?, request)
}

/// [`export_plan`] for a project that is not the open one.
pub fn plan_for(project: &Project, request: &ExportRequest) -> Result<ExportPlan, String> {
    let settings = job::resolve_settings(project, request).map_err(|e| e.to_string())?;
    Ok(ExportPlan {
        output_path: settings.output_path.display().to_string(),
        width: settings.video.width,
        height: settings.video.height,
        fps: settings.fps(),
        video_codec: settings.preset.video_codec,
        audio_codec: settings.preset.audio_codec,
        container: settings.preset.container,
        audio_only: settings.audio_only,
        has_audio: settings.audio.is_some(),
        loudness_target: settings.loudness_target,
        duration: settings.duration,
        total_frames: settings.total_frames,
        warnings: settings.warnings.clone(),
        estimate: estimate::quick(&settings),
    })
}

/// Measure how big the export will be by encoding samples of it.
///
/// Blocking, and seconds of work: run it off the UI thread. `cancel` stops
/// it between and inside samples, which is what a dialog wants when the
/// user changes a setting before the answer arrives.
pub fn export_estimate_sampled(
    state: &Arc<AppState>,
    request: &ExportRequest,
    cancel: Arc<AtomicBool>,
) -> Result<SizeEstimate, String> {
    estimate_sampled_for(snapshot(state)?, request, cancel)
}

/// [`export_estimate_sampled`] for a project that is not the open one — the
/// dialog's own snapshot, or another file.
pub fn estimate_sampled_for(
    project: Project,
    request: &ExportRequest,
    cancel: Arc<AtomicBool>,
) -> Result<SizeEstimate, String> {
    let mut job = build_job(project, request)?;
    job.cancel = cancel;
    estimate::sampled(&job).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// User presets and remembered settings
// ---------------------------------------------------------------------------

/// Save the open project's `request` as a user preset called `label`.
pub fn export_preset_save(
    state: &Arc<AppState>,
    request: &ExportRequest,
    label: &str,
) -> Result<ExportPreset, String> {
    let preset = store::preset_from_request(&snapshot(state)?, request, label)?;
    store::save_user_preset(preset)
}

/// Delete a user preset. `false` when there was none by that id.
pub fn export_preset_remove(id: &str) -> Result<bool, String> {
    store::remove_user_preset(id)
}

/// Every preset — built in, then the user's — without the hardware probe
/// `export_presets` runs.
pub fn export_preset_list() -> Vec<ExportPreset> {
    let mut presets = ExportPreset::all();
    presets.extend(store::user_presets());
    presets
}

/// The dialog's settings for a project: its own last ones, else the last
/// ones used anywhere.
pub fn export_memory_recall(project_id: Option<&str>) -> Option<ExportMemory> {
    store::recall(project_id)
}

/// Keep the dialog's settings for the next export of this project and of
/// any project.
pub fn export_memory_remember(
    project_id: Option<&str>,
    memory: &ExportMemory,
) -> Result<(), String> {
    store::remember(project_id, memory)
}

// ---------------------------------------------------------------------------
// The queue
// ---------------------------------------------------------------------------

/// Runs queued exports through the same job the dialog's export uses.
struct EngineRunner;

impl QueueRunner for EngineRunner {
    fn run(
        &self,
        project: &Project,
        request: &ExportRequest,
        cancel: Arc<AtomicBool>,
        sink: &dyn ProgressSink,
    ) -> Result<RunResult, String> {
        let mut export = build_job(project.clone(), request)?;
        export.cancel = cancel;
        let outcome = job::run_export(&export, sink).map_err(|e| e.to_string())?;
        Ok(RunResult {
            output_path: outcome.output_path.display().to_string(),
            cancelled: outcome.cancelled,
            elapsed_seconds: outcome.elapsed.as_secs_f64(),
        })
    }
}

static QUEUE: OnceLock<ExportQueue> = OnceLock::new();

/// The process's export queue.
pub fn export_queue() -> &'static ExportQueue {
    QUEUE.get_or_init(|| ExportQueue::new(EngineRunner))
}

/// Queue an export of the open project as it is now. The request is checked
/// first, so a setting that cannot work is refused here rather than failing
/// in the background later. Returns the item id.
pub fn export_queue_add(
    state: &Arc<AppState>,
    request: ExportRequest,
    label: Option<String>,
) -> Result<String, String> {
    export_queue_add_project(snapshot(state)?, request, label)
}

/// Queue an export of any project snapshot — another file, another cut.
pub fn export_queue_add_project(
    project: Project,
    request: ExportRequest,
    label: Option<String>,
) -> Result<String, String> {
    job::resolve_settings(&project, &request).map_err(|e| e.to_string())?;
    Ok(export_queue().add(project, request, label))
}

/// Every item in run order, finished ones included.
pub fn export_queue_list() -> Vec<QueueItem> {
    export_queue().items()
}

/// Skip a queued item or stop a running one.
pub fn export_queue_cancel(id: &str) -> bool {
    export_queue().cancel(id)
}

/// Take a queued or finished item off the list.
pub fn export_queue_remove(id: &str) -> Result<(), String> {
    export_queue().remove(id)
}

/// Move an item to position `to`.
pub fn export_queue_move(id: &str, to: usize) -> Result<(), String> {
    export_queue().move_to(id, to)
}

/// Drop finished items from the list. Returns how many.
pub fn export_queue_clear_finished() -> usize {
    export_queue().clear_finished()
}

/// Hear every queue change on `channel`. Returns an id to unsubscribe with.
pub fn export_queue_subscribe(channel: Channel<QueueEvent>) -> u64 {
    export_queue().subscribe(move |event| {
        let _ = channel.send(event.clone());
    })
}

pub fn export_queue_unsubscribe(id: u64) {
    export_queue().unsubscribe(id)
}

/// Block until the queue has nothing left to run.
pub fn export_queue_wait_idle() {
    export_queue().wait_idle()
}

/// Where the app keeps its queue between runs.
pub fn export_queue_file() -> std::path::PathBuf {
    crate::modules::workspace::paths::data_root().join("export-queue.json")
}

/// Make the queue survive a restart: keep it in [`export_queue_file`] from
/// now on and read back what the last run left. Returns how many exports came
/// back unfinished; they wait for [`export_queue_resume`] (or a new item).
/// The app calls this once at start-up; the CLI never does.
pub fn export_queue_restore() -> usize {
    export_queue().persist_to(export_queue_file())
}

/// Whether restored exports are waiting to be resumed.
pub fn export_queue_held() -> bool {
    export_queue().is_held()
}

/// Run the exports a restart brought back.
pub fn export_queue_resume() {
    export_queue().resume()
}

/// Whether an export runs, or a queued one is about to: the app's quit
/// guard. Exports a restart brought back and that wait to be resumed do not
/// count; quitting loses nothing of them.
pub fn export_queue_busy() -> bool {
    job::active_jobs() > 0 || QUEUE.get().is_some_and(|q| q.is_busy() && !q.is_held())
}

/// For quitting: stop every export, keep the queue file for the next start
/// (`ExportQueue::shutdown`), and wait a few seconds for the exports to let
/// go of the GPU. An export still rendering while the process exits can hang
/// the exit in the driver.
pub fn export_shutdown() {
    job::cancel_all();
    if let Some(queue) = QUEUE.get() {
        queue.shutdown(std::time::Duration::from_secs(5));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while job::active_jobs() > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Save the frame at `time` as a PNG at full canvas resolution.
///
/// Renders fresh through the export compositor rather than reading anything
/// out of the preview: the preview's frame is panel-sized and may be on a
/// lower quality-ladder rung, and neither belongs in a file. Returns the path
/// actually written, which may differ from the request by its extension.
pub async fn export_snapshot(
    state: &Arc<AppState>,
    time: i64,
    output_path: String,
) -> Result<String, String> {
    // The snapshot of the document, taken before leaving the main thread —
    // same rule as an export: the frame saved is the one on screen when the
    // button was pressed.
    let project = state
        .project
        .read()
        .clone()
        .ok_or("no project is open, so there is no frame to save")?;
    project.render_check()?;

    // Rendering a canvas-sized frame plus a PNG encode is tens to hundreds of
    // milliseconds, which is far past what a command may spend on the main
    // thread.
    crate::shell::spawn_blocking(move || {
        let compositor = compositor()?;
        let sources = crate::modules::media::MediaSourceProvider::from_project(&project);
        let written = super::snapshot::write_png(
            &project,
            time,
            &compositor,
            &sources,
            std::path::Path::new(&output_path),
        )?;
        tracing::info!(path = %written.display(), time, "frame snapshot written");
        Ok(written.display().to_string())
    })
    .await
    .map_err(|error| format!("the snapshot thread panicked: {error}"))?
}

/// Ask a running export to stop.
///
/// Succeeds even when the job has already finished — closing the dialog on the
/// last frame is a race with no winner, and reporting it as an error would put
/// a pointless message in front of the user.
pub fn export_cancel(job_id: String) -> Result<bool, String> {
    Ok(job::cancel_job(&job_id))
}

/// Stop every running export. For window close and app shutdown.
pub fn cancel_all_exports() {
    job::cancel_all();
    if let Some(queue) = QUEUE.get() {
        queue.cancel_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_an_unknown_job_is_not_an_error() {
        assert_eq!(export_cancel("no-such-job".into()), Ok(false));
    }

    #[test]
    fn the_options_command_always_answers() {
        let options = export_presets();
        assert!(!options.presets.is_empty());
        assert_eq!(options.default_preset_id, super::super::CUSTOM_PRESET_ID);
    }

    #[test]
    fn a_progress_sink_that_cannot_deliver_does_not_stop_the_job() {
        // The unit sink is the "nobody is watching" case the job falls back to.
        let sink: &dyn ProgressSink = &();
        sink.send(ExportProgress {
            job_id: "x".into(),
            stage: super::super::ExportStage::Encoding,
            frame: 1,
            total_frames: 2,
            fraction: 0.5,
            fps: 30.0,
            elapsed_seconds: 1.0,
            remaining_seconds: Some(1.0),
            output_path: None,
            message: None,
        });
    }
}
