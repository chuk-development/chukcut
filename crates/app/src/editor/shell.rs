//! The window's root: the start screen or the editor, and every way in and
//! out of a document.
//!
//! Decision 0004. The shell owns the lifecycle: what launch does, opening,
//! creating, restoring after a crash, going back to the start screen and
//! quitting. The editor only ever asks — through [`EditorEvent`], after its
//! unsaved-changes guard has passed — and each document gets a fresh
//! [`Editor`], so no panel state (selection, scroll, caches) leaks from one
//! project into the next.
//!
//! A clean way out of a document always ends in `project_close`, which
//! deletes the working copy. A working copy that survives to the next launch
//! is therefore the sign of a crash, and is offered back (`recovery.rs` in
//! the engine).

use chukcut_engine::modules::project::recovery::RecoveryInfo;
use chukcut_engine::modules::workspace::commands as workspace_commands;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::dialog::DialogFooter;
use gpui::component::WindowExt as _;
use gpui::{AnyWindowHandle, Entity, Global, Subscription, WeakEntity};

use super::home::{recovery_detail, Home, HomeEvent};
use super::lifecycle::{record_recent, EditorEvent};
use super::*;

/// What the command line asked for.
pub enum Startup {
    /// Nothing: show the start screen.
    Home { notice: Option<String> },
    /// A project is open in the engine; import these files into it.
    Editor {
        media: Vec<PathBuf>,
        notice: Option<String>,
    },
}

/// Read the command line and prepare the engine's state, before the window
/// exists.
///
/// `chukcut project.chukcut` opens it; `chukcut a.mp4 …` makes a project with
/// the default canvas and puts the files on its timeline; plain `chukcut`
/// shows the start screen. Crash recovery is claimed first, because opening
/// or creating a project schedules an autosave that would overwrite the very
/// working copy the user is about to be offered.
pub fn startup(state: &Arc<AppState>) -> (Startup, Option<RecoveryInfo>) {
    let recovery = project_commands::project_recovery_claim();
    let mut media = Vec::new();
    let mut opened = false;
    // Why the project named on the command line did not open. Shown on the
    // screen the app starts on: stderr is not where a user looks.
    let mut notice = None;
    for argument in std::env::args().skip(1) {
        let path = PathBuf::from(&argument);
        if path.extension().is_some_and(|e| e == "chukcut") && !opened {
            // Absolute, so Details and Save show and use the real place
            // whatever the working directory was.
            match project_commands::project_open(state, absolute(&path)) {
                Ok(project) => {
                    opened = true;
                    record_recent(&absolute(&path), &project.name);
                }
                Err(error) => {
                    eprintln!("chukcut: {error}");
                    notice = Some(error);
                }
            }
        } else {
            // Absolute, like every other import: a relative path in the
            // document breaks when the app starts elsewhere, and the same
            // file imported again through a dialog became a second material.
            media.push(PathBuf::from(absolute(&path)));
        }
    }
    if !opened && media.is_empty() {
        return (Startup::Home { notice }, recovery);
    }
    if !opened {
        let settings = workspace_commands::workspace_settings_get();
        let (width, height) = settings.default_canvas;
        if let Err(error) = project_commands::project_new(
            state,
            "Untitled".into(),
            width,
            height,
            settings.default_fps,
            // Media on the command line, no canvas picked: the first clip
            // sets the shape.
            false,
        ) {
            eprintln!("chukcut: {error}");
            return (
                Startup::Home {
                    notice: Some(error),
                },
                recovery,
            );
        }
    }
    (Startup::Editor { media, notice }, recovery)
}

fn absolute(path: &std::path::Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

enum View {
    Home(Entity<Home>),
    Editor(Entity<Editor>),
}

/// Lets the app-wide Quit action, which has no window of its own, find the
/// shell.
#[derive(Clone)]
struct ShellHandle {
    window: AnyWindowHandle,
    shell: WeakEntity<Shell>,
}

impl Global for ShellHandle {}

/// Quit, by way of the unsaved-changes guard. Bound to the app-wide Quit
/// action in `main.rs`.
pub fn quit(cx: &mut App) {
    let Some(handle) = cx.try_global::<ShellHandle>().cloned() else {
        cx.quit();
        return;
    };
    // Deferred: an action handler runs while its window is borrowed for the
    // dispatch, so updating that window here would fail — and a fallback
    // that quits on failure would skip the guard. That happened.
    cx.defer(move |cx| {
        let result = handle.window.update(cx, |_, window, cx| {
            handle
                .shell
                .update(cx, |shell, cx| shell.request_quit(window, cx))
        });
        match result {
            Ok(Ok(())) => {}
            // The window or the shell is gone; nothing is left to guard.
            Ok(Err(_)) | Err(_) => cx.quit(),
        }
    });
}

pub struct Shell {
    state: Arc<AppState>,
    view: View,
    focus: FocusHandle,
    _events: Option<Subscription>,
    /// Set once the user has agreed to leave; the close request that follows
    /// must not ask again.
    closing: bool,
    /// The user said "Quit anyway" to a running export queue; the unsaved
    /// changes guard and the close that follow must not ask about it again.
    exports_confirmed: bool,
}

impl Shell {
    pub fn new(
        state: Arc<AppState>,
        startup: Startup,
        recovery: Option<RecoveryInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.set_global(ShellHandle {
            window: window.window_handle(),
            shell: cx.weak_entity(),
        });
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |shell, cx| shell.should_close(window, cx))
                .unwrap_or(true)
        });

        let placeholder = cx.new(|cx| Home::new(None, window, cx));
        let mut shell = Self {
            state,
            view: View::Home(placeholder),
            focus: cx.focus_handle(),
            _events: None,
            closing: false,
            exports_confirmed: false,
        };
        // Only on the start screen: a launch that opened a project or media
        // asked for that, not for a question about another session. The
        // work stays offered on the start screen and at the next launch.
        let prompt = recovery.is_some() && matches!(startup, Startup::Home { .. });
        let notice = match startup {
            Startup::Home { notice } => {
                shell.show_home(window, cx);
                notice
            }
            Startup::Editor { media, notice } => {
                shell.show_editor(media, false, window, cx);
                notice
            }
        };
        if let Some(notice) = notice {
            shell.home_notice(notice, cx);
        }
        if prompt {
            let this = cx.weak_entity();
            // After the window's root exists: dialogs draw into it.
            window.defer(cx, move |window, cx| {
                let _ = this.update(cx, |shell, cx| shell.prompt_recovery(window, cx));
            });
        }
        shell
    }

    fn editor(&self) -> Option<&Entity<Editor>> {
        match &self.view {
            View::Editor(editor) => Some(editor),
            View::Home(_) => None,
        }
    }

    fn show_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let recovery = project_commands::project_recovery_pending();
        let home = cx.new(|cx| Home::new(recovery, window, cx));
        let focus = home.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        self._events = Some(cx.subscribe_in(&home, window, Self::on_home_event));
        self.view = View::Home(home);
        window.set_window_title("chukcut");
        cx.notify();
    }

    /// A project is open in the engine; give it an editor.
    fn show_editor(
        &mut self,
        media: Vec<PathBuf>,
        restored: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = Arc::clone(&self.state);
        // What the open found wrong with the file, said once: the project
        // opens anyway, and a render refuses it until it is fixed.
        let notes = project_commands::project_open_notes(&state);
        let editor = cx.new(|cx| {
            let mut editor = Editor::new(state, media, window, cx);
            if restored {
                editor.mark_unsaved();
            }
            if let Some(first) = notes.first() {
                editor.status = Some(
                    match notes.len() {
                        1 => format!("This project opened with a problem: {first}"),
                        n => format!(
                            "This project opened with {n} problems; the first: {first}. The log lists all of them."
                        ),
                    }
                    .into(),
                );
            }
            editor
        });
        self._events = Some(cx.subscribe_in(&editor, window, Self::on_editor_event));
        self.view = View::Editor(editor);
        cx.notify();
    }

    fn on_home_event(
        &mut self,
        _: &Entity<Home>,
        event: &HomeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            HomeEvent::Create {
                name,
                width,
                height,
                fps,
                canvas_chosen,
            } => {
                match project_commands::project_new(
                    &self.state,
                    name.clone(),
                    *width,
                    *height,
                    *fps,
                    *canvas_chosen,
                ) {
                    Ok(_) => self.show_editor(Vec::new(), false, window, cx),
                    Err(error) => self.home_notice(error, cx),
                }
            }
            HomeEvent::Open(path) => self.open_path(path.clone(), window, cx),
            HomeEvent::FromTemplate(request) => {
                self.open_from_template(request.clone(), window, cx)
            }
            HomeEvent::Browse => self.browse(window, cx),
            HomeEvent::Restore => self.restore(window, cx),
            HomeEvent::DiscardRecovery => {
                project_commands::project_recovery_discard();
                if let View::Home(home) = &self.view {
                    home.update(cx, |home, cx| {
                        home.recovery = None;
                        cx.notify();
                    });
                }
            }
        }
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Home => {
                project_commands::project_close(&self.state, false);
                self.show_home(window, cx);
            }
            EditorEvent::Open(path) => self.open_path(path.clone(), window, cx),
            EditorEvent::Quit => self.finish_and_quit(cx),
            EditorEvent::FromTemplate(request) => {
                self.open_from_template(request.clone(), window, cx)
            }
        }
    }

    fn home_notice(&mut self, error: String, cx: &mut Context<Self>) {
        match &self.view {
            View::Home(home) => home.update(cx, |home, cx| {
                home.notice = Some(error.into());
                cx.notify();
            }),
            View::Editor(editor) => editor.update(cx, |editor, cx| {
                editor.status = Some(error.into());
                cx.notify();
            }),
        }
    }

    /// Open a project file in a fresh editor. A file that will not open
    /// leaves whatever was on screen as it was, and says why.
    fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let argument = path.to_string_lossy().to_string();
        match project_commands::project_open(&self.state, argument) {
            Ok(project) => {
                record_recent(&absolute(&path), &project.name);
                self.show_editor(Vec::new(), false, window, cx);
            }
            Err(error) => {
                self.home_notice(error, cx);
                if let View::Home(home) = &self.view {
                    home.update(cx, |home, cx| home.reload(cx));
                }
            }
        }
    }

    /// A new project from a template, the files in its slots. Probing the
    /// files is IO, so the project is built off the UI thread; whatever was
    /// on screen stays until it is ready, and stays if it fails.
    fn open_from_template(
        &mut self,
        request: super::templates::FillRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use chukcut_engine::modules::template::commands as template_commands;
        self.home_notice("Making the project from the template\u{2026}".into(), cx);
        let media: Vec<String> = request
            .media
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        cx.spawn_in(window, async move |this, cx| {
            let built = cx
                .background_executor()
                .spawn(async move {
                    template_commands::template_build_project(
                        &request.template_id,
                        &media,
                        request.name,
                    )
                })
                .await;
            let _ = this.update_in(cx, |shell, window, cx| match built {
                Ok(applied) => {
                    if shell.editor().is_some() {
                        project_commands::project_close(&shell.state, false);
                    }
                    template_commands::template_open_project(&shell.state, &applied.project);
                    shell.show_editor(Vec::new(), false, window, cx);
                    if let Some(editor) = shell.editor() {
                        editor.update(cx, |editor, cx| {
                            // Never saved: the guard asks before it is lost.
                            editor.mark_unsaved();
                            if !applied.empty.is_empty() {
                                editor.status = Some(
                                    format!(
                                        "{} slot{} still empty: Templates \u{2192} This project fills them",
                                        applied.empty.len(),
                                        if applied.empty.len() == 1 { " is" } else { "s are" }
                                    )
                                    .into(),
                                );
                            }
                            cx.notify();
                        });
                    }
                }
                Err(error) => shell.home_notice(error, cx),
            });
        })
        .detach();
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = files::choose_one(FileRequest::open("Open project", Filter::Projects), cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Some(path) = picked.await {
                let _ = this.update_in(cx, |shell, window, cx| shell.open_path(path, window, cx));
            }
        })
        .detach();
    }

    /// Restore the work a crashed session left, replacing what is open once
    /// that is safe to replace.
    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor().cloned() else {
            self.restore_now(window, cx);
            return;
        };
        let this = cx.weak_entity();
        editor.update(cx, |editor, cx| {
            editor.guard_unsaved(window, cx, move |_, window, cx| {
                // Not from inside the editor's own update.
                window.defer(cx, move |window, cx| {
                    let _ = this.update(cx, |shell, cx| shell.restore_now(window, cx));
                });
            })
        });
    }

    fn restore_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match project_commands::project_recovery_restore(&self.state) {
            Ok(_) => self.show_editor(Vec::new(), true, window, cx),
            Err(error) => self.home_notice(error, cx),
        }
    }

    /// The prompt at launch after a crash. "Not now" keeps the work for the
    /// start screen's banner and the next launch.
    fn prompt_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = project_commands::project_recovery_pending() else {
            return;
        };
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (restore, discard) = (this.clone(), this.clone());
            dialog
                .w(px(500.0))
                .title("Restore unsaved work?")
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_sm()
                        .child(format!(
                            "chukcut did not close properly last time. \u{201c}{}\u{201d} \
                             has changes that are not in a file.",
                            info.name
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_DIM))
                                .child(recovery_detail(&info)),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("recovery-later")
                                .label("Not now")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(Button::new("recovery-discard").label("Discard").on_click(
                            move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = discard.update(cx, |shell, cx| {
                                    shell.on_home_event_discard(cx);
                                });
                            },
                        ))
                        .child(
                            Button::new("recovery-restore")
                                .primary()
                                .label("Restore")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ =
                                        restore.update(cx, |shell, cx| shell.restore(window, cx));
                                }),
                        ),
                )
        });
    }

    fn on_home_event_discard(&mut self, cx: &mut Context<Self>) {
        project_commands::project_recovery_discard();
        if let View::Home(home) = &self.view {
            home.update(cx, |home, cx| {
                home.recovery = None;
                cx.notify();
            });
        }
    }

    /// Quit from the menu or Ctrl+Q.
    pub(crate) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.guard_exports(window, cx) {
            return;
        }
        match self.editor().cloned() {
            Some(editor) => editor.update(cx, |editor, cx| editor.request_quit(window, cx)),
            None => self.finish_and_quit(cx),
        }
    }

    /// Ask before quitting while the export queue runs. `true` when it asked:
    /// the answer comes back through [`Self::request_quit`], with the
    /// question already answered. The queue is kept in a file
    /// (`export_queue_restore`), so quitting anyway loses nothing but the
    /// running export's progress: it starts over after the restart.
    fn guard_exports(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use chukcut_engine::modules::export::commands as export_commands;
        if self.exports_confirmed || !export_commands::export_queue_busy() {
            return false;
        }
        let shell = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let shell = shell.clone();
            dialog
                .w(px(460.0))
                .title("Export running \u{2014} quit anyway?")
                .child(div().text_sm().text_color(rgb(TEXT_DIM)).child(
                    "The export queue is still working. It comes back the next time \
                             chukcut starts, and the export that was running starts over.",
                ))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("exports-quit-cancel")
                                .label("Keep exporting")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("exports-quit-anyway")
                                .primary()
                                .label("Quit anyway")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = shell.update(cx, |shell, cx| {
                                        shell.exports_confirmed = true;
                                        shell.request_quit(window, cx);
                                    });
                                }),
                        ),
                )
        });
        true
    }

    /// The window's close button. Answering `false` keeps the window open
    /// while the guard asks; its answer comes back as `EditorEvent::Quit`.
    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.closing {
            return true;
        }
        if self.guard_exports(window, cx) {
            return false;
        }
        if let Some(editor) = self.editor().cloned() {
            if editor.read(cx).is_dirty() {
                editor.update(cx, |editor, cx| editor.request_quit(window, cx));
                return false;
            }
        }
        self.finish();
        true
    }

    /// A clean exit: no working copy left behind to be taken for a crash.
    fn finish(&mut self) {
        self.closing = true;
        chukcut_engine::modules::export::commands::export_shutdown();
        project_commands::project_close(&self.state, true);
    }

    fn finish_and_quit(&mut self, cx: &mut Context<Self>) {
        self.finish();
        cx.quit();
    }

    fn on_new_project(&mut self, _: &NewProject, window: &mut Window, cx: &mut Context<Self>) {
        match self.view {
            View::Home(ref home) => home.update(cx, |home, cx| home.create(cx)),
            View::Editor(ref editor) => {
                editor.update(cx, |editor, cx| editor.request_home(window, cx));
            }
        }
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.view {
            View::Home(home) => home.clone().into_any_element(),
            View::Editor(editor) => editor.clone().into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .key_context("Shell")
            .size_full()
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                // Live changes go to the editor, when one is open.
                let editor = this.editor().map(|editor| editor.downgrade());
                super::settings::open(editor, window, cx);
            }))
            .on_action(cx.listener(|_, _: &ShowShortcuts, window, cx| {
                super::shortcuts::open(window, cx);
            }))
            .on_action(cx.listener(Self::on_new_project))
            // Reached only from the start screen: the editor handles its own.
            .on_action(cx.listener(|this, _: &Open, window, cx| this.browse(window, cx)))
            .child(content)
    }
}
