//! The bar across the top, laid out like CapCut's: logo and the Menu on the
//! left with the save state next to them, the project name centred, the
//! accent Export button on the right.

use std::time::Instant;

use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::{Icon, Sizable as _};

use super::*;
use crate::ui::{icons, Badge, IconSrc};

/// What the title bar remembers between frames: enough to say whether the
/// document on screen is the one on disk.
#[derive(Default)]
pub(crate) struct TitleState {
    /// The generation seen on the previous frame, to notice an edit.
    seen_generation: u64,
    /// When the last edit happened. The engine's autosave writes the working
    /// copy a moment after every edit, so this is also when it was autosaved.
    last_edit: Option<Instant>,
}

impl TitleState {
    /// Called after a successful save to the project file.
    pub(crate) fn mark_saved(&mut self, generation: u64) {
        self.seen_generation = generation;
    }

    fn observe(&mut self, generation: u64) {
        if generation != self.seen_generation {
            self.seen_generation = generation;
            self.last_edit = Some(Instant::now());
        }
    }
}

/// "30", "29.97", "23.976": whole rates without decimals.
fn fps_label(fps: f64) -> String {
    if (fps - fps.round()).abs() < 0.005 {
        format!("{}", fps.round() as i64)
    } else {
        format!("{fps:.2}").trim_end_matches('0').to_string()
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
            (Some(_), _) if !self.is_dirty() => (Some(Lucide::CircleCheck), "Saved".to_string()),
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
                    .min_w(px(220.0))
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
                    .menu("Settings…", Box::new(OpenSettings))
                    .menu("Keyboard shortcuts", Box::new(ShowShortcuts))
                    .separator()
                    .menu("Quit", Box::new(Quit))
            });

        // Our mark on a raised tile, then the word mark.
        let logo = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .size(px(24.0))
                    .rounded(px(R_SM + 1.0))
                    .bg(rgb(PANEL_RAISED))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icons::glyph(icons::LOGO, 16.0, rgb(ACCENT))),
            )
            .child(
                div()
                    .text_size(px(TEXT_BODY + 1.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .child("chukcut"),
            );

        let save_state = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .text_size(px(TEXT_CAPTION + 1.0))
            .text_color(rgb(TEXT_MUTED))
            .children(saved_icon.map(|icon| IconSrc::from(icon).svg(13.0, rgb(SUCCESS))))
            .child(saved_label);

        let divider = || div().w(px(1.0)).h(px(16.0)).bg(rgb(BORDER));

        let canvas = &self.project.canvas;
        let format = format!(
            "{}×{} · {} fps",
            canvas.width,
            canvas.height,
            fps_label(self.project.fps)
        );

        let export = Button::new("export")
            .primary()
            .small()
            .icon(Icon::default().data(icons::EXPORT.0))
            .label("Export")
            .tooltip_with_action("Export the timeline", &Export, Some("Editor"))
            .on_click(cx.listener(|this, _, window, cx| this.on_export(&Export, window, cx)));

        div()
            .h(px(TITLE_BAR_H))
            .flex_none()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .pl(px(PAD))
            .pr(px(GUTTER + 2.0))
            .bg(rgb(BG))
            .child(logo)
            .child(divider())
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
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .child(name),
                    )
                    .child(Badge::new(format).mono()),
            )
            .child(div().flex_1())
            .children(self.status.clone().map(|status| {
                div()
                    .max_w(px(420.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(TEXT_CAPTION + 1.0))
                    .text_color(rgb(TEXT_DIM))
                    .child(status)
            }))
            .child(export)
    }

    /// Menu → New project: back to the start screen, which asks for the
    /// canvas — after the unsaved-changes guard.
    pub(super) fn on_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_home(window, cx);
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
            let _ = this.update(cx, |editor, cx| editor.save_to(Some(path), cx));
        })
        .detach();
    }
}
