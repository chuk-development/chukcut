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
use crate::modules::project::document::{Micros, Project};
use crate::modules::render::SourceProvider;
use crate::state::AppState;

/// The provider from the last session, kept alive so its open decoders are.
static CACHED_PROVIDER: std::sync::Mutex<Option<(u64, Arc<MediaSourceProvider>)>> =
    std::sync::Mutex::new(None);

/// Identify the media a project references, ignoring how it is arranged.
///
/// FNV-1a over the sorted material id/path pairs: cheap, and stable across
/// runs. Sorted because the pool order is not meaningful and a reorder must not
/// look like a change.
fn material_fingerprint(project: &Project) -> u64 {
    let mut entries: Vec<String> = Vec::new();
    for m in &project.materials.videos {
        entries.push(format!("v:{}:{}", m.id, m.path));
    }
    for m in &project.materials.audios {
        entries.push(format!("a:{}:{}", m.id, m.path));
    }
    for m in &project.materials.images {
        entries.push(format!("i:{}:{}", m.id, m.path));
    }
    entries.sort();

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for entry in entries {
        for byte in entry.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn source_provider_for(project: &Project) -> Arc<dyn SourceProvider> {
    let fingerprint = material_fingerprint(project);
    let mut cached = CACHED_PROVIDER.lock().expect("provider cache is not poisoned");

    if let Some((known, provider)) = cached.as_ref() {
        if *known == fingerprint {
            return Arc::clone(provider) as Arc<dyn SourceProvider>;
        }
    }

    let provider = Arc::new(MediaSourceProvider::from_project(project));
    *cached = Some((fingerprint, Arc::clone(&provider)));
    provider as Arc<dyn SourceProvider>
}

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

    // Reuse the provider when the media has not changed.
    //
    // A provider owns the open decoders, and opening a file plus finding stream
    // info costs tens of milliseconds — so building a fresh one throws away
    // every warm decoder. That matters more than it sounds: the preview
    // restarts on *every edit*, so a session of trimming would have reopened
    // and re-seeked every file on the timeline after every single gesture.
    //
    // The fingerprint is the material set, not the document: moving a clip does
    // not change which files are open, and rebuilding for that would defeat the
    // purpose.
    preview.set_source_provider(source_provider_for(&project));

    tracing::info!(time = ?time, "IPC preview_start");
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
    tracing::info!(time, "IPC preview_seek");
    preview.seek(time).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preview_play(preview: State<'_, Arc<PreviewServer>>) -> Result<PreviewInfo, String> {
    tracing::info!("IPC preview_play");
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
