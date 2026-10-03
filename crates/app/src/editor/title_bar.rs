//! The bar across the top, laid out like CapCut's: logo and the Menu on the
//! left with the save state next to them, the project name centred, the
//! accent Export button on the right.

use std::time::Instant;

use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::{Icon, Sizable as _};

use super::*;

/// What the title bar remembers between frames: enough to say whether the
/// document on screen is the one on disk.
#[derive(Default)]
pub(crate) struct TitleState {
    /// The edit generation that was last written to the project file.
    saved_generation: Option<u64>,
    /// The generation seen on the previous frame, to notice an edit.
    seen_generation: u64,
    /// When the last edit happened. The engine's autosave writes the working
    /// copy a moment after every edit, so this is also when it was autosaved.
    last_edit: Option<Instant>,
}

impl TitleState {
    /// Called after a successful save to the project file.
    pub(crate) fn mark_saved(&mut self, generation: u64) {
        self.saved_generation = Some(generation);
        self.seen_generation = generation;
    }

    fn observe(&mut self, generation: u64) {
        if generation != self.seen_generation {
            self.seen_generation = generation;
            self.last_edit = Some(Instant::now());
        }
    }
}

/// "just now", "3 min ago", "2 h ago".
fn ago(since: Instant) -> String {
    let seconds = since.elapsed().as_secs();
    match seconds {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", seconds / 60),
        _ => format!("{} h ago", seconds / 3600),
    }
}

impl Editor {
    pub(super) fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.title.observe(self.generation);
        let name = self.project.name.clone();
        let saved_path = self.state.project_path.read().clone();

        let (saved_icon, saved_label) = match (&saved_path, self.title.last_edit) {
            (Some(_), _) if self.title.saved_generation == Some(self.generation) => {
                (Some(Lucide::CircleCheck), "Saved".to_string())
            }
            (_, Some(edited)) => (
                Some(Lucide::CloudCheck),
                format!("Autosaved {}", ago(edited)),
            ),
            (Some(_), None) => (Some(Lucide::CircleCheck), "Saved".to_string()),
            (None, None) => (None, String::new()),
        };

        let focus = self.focus.clone();
        let editor = cx.entity().downgrade();
        let menu = Button::new("app-menu")
            .label("Menu")
            .ghost()
            .small()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let new_project = editor.clone();
                let save_as = editor.clone();
                menu.action_context(focus.clone())
                    .min_w(px(200.0))
                    .item(
                        PopupMenuItem::new("New project").on_click(move |_, window, cx| {
                            let _ = new_project
                                .update(cx, |editor, cx| editor.on_new_project(window, cx));
                        }),
                    )
                    .menu("Open…", Box::new(Open))
                    .separator()
                    .menu("Save", Box::new(Save))
                    .item(PopupMenuItem::new("Save as…").on_click(move |_, _, cx| {
                        let _ = save_as.update(cx, |editor, cx| editor.on_save_as(cx));
                    }))
                    .separator()
                    .menu("Import media…", Box::new(Import))
                    .menu("Export…", Box::new(Export))
                    .separator()
                    .menu("Quit", Box::new(Quit))
            });

        let logo = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1p5()
            .child(
                div()
                    .size(px(20.0))
                    .rounded(px(5.0))
                    .bg(rgb(ACCENT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(Lucide::Scissors)
                            .size(px(13.0))
                            .text_color(rgb(0x0b1214)),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(TEXT))
                    .child("chukcut"),
            );

        let save_state = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .text_xs()
            .text_color(rgb(TEXT_DIM))
            .children(saved_icon.map(|icon| Icon::new(icon).size(px(13.0)).text_color(rgb(ACCENT))))
            .child(saved_label);

        let export = Button::new("export")
            .primary()
            .small()
            .icon(Lucide::Upload)
            .label("Export")
            .on_click(cx.listener(|this, _, window, cx| this.on_export(&Export, window, cx)));

        div()
            .h(px(38.0))
            .flex_none()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_3()
            .bg(rgb(BG))
            .child(logo)
            .child(menu)
            .child(save_state)
            // The project name sits on its own layer so it stays centred on
            // the window whatever the left and right groups measure.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child(name),
            )
            .child(div().flex_1())
            .children(self.status.clone().map(|status| {
                div()
                    .max_w(px(420.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_xs()
                    .text_color(rgb(TEXT_DIM))
                    .child(status)
            }))
            .child(export)
    }

    /// Menu → New project: an empty 9:16 project, like a fresh start.
    pub(super) fn on_new_project(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.pause();
        let result =
            project_commands::project_new(&self.state, "Untitled".into(), 1080, 1920, 30.0)
                .map(|_| ());
        self.selected = None;
        self.clock.seek(0);
        self.title = TitleState::default();
        self.refresh(cx);
        // The fresh project is not an edit; do not call it autosaved.
        self.title.seen_generation = self.generation;
        self.report(result, cx);
    }

    /// Menu → Save as: always asks for a path, even for a saved project.
    pub(super) fn on_save_as(&mut self, cx: &mut Context<Self>) {
        let directory = self
            .state
            .project_path
            .read()
            .as_ref()
            .and_then(|path| path.parent().map(PathBuf::from))
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        let name = format!("{}.chukcut", self.project.name);
        let picked = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(path))) => path,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |editor, cx| editor.dialog_failed(error, cx));
                    return;
                }
                _ => return,
            };
            let _ = this.update(cx, |editor, cx| {
                let result = project_commands::project_save(
                    &editor.state,
                    Some(path.to_string_lossy().to_string()),
                );
                editor.status = Some(match result {
                    Ok(path) => {
                        editor.title.mark_saved(editor.generation);
                        format!("Saved {path}").into()
                    }
                    Err(error) => error.into(),
                });
                cx.notify();
            });
        })
        .detach();
    }
}
