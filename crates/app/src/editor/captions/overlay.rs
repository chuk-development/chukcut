//! Moving captions by dragging them on the player.
//!
//! When the selected clip is a caption, a frame is drawn around it on the
//! picture. Dragging the frame moves the caption — or every caption, when the
//! style panel applies to all, which is what a creator moving subtitles out of
//! the way wants. The picture follows the pointer live, from a preview copy of
//! the document; releasing commits one undoable `SetTransform` per caption.

use chukcut_engine::modules::project::{Project, Transform};
use chukcut_engine::modules::text::{RasterOptions, TextRenderer, TextRequest};
use gpui::{AnyElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Point};

use super::*;

/// A drag in progress.
pub(crate) struct CaptionDrag {
    start: Point<Pixels>,
    /// The position the dragged caption started at.
    origin: [f32; 2],
    /// Every caption that moves, with the transform it had.
    moving: Vec<(String, Transform)>,
    /// Picture size in window pixels, for converting the pointer's motion.
    picture: (f32, f32),
    last: [f32; 2],
}

/// Pointer motion within this many normalised units of the centre line snaps
/// to it, so "centred" is easy to hit.
const SNAP: f32 = 0.02;

impl Editor {
    /// The frame and the drag surface, laid over the viewer area. `None` when
    /// the selected clip is not a caption.
    pub(crate) fn caption_overlay(
        &self,
        picture: (f32, f32),
        viewer: (f32, f32),
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = self.selected_caption()?;
        let (_, segment) = self.project.segment(&id)?;
        let material = self.project.materials.text(&segment.material_id)?;
        let time = self.clock.position();
        if !segment.target_range.contains(time) && self.captions.drag.is_none() {
            return None;
        }
        let (dw, dh) = picture;
        let (cw, ch) = (
            self.project.canvas.width.max(1) as f32,
            self.project.canvas.height.max(1) as f32,
        );
        let layout = TextRenderer::shared().layout(
            &TextRequest::from(material),
            &RasterOptions::canvas(cw as u32, ch as u32),
        );
        if layout.is_empty() {
            return None;
        }
        let fit = dw / cw;
        let t = segment.transform;
        let pad = 6.0;
        let w = layout.width * fit * t.scale[0].abs() + pad * 2.0;
        let h = layout.height * fit * t.scale[1].abs() + pad * 2.0;
        // The picture is centred in the viewer; the caption's centre is the
        // picture's centre moved by the normalised position, +y up.
        let left_of_picture = (viewer.0 - dw) / 2.0;
        let top_of_picture = (viewer.1 - dh) / 2.0;
        let cx_px = left_of_picture + dw / 2.0 + t.position[0] * dw / 2.0;
        let cy_px = top_of_picture + dh / 2.0 - t.position[1] * dh / 2.0;

        let frame = div()
            .id("caption-frame")
            .absolute()
            .left(px(cx_px - w / 2.0))
            .top(px(cy_px - h / 2.0))
            .w(px(w))
            .h(px(h))
            .border_1()
            .border_color(rgb(ACCENT))
            .rounded_sm()
            .cursor_move()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.begin_caption_drag(event.position, picture, cx);
                    cx.stop_propagation();
                }),
            );

        Some(
            div()
                .id("caption-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .when(self.captions.drag.is_some(), |layer| {
                    layer
                        .cursor_move()
                        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                            this.move_caption_drag(event.position, cx)
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_caption_drag(cx)),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_caption_drag(cx)),
                        )
                })
                .child(frame)
                .into_any_element(),
        )
    }

    fn begin_caption_drag(
        &mut self,
        at: Point<Pixels>,
        picture: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_caption() else {
            return;
        };
        let Some((_, segment)) = self.project.segment(&id) else {
            return;
        };
        let origin = segment.transform.position;
        let ids: Vec<String> = if self.captions.style_all {
            caption_edit::clips(&self.project)
                .into_iter()
                .map(|c| c.segment_id)
                .collect()
        } else {
            vec![id]
        };
        let moving = ids
            .into_iter()
            .filter_map(|id| {
                let (_, s) = self.project.segment(&id)?;
                Some((id, s.transform))
            })
            .collect();
        self.pause();
        self.captions.drag = Some(CaptionDrag {
            start: at,
            origin,
            moving,
            picture,
            last: origin,
        });
        cx.notify();
    }

    fn move_caption_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.captions.drag.as_mut() else {
            return;
        };
        let dx = f32::from(at.x - drag.start.x) / (drag.picture.0 / 2.0).max(1.0);
        let dy = f32::from(at.y - drag.start.y) / (drag.picture.1 / 2.0).max(1.0);
        let mut position = [drag.origin[0] + dx, drag.origin[1] - dy];
        if position[0].abs() < SNAP {
            position[0] = 0.0;
        }
        position[0] = position[0].clamp(-1.2, 1.2);
        position[1] = position[1].clamp(-1.2, 1.2);
        if position == drag.last {
            return;
        }
        drag.last = position;
        let delta = [position[0] - drag.origin[0], position[1] - drag.origin[1]];

        // A preview document: the snapshot with the captions moved. Nothing is
        // committed until the button comes up.
        let mut preview: Project = (*self.project).clone();
        for (id, before) in &drag.moving {
            if let Some(segment) = preview.segment_mut(id) {
                segment.transform.position =
                    [before.position[0] + delta[0], before.position[1] + delta[1]];
            }
        }
        self.project = Arc::new(preview);
        self.generation += 1;
        cx.notify();
    }

    fn end_caption_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.captions.drag.take() else {
            return;
        };
        let delta = [drag.last[0] - drag.origin[0], drag.last[1] - drag.origin[1]];
        if delta == [0.0, 0.0] {
            cx.notify();
            return;
        }
        let commands = drag
            .moving
            .into_iter()
            .map(|(segment_id, before)| EditCommand::SetTransform {
                segment_id,
                before,
                after: Transform {
                    position: [before.position[0] + delta[0], before.position[1] + delta[1]],
                    ..before
                },
            })
            .collect();
        self.apply(
            Ok(EditCommand::Composite {
                label: "Move captions".into(),
                commands,
            }),
            cx,
        );
    }
}
