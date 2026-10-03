//! The bar across the top: project name, file actions, Export.

use super::*;

impl Editor {
    pub(super) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.project.name.clone();
        let saved = self
            .state
            .project_path
            .read()
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());
        div()
            .h(px(40.0))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_color(rgb(ACCENT))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("chukcut"),
            )
            .child(button(
                "import",
                "Import",
                cx.listener(|this, _, w, cx| this.on_import(&Import, w, cx)),
            ))
            .child(button(
                "open",
                "Open",
                cx.listener(|this, _, w, cx| this.on_open(&Open, w, cx)),
            ))
            .child(button(
                "save",
                "Save",
                cx.listener(|this, _, w, cx| this.on_save(&Save, w, cx)),
            ))
            .child(button(
                "export",
                "Export",
                cx.listener(|this, _, w, cx| this.on_export(&Export, w, cx)),
            ))
            .child(div().flex_1())
            .children(
                self.status
                    .clone()
                    .map(|status| div().text_xs().text_color(rgb(TEXT_DIM)).child(status)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child(saved.map(|p| file_name(&p)).unwrap_or(name)),
            )
    }
}
