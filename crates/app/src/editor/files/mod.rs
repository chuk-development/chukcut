//! Choosing files: the desktop's file chooser first, our own browser when
//! there is none.
//!
//! GPUI asks `xdg-desktop-portal` for every file dialog. Minimal window
//! managers (i3, sway without a portal backend, a bare X session) have no
//! portal, and then every Open, Import, Save as, caption import and export
//! folder failed with "File dialog failed" — a new project could never be
//! saved (`docs/QA.md`). So every request goes through [`choose`]: the portal
//! when it answers, and the in-app browser (`dialog.rs`, built from our kit)
//! when it errors. After one failure the portal is not asked again in this
//! process: a broken portal can take the D-Bus timeout to answer, and nobody
//! should wait that long twice. `CHUKCUT_FILE_DIALOG=builtin` skips it from
//! the start, `=portal` never falls back.

mod browse;
mod dialog;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, PathPromptOptions, SharedString, Task};

pub(crate) use browse::{Filter, Mode};

/// One question for the user.
#[derive(Clone, Debug)]
pub(crate) struct FileRequest {
    pub mode: Mode,
    /// The dialog's title and its button: "Import", "Open project", "Save".
    pub title: SharedString,
    pub filter: Filter,
    /// The folder to start in; home when `None` or gone.
    pub start: Option<PathBuf>,
}

impl FileRequest {
    pub(crate) fn open(title: impl Into<SharedString>, filter: Filter) -> Self {
        Self {
            mode: Mode::Open { multiple: false },
            title: title.into(),
            filter,
            start: None,
        }
    }

    pub(crate) fn open_many(title: impl Into<SharedString>, filter: Filter) -> Self {
        Self {
            mode: Mode::Open { multiple: true },
            ..Self::open(title, filter)
        }
    }

    pub(crate) fn folder(title: impl Into<SharedString>) -> Self {
        Self {
            mode: Mode::Folder,
            ..Self::open(title, Filter::Any)
        }
    }

    pub(crate) fn save(
        title: impl Into<SharedString>,
        filter: Filter,
        name: impl Into<String>,
    ) -> Self {
        Self {
            mode: Mode::Save { name: name.into() },
            ..Self::open(title, filter)
        }
    }

    pub(crate) fn starting_in(mut self, dir: impl Into<PathBuf>) -> Self {
        self.start = Some(dir.into());
        self
    }
}

/// Set once the portal has failed; from then on the browser opens at once.
static PORTAL_BROKEN: AtomicBool = AtomicBool::new(false);

fn use_portal() -> bool {
    match std::env::var("CHUKCUT_FILE_DIALOG").as_deref() {
        Ok("builtin") => false,
        Ok("portal") => true,
        _ => !PORTAL_BROKEN.load(Ordering::Relaxed),
    }
}

fn may_fall_back() -> bool {
    std::env::var("CHUKCUT_FILE_DIALOG").as_deref() != Ok("portal")
}

pub(crate) fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Ask. Resolves to the chosen paths, or `None` when the user cancelled.
/// Never fails: a portal that cannot be reached hands over to the browser.
pub(crate) fn choose(request: FileRequest, cx: &mut App) -> Task<Option<Vec<PathBuf>>> {
    // Where the request says, else where the user last chose something.
    let recent = browse::load_recent(&dialog::recent_file());
    let wanted = request.start.clone().or_else(|| recent.first().cloned());
    let start = browse::starting_dir(wanted.as_deref(), &home());
    let portal = use_portal().then(|| match &request.mode {
        Mode::Save { name } => Portal::Save(cx.prompt_for_new_path(&start, Some(name))),
        Mode::Open { multiple } => Portal::Open(cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: *multiple,
            prompt: Some(request.title.clone()),
        })),
        Mode::Folder => Portal::Open(cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(request.title.clone()),
        })),
    });
    cx.spawn(async move |cx| {
        if let Some(portal) = portal {
            let answer = match portal {
                Portal::Open(receiver) => receiver.await,
                Portal::Save(receiver) => receiver.await.map(|r| r.map(|p| p.map(|p| vec![p]))),
            };
            match answer {
                Ok(Ok(paths)) => return paths,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "no file chooser portal; using the built-in browser");
                }
                Err(_) => tracing::warn!("the file chooser portal went away"),
            }
            if !may_fall_back() {
                return None;
            }
            PORTAL_BROKEN.store(true, Ordering::Relaxed);
        }
        let (sender, receiver) = futures_channel::oneshot::channel();
        let opened = cx.update(|cx| {
            let window = cx
                .active_window()
                .or_else(|| cx.windows().first().copied())?;
            window
                .update(cx, |_, window, cx| {
                    dialog::open(request, start, sender, window, cx);
                })
                .ok()
        });
        if opened.is_none() {
            tracing::error!("no window to show the file browser in");
            return None;
        }
        // A dialog closed by Esc, the close button or a click outside drops
        // the sender: that is a cancel.
        receiver.await.ok().flatten()
    })
}

/// Ask for one path; the first of several.
pub(crate) fn choose_one(request: FileRequest, cx: &mut App) -> Task<Option<PathBuf>> {
    let task = choose(request, cx);
    cx.spawn(async move |_| task.await.and_then(|paths| paths.into_iter().next()))
}

enum Portal {
    Open(futures_channel::oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>>),
    Save(futures_channel::oneshot::Receiver<anyhow::Result<Option<PathBuf>>>),
}

/// The folder a project's files are saved next to: the project's own folder
/// once it has one.
pub(crate) fn project_dir(path: Option<&Path>) -> Option<PathBuf> {
    path.and_then(Path::parent).map(Path::to_path_buf)
}

/// A file name suggested from a project name: a name like "Before / After"
/// (a built-in template's) would otherwise be read as a folder and refused
/// by the save dialog. The `/` becomes `_`, as the export dialog does.
pub(crate) fn suggested_name(stem: &str, extension: &str) -> String {
    let stem = stem.trim().replace('/', "_");
    let stem = if stem.is_empty() {
        "Untitled"
    } else {
        stem.as_str()
    };
    format!("{stem}.{extension}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suggested_name_has_no_folder_in_it() {
        assert_eq!(
            suggested_name("Before / After", "chukcut"),
            "Before _ After.chukcut"
        );
        assert_eq!(suggested_name("  ", "png"), "Untitled.png");
        assert_eq!(suggested_name("Trip", "chukcut"), "Trip.chukcut");
        let typed = suggested_name("a/b/c", "chukcut");
        assert!(browse::save_target(Path::new("/x"), &typed, Filter::Projects).is_some());
    }
}
