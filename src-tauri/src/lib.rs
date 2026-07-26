//! chukcut — application entry point and IPC registration.
//!
//! Every capability the frontend has lives in exactly one place: a
//! `#[tauri::command]` in a module under `modules/`, registered in the
//! `generate_handler!` block below. If a command is not in that list, the
//! webview cannot call it. That is the whole security model — the renderer has
//! no file system, no process spawn, and no GPU of its own.
//!
//! The one exception is deliberate and documented: preview frames leave over
//! the `chukcut-frame://` URI scheme registered here, because pushing video at
//! frame rate through `invoke()` does not work at any resolution. See
//! `docs/architecture/preview-pipeline.md`.

pub mod modules;
pub mod state;

use modules::audio::{AudioEngine, FileAudioSource};
use modules::preview::{frame_protocol_async, PreviewServer, DEFAULT_CAPACITY, SCHEME};
use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // stdout as before — `RUST_LOG` still works — plus a file under the
    // platform's state directory, because an app started from a launcher has no
    // stdout and "which export path ran" is not answerable without one. See
    // `modules::workspace::logging`.
    modules::workspace::logging::init();

    // Frames left over from a previous run describe a document that no longer
    // exists, so the cache starts empty every time.
    if let Err(error) = std::fs::remove_dir_all(modules::workspace::paths::cache_root().join("preview")) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%error, "could not clear the preview cache");
        }
    }

    // Audio is the clock master: the device's played-sample count is the
    // authority for the playhead, and video frames are matched to it. The
    // engine is built first so the preview can read from it.
    let audio = AudioEngine::new();
    let preview = PreviewServer::with_time_source(DEFAULT_CAPACITY, audio.time_source());
    preview.set_audio(std::sync::Arc::clone(&audio));

    // Until this was registered the exporter mixed a valid but silent audio
    // track, because no implementation of its `AudioSource` seam existed.
    modules::export::job::register_audio_source(std::sync::Arc::new(FileAudioSource));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .manage(audio)
        .manage(preview)
        // Asynchronous rather than the blocking variant: a frame request can
        // wait briefly for a frame that is mid-render, and that wait must never
        // sit on a webview thread.
        .register_asynchronous_uri_scheme_protocol(SCHEME, frame_protocol_async)
        // The native menu bar. Everything about it — the items, which of them
        // are clickable, and why the rest are not — lives in
        // `modules::workspace::menu`.
        .menu(modules::workspace::menu::build)
        .on_menu_event(modules::workspace::menu::handle_event)
        // The close button is the same action as File → Quit and is guarded the
        // same way: nothing closes until the webview has answered.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                modules::workspace::menu::request_close(window);
            }
        })
        .invoke_handler(tauri::generate_handler![
            // project
            modules::project::commands::project_new,
            modules::project::commands::project_open,
            modules::project::commands::project_save,
            modules::project::commands::project_import_media,
            modules::project::commands::project_get,
            modules::project::commands::project_validate,
            modules::project::commands::project_path,
            // timeline
            modules::timeline::commands::timeline_apply,
            modules::timeline::commands::timeline_apply_many,
            modules::timeline::commands::timeline_split,
            modules::timeline::commands::timeline_link,
            modules::timeline::commands::timeline_unlink,
            modules::timeline::commands::timeline_undo,
            modules::timeline::commands::timeline_redo,
            // transitions
            modules::transitions::commands::transitions_catalog,
            modules::transitions::commands::transitions_max_duration,
            modules::transitions::commands::transitions_add,
            modules::transitions::commands::transitions_remove,
            modules::transitions::commands::transitions_retime,
            modules::transitions::commands::transitions_set,
            // text
            modules::text::commands::text_fonts,
            modules::text::commands::text_add,
            modules::text::commands::text_set,
            // media
            modules::media::commands::media_probe,
            modules::media::commands::media_thumbnails,
            modules::media::commands::media_thumbnails_cancel,
            modules::media::commands::media_waveform,
            // preview
            modules::preview::commands::preview_start,
            modules::preview::commands::preview_seek,
            modules::preview::commands::preview_play,
            modules::preview::commands::preview_pause,
            modules::preview::commands::preview_stop,
            modules::preview::commands::preview_state,
            // audio
            modules::audio::commands::audio_status,
            modules::audio::commands::audio_set_volume,
            // export
            modules::export::commands::export_presets,
            modules::export::commands::export_start,
            modules::export::commands::export_cancel,
            // proxy
            modules::proxy::commands::proxy_request,
            modules::proxy::commands::proxy_cancel,
            modules::proxy::commands::proxy_watch,
            modules::proxy::commands::proxy_queue_status,
            modules::proxy::commands::proxy_state,
            modules::proxy::commands::proxy_cache_info,
            modules::proxy::commands::proxy_cache_clear,
            // effects
            modules::effects::commands::effects_describe,
            // workspace
            modules::workspace::commands::workspace_settings_get,
            modules::workspace::commands::workspace_settings_set,
            modules::workspace::commands::workspace_recent_list,
            modules::workspace::commands::workspace_recent_record,
            modules::workspace::commands::workspace_cache_size,
            modules::workspace::commands::workspace_cache_clear,
            modules::workspace::commands::workspace_hardware,
            modules::workspace::commands::workspace_log_path,
            modules::workspace::commands::workspace_menu_sync,
            modules::workspace::commands::workspace_close_answer,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
