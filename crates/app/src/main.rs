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

use chukcut_engine::state::AppState;
use gpui::application;
use gpui::{px, size, App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions};

use editor::*;

fn main() {
    chukcut_engine::init();

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
            // Plain keys stay out of text fields: typing "s" into a number
            // box must not split the clip.
            const TYPING_OFF: Option<&str> = Some("!Input");
            cx.bind_keys([
                KeyBinding::new("space", PlayPause, TYPING_OFF),
                KeyBinding::new("s", Split, TYPING_OFF),
                KeyBinding::new("ctrl-b", Split, None),
                KeyBinding::new("delete", DeleteSelected, TYPING_OFF),
                KeyBinding::new("backspace", DeleteSelected, TYPING_OFF),
                KeyBinding::new("ctrl-z", Undo, None),
                KeyBinding::new("ctrl-shift-z", Redo, None),
                KeyBinding::new("ctrl-y", Redo, None),
                KeyBinding::new("ctrl-i", Import, None),
                KeyBinding::new("ctrl-o", Open, None),
                KeyBinding::new("ctrl-s", Save, None),
                KeyBinding::new("ctrl-e", Export, None),
                KeyBinding::new("left", StepBack, TYPING_OFF),
                KeyBinding::new("right", StepForward, TYPING_OFF),
                KeyBinding::new("home", GoToStart, TYPING_OFF),
                KeyBinding::new("end", GoToEnd, TYPING_OFF),
                KeyBinding::new("ctrl-=", ZoomIn, None),
                KeyBinding::new("ctrl--", ZoomOut, None),
                KeyBinding::new("ctrl-q", Quit, None),
            ]);
            cx.bind_keys(timeline_key_bindings());
            cx.bind_keys(playback_key_bindings());
            cx.bind_keys(shortcut_key_bindings());
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
