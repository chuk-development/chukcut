//! The player in the middle: the canvas and the transport.

use super::*;

impl Editor {
    pub(super) fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let viewer = Rc::clone(&self.viewer);
        let (cw, ch) = (
            self.project.canvas.width as f32,
            self.project.canvas.height as f32,
        );
        let bounds = self.viewer.get();
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let fit = if cw > 0.0 && ch > 0.0 {
            (bw / cw).min(bh / ch)
        } else {
            0.0
        };
        let (dw, dh) = (cw * fit, ch * fit);

        let picture = match (&self.frame, self.player.failure()) {
            (_, Some(failure)) => div()
                .text_color(rgb(TEXT_DIM))
                .child(failure)
                .into_any_element(),
            (Some(frame), None) => img(Arc::clone(frame))
                .w(px(dw))
                .h(px(dh))
                .into_any_element(),
            (None, None) => div()
                .w(px(dw))
                .h(px(dh))
                .bg(rgb(0x000000))
                .into_any_element(),
        };

        let playing = self.clock.is_playing();
        let position = self.clock.position();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .child(
                div()
                    .flex_1()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .m_3()
                    .child(
                        canvas(move |bounds, _, _| viewer.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(picture),
            )
            .child(
                div()
                    .h(px(40.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child(timecode(position, self.project.fps)),
                    )
                    .child(button(
                        "play",
                        if playing { "Pause" } else { "Play" },
                        cx.listener(|this, _, w, cx| this.on_play_pause(&PlayPause, w, cx)),
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(TEXT_DIM))
                            .child(timecode(self.project.duration(), self.project.fps)),
                    )
                    .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(format!(
                        "{}×{} · {:.2} fps",
                        self.project.canvas.width, self.project.canvas.height, self.project.fps
                    ))),
            )
    }
}
