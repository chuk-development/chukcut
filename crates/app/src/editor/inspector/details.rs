//! The inspector on the right: the selected clip's properties, or the
//! project's details when nothing is selected.

use super::*;

impl Editor {
    pub(super) fn render_inspector(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let row = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_row()
                .gap_4()
                .py_1()
                .child(
                    div()
                        .w(px(120.0))
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child(label),
                )
                .child(div().text_xs().text_color(rgb(TEXT)).child(value))
        };
        let path = self
            .state
            .project_path
            .read()
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "not saved yet".into());
        div()
            .w(px(INSPECTOR_W))
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .rounded(px(6.0))
            .child(
                div()
                    .h(px(36.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(BG))
                    .text_sm()
                    .text_color(rgb(ACCENT))
                    .child("Details"),
            )
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .child(row("Name", self.project.name.clone()))
                    .child(row("Path", path))
                    .child(row(
                        "Resolution",
                        format!(
                            "{}×{}",
                            self.project.canvas.width, self.project.canvas.height
                        ),
                    ))
                    .child(row("Frame rate", format!("{:.2} fps", self.project.fps)))
                    .child(row(
                        "Duration",
                        timecode(self.project.duration(), self.project.fps),
                    )),
            )
    }
}
