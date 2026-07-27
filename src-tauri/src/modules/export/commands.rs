//! The IPC surface for exporting.
//!
//! Three commands: ask what can be exported, start an export, stop one. The
//! work itself happens on a thread, and progress comes back over the caller's
//! own `Channel` rather than a global event, so two dialogs could never see
//! each other's frames.

use std::sync::{Arc, OnceLock};

use tauri::ipc::Channel;
use tauri::State;

use crate::modules::render::{Compositor, CompositorConfig};
use crate::state::AppState;

use super::job::{self, ExportJob, ExportOptions, ExportProgress, ExportRequest, ProgressSink};

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
#[tauri::command]
pub fn export_presets() -> ExportOptions {
    job::export_options()
}

/// Start an export. Returns the job id to cancel it with.
///
/// Returns as soon as the settings are known to be valid — the encode itself
/// runs on its own thread and reports through `on_progress`.
#[tauri::command]
pub fn export_start(
    state: State<'_, Arc<AppState>>,
    request: ExportRequest,
    on_progress: Channel<ExportProgress>,
) -> Result<String, String> {
    // The snapshot. Taken under the lock, used outside it: an export must not
    // stop the user editing, and the cut they asked for is the one on screen
    // when they pressed the button, not whatever it becomes while it renders.
    let project = state
        .project
        .read()
        .clone()
        .ok_or("no project is open, so there is nothing to export")?;

    let settings = job::resolve_settings(&project, &request).map_err(|e| e.to_string())?;
    let compositor = compositor()?;

    let job_id = uuid::Uuid::new_v4().to_string();
    let cancel = job::begin_job(&job_id);

    // Built from this export's own snapshot, so a file imported since the last
    // export is present without anything having to re-register it. Built before
    // the struct literal because `project` is moved into it.
    let sources: Arc<dyn crate::modules::render::SourceProvider> = Arc::new(
        crate::modules::media::MediaSourceProvider::from_project(&project),
    );

    let export = ExportJob {
        job_id: job_id.clone(),
        project,
        settings,
        compositor,
        sources,
        audio: job::audio_source(),
        cancel,
    };

    let sink = ChannelSink(on_progress);
    let id = job_id.clone();
    std::thread::Builder::new()
        .name(format!("chukcut-export-{job_id}"))
        .spawn(move || {
            let outcome = job::run_export(&export, &sink);
            job::end_job(&export.job_id);
            match outcome {
                Ok(result) if result.cancelled => {
                    tracing::info!(job = %export.job_id, "export cancelled");
                }
                Ok(result) => {
                    tracing::info!(
                        job = %export.job_id,
                        frames = result.frames,
                        seconds = result.elapsed.as_secs_f64(),
                        path = %result.output_path.display(),
                        "export finished"
                    );
                }
                Err(error) => {
                    // The terminal progress message already carried this to the
                    // user; the log is for the developer.
                    tracing::error!(job = %export.job_id, %error, "export failed");
                }
            }
        })
        .map_err(|error| format!("could not start the export thread: {error}"))?;

    Ok(id)
}

/// Save the frame at `time` as a PNG at full canvas resolution.
///
/// Renders fresh through the export compositor rather than reading anything
/// out of the preview: the preview's frame is panel-sized and may be on a
/// lower quality-ladder rung, and neither belongs in a file. Returns the path
/// actually written, which may differ from the request by its extension.
#[tauri::command]
pub async fn export_snapshot(
    state: State<'_, Arc<AppState>>,
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

    // Rendering a canvas-sized frame plus a PNG encode is tens to hundreds of
    // milliseconds, which is far past what a command may spend on the main
    // thread.
    tauri::async_runtime::spawn_blocking(move || {
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
#[tauri::command]
pub fn export_cancel(job_id: String) -> Result<bool, String> {
    Ok(job::cancel_job(&job_id))
}

/// Stop every running export. For window close and app shutdown.
pub fn cancel_all_exports() {
    job::cancel_all();
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
