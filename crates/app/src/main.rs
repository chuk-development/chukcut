//! chukcut — the native app.
//!
//! ```text
//! chukcut                      new project
//! chukcut project.chukcut      open a project
//! chukcut a.mp4 b.mov …        new project with these files on the timeline
//! ```

mod editor;
mod edits;
mod player;
mod theme;
mod ui;

use std::path::PathBuf;

use chukcut_engine::modules::project::commands as project_commands;
use chukcut_engine::state::AppState;
use gpui::application;
use gpui::{px, size, App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions};

use editor::*;

fn main() {
    chukcut_engine::init();

    let state = AppState::new();
    let mut media = Vec::new();
    let mut opened = false;
    for argument in std::env::args().skip(1) {
        let path = PathBuf::from(&argument);
        if path.extension().is_some_and(|e| e == "chukcut") && !opened {
            match project_commands::project_open(&state, argument.clone()) {
                Ok(_) => opened = true,
                Err(error) => eprintln!("chukcut: {error}"),
            }
        } else {
            media.push(path);
        }
    }
    if !opened {
        // 9:16 at 30 fps, CapCut's default. The first imported video adopts
        // its own shape while the timeline is empty.
        if let Err(error) =
            project_commands::project_new(&state, "Untitled".into(), 1080, 1920, 30.0)
        {
            eprintln!("chukcut: {error}");
            std::process::exit(1);
        }
    }

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
            cx.on_action(|_: &Quit, cx| cx.quit());

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
                |window, cx| cx.new(|cx| Editor::new(state, media, window, cx)),
            )
            .expect("open the editor window");
            cx.activate(true);
        });
}
