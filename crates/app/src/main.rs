//! chukcut — the native app.
//!
//! ```text
//! chukcut                      the start screen
//! chukcut project.chukcut      open a project
//! chukcut a.mp4 b.mov …        new project with these files on the timeline
//! ```

mod editor;
mod edits;
mod player;
mod theme;
mod ui;

use chukcut_engine::state::AppState;
use gpui::application;
use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowOptions};

use editor::*;

fn main() {
    chukcut_engine::init();
    // The export queue outlives a restart: what was queued or running when
    // the app last quit comes back, held until the user runs it.
    chukcut_engine::modules::export::commands::export_queue_restore();

    let state = AppState::new();
    // Before the window: claims crash recovery, then opens or creates what
    // the command line names. See `editor/shell.rs`.
    let (startup, recovery) = editor::startup(&state);

    application()
        // The whole Lucide catalog, not only the component defaults: the
        // timeline needs icons (scissors, lock, magnet …) the defaults lack.
        .with_assets(gpui::assets::AllAssets)
        .run(move |cx: &mut App| {
            // GPUI Component's widgets, then our colours over its dark theme.
            gpui::init(cx);
            theme::apply(cx);
            // Every key comes from the shortcut registry, with the user's
            // own changes (`editor/keymap.rs`, `modules::keymap`).
            install_keymap(cx);
            // Quit asks about unsaved changes first; so does the close button.
            cx.on_action(|_: &Quit, cx| editor::quit(cx));
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1600.0), px(960.0)), cx);
            // `gpui::open_window` mounts the component Root, which dialogs,
            // popovers and notifications draw into.
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    app_id: Some("chukcut".into()),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Shell::new(state, startup, recovery, window, cx)),
            )
            .expect("open the editor window");
            cx.activate(true);
        });
}
