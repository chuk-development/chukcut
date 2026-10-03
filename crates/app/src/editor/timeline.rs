//! The timeline: toolbar, ruler, tracks, clips, playhead, and the pointer.

use super::*;

impl Editor {
    // --- timeline geometry ----------------------------------------------------------

    pub(super) fn time_to_x(&self, time: Micros) -> f32 {
        HEADER_W + time as f32 / 1_000_000.0 * self.zoom - self.scroll_x
    }

    pub(super) fn x_to_time(&self, x: f32) -> Micros {
        (((x - HEADER_W + self.scroll_x) / self.zoom) * 1_000_000.0).max(0.0) as Micros
    }

    /// Timeline-local coordinates of a window position.
    pub(super) fn local(&self, position: Point<Pixels>) -> (f32, f32) {
        let bounds = self.timeline.get();
        (
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        )
    }

    /// The track under a timeline-local y, if any.
    pub(super) fn track_at(&self, y: f32) -> Option<&Track> {
        let mut top = RULER_H;
        for track in &self.project.tracks {
            let height = row_height(track.kind);
            if y >= top && y < top + height {
                return Some(track);
            }
            top += height;
        }
        None
    }

    pub(super) fn on_timeline_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = self.local(event.position);
        let time = self.x_to_time(x);
        if y >= RULER_H && x >= HEADER_W {
            let hit = self.track_at(y).and_then(|track| {
                track
                    .segments
                    .iter()
                    .find(|s| {
                        time >= s.target_range.start
                            && time < s.target_range.start + s.target_range.duration
                    })
                    .map(|s| {
                        (
                            track.id.clone(),
                            track.kind,
                            s.id.clone(),
                            s.target_range.start,
                        )
                    })
            });
            if let Some((track_id, kind, segment_id, start)) = hit {
                self.selected = Some(segment_id.clone());
                self.drag = Some(Drag::Clip {
                    segment_id,
                    kind,
                    grab: time - start,
                    origin_track: track_id.clone(),
                    origin_start: start,
                    track: track_id,
                    start,
                });
                cx.notify();
                return;
            }
            self.selected = None;
        }
        if x >= HEADER_W {
            self.seek(time);
            self.drag = Some(Drag::Scrub);
        }
        cx.notify();
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drag.is_none() || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (x, y) = self.local(event.position);
        let time = self.x_to_time(x);
        let hovered = self.track_at(y).map(|t| (t.id.clone(), t.kind));
        match &mut self.drag {
            Some(Drag::Scrub) => {
                self.seek(time);
            }
            Some(Drag::Clip {
                grab,
                kind,
                track,
                start,
                ..
            }) => {
                *start = (time - *grab).max(0);
                if let Some((id, hovered_kind)) = hovered {
                    if hovered_kind == *kind {
                        *track = id;
                    }
                }
            }
            None => {}
        }
        cx.notify();
    }

    pub(super) fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Drag::Clip {
            segment_id,
            origin_track,
            origin_start,
            track,
            start,
            ..
        }) = self.drag.take()
        {
            if track != origin_track || start != origin_start {
                let command = edits::move_to(&self.project, &segment_id, &track, start);
                self.apply(command, cx);
            }
        }
        self.drag = None;
        cx.notify();
    }

    pub(super) fn on_timeline_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(20.0));
        if event.modifiers.control {
            let (x, _) = self.local(event.position);
            let anchor = self.x_to_time(x);
            let factor = if f32::from(delta.y) > 0.0 {
                1.15
            } else {
                1.0 / 1.15
            };
            self.zoom = (self.zoom * factor).clamp(2.0, 2000.0);
            // Keep the time under the pointer where it was.
            self.scroll_x = (anchor as f32 / 1_000_000.0 * self.zoom - (x - HEADER_W)).max(0.0);
        } else {
            let step = if f32::from(delta.x).abs() > f32::from(delta.y).abs() {
                f32::from(delta.x)
            } else {
                f32::from(delta.y)
            };
            self.scroll_x = (self.scroll_x - step).max(0.0);
        }
        cx.notify();
    }

    pub(super) fn render_timeline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let probe = Rc::clone(&self.timeline);
        let width = f32::from(self.timeline.get().size.width).max(400.0);

        // Ruler: a label every `step` seconds, `step` chosen so labels stay
        // at least ~90 px apart.
        let step = [
            0.5_f32, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0,
        ]
        .into_iter()
        .find(|s| s * self.zoom >= 90.0)
        .unwrap_or(1200.0);
        let first = (self.scroll_x / self.zoom / step).floor() as i64;
        let last = ((self.scroll_x + width) / self.zoom / step).ceil() as i64;
        let ticks = (first.max(0)..=last).map(|i| {
            let seconds = i as f32 * step;
            let x = self.time_to_x((seconds * 1_000_000.0) as Micros);
            div()
                .absolute()
                .left(px(x))
                .top(px(0.0))
                .h(px(RULER_H))
                .border_l_1()
                .border_color(rgb(BORDER))
                .pl_1()
                .text_xs()
                .text_color(rgb(TEXT_DIM))
                .child(clock_label(seconds))
        });

        let mut rows = Vec::new();
        let mut clips = Vec::new();
        let mut top = RULER_H;
        for track in &self.project.tracks {
            let height = row_height(track.kind);
            rows.push(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(top))
                    .w_full()
                    .h(px(height))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .w(px(HEADER_W))
                            .h_full()
                            .flex()
                            .items_center()
                            .px_2()
                            .bg(rgb(PANEL))
                            .border_r_1()
                            .border_color(rgb(BORDER))
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .child(track.name.clone()),
                    ),
            );

            for segment in &track.segments {
                // While a clip is being dragged it is drawn where the pointer
                // has it, not where the document has it.
                let (track_top, start) = match &self.drag {
                    Some(Drag::Clip {
                        segment_id,
                        track: to,
                        start,
                        ..
                    }) if *segment_id == segment.id => (self.track_top(to).unwrap_or(top), *start),
                    _ => (top, segment.target_range.start),
                };
                let x = self.time_to_x(start).max(HEADER_W);
                let right = self.time_to_x(start + segment.target_range.duration);
                if right <= HEADER_W {
                    continue;
                }
                let selected = self.selected.as_deref() == Some(segment.id.as_str());
                clips.push(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(track_top + 3.0))
                        .w(px((right - x - 1.0).max(2.0)))
                        .h(px(height - 6.0))
                        .rounded(px(4.0))
                        .bg(rgb(self.clip_color(track.kind, &segment.material_id)))
                        .border_2()
                        .border_color(rgb(if selected { PLAYHEAD } else { 0x000000 }))
                        .overflow_hidden()
                        .px_1()
                        .text_xs()
                        .text_color(rgb(TEXT))
                        .child(self.material_name(&segment.material_id)),
                );
            }
            top += height;
        }

        let playhead_x = self.time_to_x(self.clock.position());
        let playhead = (playhead_x >= HEADER_W).then(|| {
            div()
                .absolute()
                .left(px(playhead_x - 1.0))
                .top(px(0.0))
                .w(px(2.0))
                .h_full()
                .bg(rgb(PLAYHEAD))
        });

        div()
            .h(px(TIMELINE_H))
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(34.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(button(
                        "split",
                        "Split (S)",
                        cx.listener(|this, _, w, cx| this.on_split(&Split, w, cx)),
                    ))
                    .child(button(
                        "delete",
                        "Delete",
                        cx.listener(|this, _, w, cx| this.on_delete(&DeleteSelected, w, cx)),
                    ))
                    .child(button(
                        "undo",
                        "Undo",
                        cx.listener(|this, _, w, cx| this.on_undo(&Undo, w, cx)),
                    ))
                    .child(button(
                        "redo",
                        "Redo",
                        cx.listener(|this, _, w, cx| this.on_redo(&Redo, w, cx)),
                    ))
                    .child(div().flex_1())
                    .child(button(
                        "zoom-out",
                        "−",
                        cx.listener(|this, _, _, cx| this.zoom_by(1.0 / 1.4, cx)),
                    ))
                    .child(button(
                        "zoom-in",
                        "+",
                        cx.listener(|this, _, _, cx| this.zoom_by(1.4, cx)),
                    )),
            )
            .child(
                div()
                    .id("timeline")
                    .flex_1()
                    .relative()
                    .overflow_hidden()
                    .bg(rgb(BG))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_timeline_down))
                    .on_scroll_wheel(cx.listener(Self::on_timeline_scroll))
                    .child(
                        canvas(move |bounds, _, _| probe.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .top(px(0.0))
                            .w_full()
                            .h(px(RULER_H))
                            .bg(rgb(PANEL))
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .children(ticks),
                    )
                    .children(rows)
                    .children(clips)
                    .children(playhead),
            )
    }

    pub(super) fn track_top(&self, track_id: &str) -> Option<f32> {
        let mut top = RULER_H;
        for track in &self.project.tracks {
            if track.id == track_id {
                return Some(top);
            }
            top += row_height(track.kind);
        }
        None
    }
}
