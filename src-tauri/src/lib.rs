//! chukcut — application entry point and IPC registration.
//!
//! Every capability the frontend has lives in exactly one place: a
//! `#[tauri::command]` in a module under `modules/`, registered in the
//! `generate_handler!` block below. If a command is not in that list, the
//! webview cannot call it. That is the whole security model — the renderer has
//! no file system, no process spawn, and no GPU of its own.

pub mod modules;
pub mod state;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "chukcut=debug,warn".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            // project
            modules::project::commands::project_new,
            modules::project::commands::project_open,
            modules::project::commands::project_save,
            modules::project::commands::project_get,
            modules::project::commands::project_validate,
            modules::project::commands::project_path,
            // timeline
            modules::timeline::commands::timeline_apply,
            modules::timeline::commands::timeline_split,
            modules::timeline::commands::timeline_undo,
            modules::timeline::commands::timeline_redo,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
