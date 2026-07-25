//! The `#[tauri::command]` surface for the preview.
//!
//! Six calls that all do the same small thing: change what the frame server is
//! doing and hand back a [`PreviewInfo`] the frontend can drop into its store.
//! None of them carries a frame. Frames are bulk binary and leave over
//! `chukcut-frame://`; these calls only ever move the playhead and report where
//! it is. See `docs/architecture/preview-pipeline.md`.
//!
//! Every one of them returns in well under a millisecond: rendering, encoding
//! and device creation all happen on the frame server's own threads, and their
//! results arrive on the [`Channel`] that `preview_start` was given.

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use super::server::{PreviewEvent, PreviewInfo, PreviewServer, PreviewStatus};
use super::session::PreviewOptions;
use crate::modules::media::MediaSourceProvider;
use crate::modules::project::document::Micros;
use crate::state::AppState;

/// Open a preview session on the current project.
///
/// Also the way to pick up an edit: the project is snapshotted here, so after a
/// timeline change the frontend calls this again and gets a new session id.
/// Cheap — the threads and the GPU device stay alive across sessions.
#[tauri::command]
pub fn preview_start(
    state: State<'_, Arc<AppState>>,
    preview: State<'_, Arc<PreviewServer>>,
    time: Option<Micros>,
    options: Option<PreviewOptions>,
    on_event: Channel<PreviewEvent>,
) -> Result<PreviewInfo, String> {
    // Clone the document and drop the lock before anything slow. The render
    // thread works from this snapshot and never touches the live one.
    let project = Arc::new(
        state
            .project
            .read()
            .clone()
            .ok_or("no project is open to preview")?,
    );

    // Rebuild the provider from the same snapshot rather than reusing whatever
    // the last session had: a media file imported since then would otherwise be
    // invisible to the renderer until the app restarted.
    preview.set_source_provider(Arc::new(MediaSourceProvider::from_project(&project)));

    let info = preview.start(
        project,
        options.unwrap_or_default(),
        time.unwrap_or(0),
        on_event,
    );
    Ok(info)
}

/// Move the playhead and render exactly that frame.
///
/// Supersedes the session, so the returned `session` is new and any frame
/// request still in flight for the old one is answered with 410 rather than a
/// picture of where the playhead used to be.
#[tauri::command]
pub fn preview_seek(
    preview: State<'_, Arc<PreviewServer>>,
    time: Micros,
) -> Result<PreviewInfo, String> {
    preview.seek(time).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preview_play(preview: State<'_, Arc<PreviewServer>>) -> Result<PreviewInfo, String> {
    preview.play().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preview_pause(preview: State<'_, Arc<PreviewServer>>) -> Result<PreviewInfo, String> {
    preview.pause().map_err(|e| e.to_string())
}

/// Close the session, stop the threads and drop the ring.
#[tauri::command]
pub fn preview_stop(preview: State<'_, Arc<PreviewServer>>) -> Result<(), String> {
    preview.stop();
    Ok(())
}

/// Where the playhead is and what the preview is doing, for a frontend that
/// mounted after playback started or lost its channel.
#[tauri::command]
pub fn preview_state(preview: State<'_, Arc<PreviewServer>>) -> Result<PreviewStatus, String> {
    Ok(preview.status())
}
