//! The document's lifecycle as the editor sees it: whether there are unsaved
//! changes, the three-way prompt that guards every way of losing them, saving,
//! and the window title.
//!
//! Decision 0004: New, Open, Quit and closing the window all go through
//! [`Editor::guard_unsaved`]. "Save" only clears the way when the save really
//! landed — a cancelled file dialog must not then discard the work, which is
//! the bug the guard exists to stop.
//!
//! The editor does not switch documents itself. Once the guard has passed, it
//! emits an [`EditorEvent`] and the shell (`shell.rs`) replaces the editor or
//! shows the start screen.

use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};

use chukcut_engine::modules::workspace::commands as workspace_commands;
use chukcut_engine::modules::workspace::Settings;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::dialog::DialogFooter;
use gpui::component::WindowExt as _;
use gpui::EventEmitter;

use super::playback::PlaybackState;
use super::*;

/// What the editor asks the shell to do, after the unsaved-changes guard.
pub(crate) enum EditorEvent {
    /// Close the document and show the start screen.
    Home,
    /// Close the document and open this project instead.
    Open(PathBuf),
    /// Close the document and quit.
    Quit,
}

impl EventEmitter<EditorEvent> for Editor {}

/// The shell's part of the editor's state. One field on [`Editor`], so the
/// panels' files never have to know it exists.
pub(crate) struct ShellState {
    /// A fingerprint of the document as it is in its file — or as it was
    /// created, for a project that was never saved. `None` matches nothing:
    /// recovered work is unsaved by definition.
    baseline: Option<u64>,
    /// `(generation, dirty)` — the answer is computed once per edit, not once
    /// per frame.
    dirty: Cell<Option<(u64, bool)>>,
    window_title: RefCell<String>,
    /// The application settings, as last written.
    pub(crate) settings: Settings,
    pub(crate) playback: PlaybackState,
}

impl ShellState {
    /// Whether the shuttle is moving the playhead at a speed other than 1×.
    pub(crate) fn playback_driven(&self) -> bool {
        self.playback.driven()
    }

    pub(crate) fn new(project: &Project) -> Self {
        Self {
            baseline: Some(fingerprint(project)),
            dirty: Cell::new(None),
            window_title: RefCell::new(String::new()),
            settings: workspace_commands::workspace_settings_get(),
            playback: PlaybackState::default(),
        }
    }
}

/// A hash of the document's serialised form. The serialisation is
/// deterministic (`BTreeMap` throughout, see `MaterialPool::extras`), so an
/// edit that is undone hashes the same as before it — and the document is
/// clean again, as in every editor people are used to.
pub(crate) fn fingerprint(project: &Project) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match serde_json::to_vec(project) {
        Ok(bytes) => bytes.hash(&mut hasher),
        // Unserialisable means unsaveable; never call that clean.
        Err(_) => return u64::MAX,
    }
    hasher.finish()
}

/// The answer to the unsaved-changes prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Save,
    Discard,
}

type Then = Box<dyn FnOnce(&mut Editor, &mut Window, &mut Context<Editor>)>;

impl Editor {
    /// Whether the document differs from its file.
    pub(crate) fn is_dirty(&self) -> bool {
        if let Some((generation, dirty)) = self.shell.dirty.get() {
            if generation == self.generation {
                return dirty;
            }
        }
        let dirty = self.shell.baseline != Some(fingerprint(&self.project));
        self.shell.dirty.set(Some((self.generation, dirty)));
        dirty
    }

    /// The document as it is now is the one in its file.
    pub(crate) fn mark_clean(&mut self) {
        self.shell.baseline = Some(fingerprint(&self.project));
        self.shell.dirty.set(None);
    }

    /// The document is not in its file, whatever it looks like: used for
    /// work restored after a crash.
    pub(crate) fn mark_unsaved(&mut self) {
        self.shell.baseline = None;
        self.shell.dirty.set(None);
    }

    /// Save to `path`, or to the project's own file when `None`. Marks the
    /// document clean and puts it on the recent list when the write landed.
    pub(crate) fn save_to(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) -> bool {
        let result = project_commands::project_save(
            &self.state,
            path.map(|path| path.to_string_lossy().to_string()),
        );
        let saved = match result {
            Ok(path) => {
                self.title.mark_saved(self.generation);
                self.mark_clean();
                record_recent(&path, &self.project.name);
                self.status = Some(format!("Saved {path}").into());
                true
            }
            Err(error) => {
                self.status = Some(format!("Could not save: {error}").into());
                false
            }
        };
        cx.notify();
        saved
    }

    /// Save, asking for a path first when the project has none. Answers
    /// whether the document is now in a file.
    pub(super) fn save_interactively(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        if self.state.project_path.read().is_some() {
            return Task::ready(self.save_to(None, cx));
        }
        let name = format!("{}.chukcut", self.project.name);
        let picked = files::choose_one(FileRequest::save("Save", Filter::Projects, name), cx);
        cx.spawn(async move |this, cx| {
            // Cancelled: the work is not saved, so nothing may proceed.
            let Some(path) = picked.await else {
                return false;
            };
            this.update(cx, |editor, cx| editor.save_to(Some(path), cx))
                .unwrap_or(false)
        })
    }

    /// Run `then` once unsaved changes are dealt with: right away when there
    /// are none, otherwise after Save (which must succeed) or Don't save.
    /// Cancel, Esc and the close button run nothing.
    pub(crate) fn guard_unsaved(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if !self.is_dirty() {
            then(self, window, cx);
            return;
        }
        self.pause();
        let then: Rc<RefCell<Option<Then>>> = Rc::new(RefCell::new(Some(Box::new(then))));
        let editor = cx.entity().downgrade();
        let name = self.project.name.clone();

        let answer = move |choice: Choice, window: &mut Window, cx: &mut App| {
            window.close_dialog(cx);
            let Some(then) = then.borrow_mut().take() else {
                return;
            };
            let _ = editor.update(cx, |editor, cx| match choice {
                Choice::Discard => then(editor, window, cx),
                Choice::Save => {
                    let saved = editor.save_interactively(cx);
                    cx.spawn_in(window, async move |this, cx| {
                        if saved.await {
                            let _ =
                                this.update_in(cx, |editor, window, cx| then(editor, window, cx));
                        }
                    })
                    .detach();
                }
            });
        };
        let answer = Rc::new(answer);

        window.open_dialog(cx, move |dialog, _, _| {
            let (save, discard) = (Rc::clone(&answer), Rc::clone(&answer));
            dialog
                .w(px(460.0))
                .title(format!("Save changes to \u{201c}{name}\u{201d}?"))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(TEXT_DIM))
                        .child("Your changes will be lost if you don't save them."),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("unsaved-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("unsaved-discard").label("Don't save").on_click(
                                move |_, window, cx| discard(Choice::Discard, window, cx),
                            ),
                        )
                        .child(
                            Button::new("unsaved-save")
                                .primary()
                                .label("Save")
                                .on_click(move |_, window, cx| save(Choice::Save, window, cx)),
                        ),
                )
        });
    }

    /// Menu → New project, Ctrl+N: back to the start screen, where the
    /// canvas is chosen.
    pub(crate) fn request_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.guard_unsaved(window, cx, |editor, _, cx| {
            editor.pause();
            cx.emit(EditorEvent::Home);
        });
    }

    /// Ctrl+O: pick a project, then hand it to the shell.
    pub(crate) fn request_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.guard_unsaved(window, cx, |_, _, cx| {
            let picked = files::choose_one(FileRequest::open("Open project", Filter::Projects), cx);
            cx.spawn(async move |this, cx| {
                if let Some(path) = picked.await {
                    let _ = this.update(cx, |editor, cx| {
                        editor.pause();
                        cx.emit(EditorEvent::Open(path));
                    });
                }
            })
            .detach();
        });
    }

    /// Quit and the window's close button.
    pub(crate) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.guard_unsaved(window, cx, |editor, _, cx| {
            editor.pause();
            cx.emit(EditorEvent::Quit);
        });
    }

    /// "name • — chukcut" while there are unsaved changes. Set only when it
    /// changes: it is a round trip to the window manager.
    pub(crate) fn sync_window_title(&self, window: &mut Window) {
        let marker = if self.is_dirty() { " \u{2022}" } else { "" };
        let title = format!("{}{marker} \u{2014} chukcut", self.project.name);
        if *self.shell.window_title.borrow() != title {
            window.set_window_title(&title);
            *self.shell.window_title.borrow_mut() = title;
        }
    }

    /// The settings dialog changed something; take what applies live.
    pub(crate) fn apply_settings(&mut self, settings: &Settings, cx: &mut Context<Self>) {
        self.shell.settings = settings.clone();
        self.preview.quality = super::settings::quality_for_scale(settings.preview_scale());
        self.consider_proxies();
        // A new size is a new request.
        self.last_request = None;
        cx.notify();
    }
}

/// Put a project on the start screen's list. A failure costs a convenience,
/// not work, so it is logged and nothing more.
pub(crate) fn record_recent(path: &str, name: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if let Err(error) = workspace_commands::workspace_recent_record(path.into(), name.into(), now) {
        tracing::warn!(%error, "could not update the recent projects list");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chukcut_engine::modules::project::CanvasConfig;

    #[test]
    fn an_undone_edit_is_clean_again() {
        let original = Project::new("a", CanvasConfig::default(), 30.0);
        let mut edited = original.clone();
        edited.name = "b".into();
        assert_ne!(fingerprint(&original), fingerprint(&edited));
        edited.name = "a".into();
        assert_eq!(fingerprint(&original), fingerprint(&edited));
    }
}
