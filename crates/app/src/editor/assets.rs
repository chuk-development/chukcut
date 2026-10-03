//! The asset panel on the left: media, audio, text, transitions …

use super::*;

impl Editor {
    pub(super) fn render_media(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pool = &self.project.materials;
        let items: Vec<(String, String, Micros)> = pool
            .videos
            .iter()
            .map(|m| (m.id.clone(), file_name(&m.path), m.duration))
            .chain(
                pool.images
                    .iter()
                    .map(|m| (m.id.clone(), file_name(&m.path), 0)),
            )
            .chain(
                pool.audios
                    .iter()
                    .map(|m| (m.id.clone(), file_name(&m.path), m.duration)),
            )
            .collect();

        let list = items
            .into_iter()
            .enumerate()
            .map(|(index, (id, name, duration))| {
                let detail = if duration > 0 {
                    timecode(duration, 0.0)
                } else {
                    "still".into()
                };
                div()
                    .id(("media", index))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(PANEL_RAISED))
                    .hover(|style| style.bg(rgb(BORDER)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let command = edits::append(&this.project, &id);
                        this.apply(command, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child(name),
                    )
                    .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(detail))
                    .child(div().text_color(rgb(ACCENT)).child("+"))
            });

        div()
            .w(px(MEDIA_W))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .pb_1()
                    .child(div().text_sm().text_color(rgb(TEXT)).child("Media"))
                    .child(button("import-media", "+ Import", cx.listener(|this, _, w, cx| this.on_import(&Import, w, cx)))),
            )
            .children(list)
            .when(self.project.materials.videos.is_empty()
                && self.project.materials.images.is_empty()
                && self.project.materials.audios.is_empty(), |panel| {
                panel.child(
                    div()
                        .pt_4()
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child("Import video, images or audio (Ctrl+I). Click an item to add it to the timeline."),
                )
            })
    }
}
