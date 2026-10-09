//! A headless shell (the CLI, the MCP server) must not touch the user's
//! working copy.
//!
//! Its own test binary, because `autosave::enable_for_process` and
//! `disable_for_process` are one way and process wide: in the library's test
//! binary they would silently change what every other test sees.

use std::sync::Arc;

use chukcut_engine::modules::project::{autosave, commands as project_commands, Track, TrackKind};
use chukcut_engine::modules::timeline::{commands as timeline_commands, ops::EditCommand};
use chukcut_engine::state::AppState;

#[test]
fn edits_in_a_headless_process_leave_the_working_copy_alone() {
    // Point the config directory at a scratch place before anything reads it,
    // so a failure of this test cannot overwrite the real working copy.
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("headless-autosave");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", &root);

    // Off unless the app turns it on; and a headless shell's opt-out wins
    // even over that.
    assert!(!autosave::is_enabled());
    autosave::enable_for_process();
    assert!(autosave::is_enabled());
    autosave::disable_for_process();
    assert!(!autosave::is_enabled());

    let state: Arc<AppState> = AppState::new();
    project_commands::project_new(&state, "headless".into(), 1080, 1920, 30.0, false).unwrap();
    timeline_commands::timeline_apply(
        &state,
        EditCommand::AddTrack {
            track: Track::new(TrackKind::Text, "Text 1"),
            index: 2,
        },
    )
    .unwrap();
    let saved = root.join("saved.chukcut");
    project_commands::project_save(&state, Some(saved.to_string_lossy().into_owned())).unwrap();
    autosave::flush();

    assert!(saved.is_file(), "the explicit save still writes");
    assert!(
        !autosave::file().exists(),
        "no working copy at {}",
        autosave::file().display()
    );
}
