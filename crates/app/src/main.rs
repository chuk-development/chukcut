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

use std::path::PathBuf;

use chukcut_engine::modules::project::commands as project_commands;
use chukcut_engine::state::AppState;
use gpui::{px, size, App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions};
use gpui_platform::application;

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

    application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("space", PlayPause, None),
            KeyBinding::new("s", Split, None),
            KeyBinding::new("ctrl-b", Split, None),
            KeyBinding::new("delete", DeleteSelected, None),
            KeyBinding::new("backspace", DeleteSelected, None),
            KeyBinding::new("ctrl-z", Undo, None),
            KeyBinding::new("ctrl-shift-z", Redo, None),
            KeyBinding::new("ctrl-y", Redo, None),
            KeyBinding::new("ctrl-i", Import, None),
            KeyBinding::new("ctrl-o", Open, None),
            KeyBinding::new("ctrl-s", Save, None),
            KeyBinding::new("left", StepBack, None),
            KeyBinding::new("right", StepForward, None),
            KeyBinding::new("home", GoToStart, None),
            KeyBinding::new("end", GoToEnd, None),
            KeyBinding::new("ctrl-=", ZoomIn, None),
            KeyBinding::new("ctrl--", ZoomOut, None),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("chukcut".into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Editor::new(state, media, window, cx)),
        )
        .expect("open the editor window");
        cx.activate(true);
    });
}
