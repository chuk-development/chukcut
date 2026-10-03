//! Keyframe easing: the menu on a keyframe diamond (in the inspector and on
//! the timeline) and the small graph editor under the Transform section.
//!
//! An easing belongs to the keyframe a move starts at: it shapes the way to
//! the next keyframe (`project::Easing`). The menu offers every named curve;
//! the graph shows the move the playhead is in and lets its two Bézier
//! handles be dragged, which stores a custom `Easing::Bezier`. Both write
//! `EditCommand::SetKeyframeEasing`, one per animated property of the row, as
//! one undo step.

use std::cell::Cell;
use std::rc::Rc;

use chukcut_engine::modules::project::{AnimatableProperty, Easing};
use gpui::component::button::Button;
use gpui::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui::component::Sizable as _;
use gpui::{canvas, point, AnyElement, DispatchPhase, PathBuilder, WeakEntity};

use super::controls::*;
use super::grading::{disc, ring};
use super::*;

/// Height of the graph, in pixels.
const GRAPH_H: f32 = 140.0;
const INSET: f32 = 10.0;
const GRAB_PX: f32 = 10.0;
/// The graph's vertical range: room for an overshoot above 1 and below 0.
const Y_LO: f32 = -0.35;
const Y_HI: f32 = 1.35;

/// Which keyframes an easing edit is aimed at: the ones at `time` (relative
/// to the clip) on each of `properties`.
#[derive(Clone, Debug)]
pub(crate) struct EasingTarget {
    pub segment_id: String,
    pub time: Micros,
    pub properties: Vec<AnimatableProperty>,
}

#[derive(Default)]
pub(crate) struct EasingState {
    /// The row whose move the graph shows.
    prop: Option<Prop>,
    drag: Option<HandleDrag>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

struct HandleDrag {
    /// 0 is the handle leaving the first keyframe, 1 the one arriving.
    handle: usize,
    handles: [f32; 4],
    target: EasingTarget,
}

/// The rows the graph can show, in panel order, with their chip label.
const ROWS: [(Prop, &str); 4] = [
    (Prop::Scale, "Scale"),
    (Prop::PosX, "Position"),
    (Prop::Rotation, "Rotate"),
    (Prop::Opacity, "Opacity"),
];

/// The document properties one row's easing applies to: a row that edits two
/// properties (scale, position) eases both alike.
fn properties_of(prop: Prop) -> Vec<AnimatableProperty> {
    use AnimatableProperty as A;
    match prop {
        Prop::Scale | Prop::ScaleX | Prop::ScaleY => vec![A::ScaleX, A::ScaleY],
        Prop::PosX | Prop::PosY => vec![A::PositionX, A::PositionY],
        _ => prop.animated().to_vec(),
    }
}

/// Add the easing entries to `menu`: every named easing, the current one
/// checked, a drawn one shown as "Custom".
pub(crate) fn easing_items(
    mut menu: PopupMenu,
    editor: WeakEntity<Editor>,
    target: EasingTarget,
    current: Easing,
) -> PopupMenu {
    for easing in Easing::presets() {
        let editor = editor.clone();
        let target = target.clone();
        menu = menu.item(
            PopupMenuItem::new(easing.label())
                .checked(easing == current)
                .on_click(move |_, _, cx| {
                    let target = target.clone();
                    let _ =
                        editor.update(cx, |this, cx| this.set_keyframe_easing(&target, easing, cx));
                }),
        );
    }
    if matches!(current, Easing::Bezier { .. }) {
        menu = menu.item(PopupMenuItem::new("Custom").checked(true).disabled(true));
    }
    menu
}

/// The timeline's right-click menu, with an "Easing" submenu for a keyframe.
pub(crate) fn easing_submenu(
    menu: PopupMenu,
    editor: WeakEntity<Editor>,
    target: EasingTarget,
    current: Easing,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    menu.separator()
        .submenu("Keyframe easing", window, cx, move |menu, _, _| {
            easing_items(menu, editor.clone(), target.clone(), current)
        })
}

impl Editor {
    /// The keyframes the easing of `prop`'s row is edited on: the one at the
    /// playhead, or else the one the playhead is moving away from (the last
    /// keyframe before it, or the first after it when the playhead is before
    /// them all). With the easing it has now, and the time of the next
    /// keyframe when there is one.
    pub(crate) fn easing_target(
        &self,
        prop: Prop,
        segment: &Segment,
    ) -> Option<(EasingTarget, Easing, Option<Micros>)> {
        let properties = properties_of(prop);
        let track = segment
            .keyframes
            .iter()
            .find(|t| properties.contains(&t.property))?;
        let rel = self.clock.position() - segment.target_range.start;
        let tolerance = self.keyframe_tolerance();
        let keys = &track.keyframes;
        let index = keys
            .iter()
            .position(|k| (k.time - rel).abs() <= tolerance)
            .or_else(|| keys.iter().rposition(|k| k.time < rel))
            .unwrap_or(0);
        let key = keys.get(index)?;
        let next = keys.get(index + 1).map(|k| k.time);
        let properties = segment
            .keyframes
            .iter()
            .filter(|t| properties.contains(&t.property))
            .filter(|t| t.keyframes.iter().any(|k| k.time == key.time))
            .map(|t| t.property)
            .collect();
        Some((
            EasingTarget {
                segment_id: segment.id.clone(),
                time: key.time,
                properties,
            },
            key.easing,
            next,
        ))
    }

    fn easing_command(
        project: &Project,
        target: &EasingTarget,
        easing: Easing,
    ) -> Option<EditCommand> {
        let (_, segment) = project.segment(&target.segment_id)?;
        let commands: Vec<EditCommand> = segment
            .keyframes
            .iter()
            .filter(|t| target.properties.contains(&t.property))
            .filter_map(|t| {
                let key = t.keyframes.iter().find(|k| k.time == target.time)?;
                (key.easing != easing).then(|| EditCommand::SetKeyframeEasing {
                    segment_id: target.segment_id.clone(),
                    property: t.property,
                    time: target.time,
                    before: key.easing,
                    after: easing,
                })
            })
            .collect();
        match commands.len() {
            0 => None,
            1 => commands.into_iter().next(),
            _ => Some(EditCommand::Composite {
                label: "Change easing".into(),
                commands,
            }),
        }
    }

    /// Give the keyframes of `target` the easing `easing`, as one undo step.
    pub(crate) fn set_keyframe_easing(
        &mut self,
        target: &EasingTarget,
        easing: Easing,
        cx: &mut Context<Self>,
    ) {
        if let Some(command) = Self::easing_command(&self.project, target, easing) {
            self.apply(Ok(command), cx);
        }
    }

    /// The Keyframe easing section of the Video tab: a chip per animated row,
    /// the graph of the move the playhead is in, and the easing menu. `None`
    /// when the clip has no keyframes on those rows.
    pub(super) fn easing_section(
        &mut self,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let rows: Vec<(Prop, &'static str)> = ROWS
            .into_iter()
            .filter(|(prop, _)| {
                let properties = properties_of(*prop);
                segment
                    .keyframes
                    .iter()
                    .any(|t| properties.contains(&t.property))
            })
            .collect();
        if rows.is_empty() {
            return None;
        }
        let prop = self
            .inspector
            .easing
            .prop
            .filter(|p| rows.iter().any(|(r, _)| r == p))
            .unwrap_or(rows[0].0);

        let chips = div()
            .flex()
            .flex_row()
            .gap(px(6.0))
            .children(rows.iter().enumerate().map(|(i, &(row, label))| {
                let on = row == prop;
                div()
                    .id(("easing-row", i))
                    .px(px(8.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .rounded(px(R_SM))
                    .text_size(px(TEXT_CAPTION))
                    .cursor_pointer()
                    .when(on, |d| d.bg(accent_soft()).text_color(rgb(ACCENT)))
                    .when(!on, |d| {
                        d.bg(rgb(WELL))
                            .text_color(rgb(TEXT_DIM))
                            .hover(|d| d.text_color(rgb(TEXT)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.inspector.easing.prop = Some(row);
                        cx.notify();
                    }))
                    .child(label)
            }));

        let mut rows_out = vec![chips.into_any_element()];
        match self.easing_target(prop, segment) {
            Some((target, current, Some(next))) => {
                let shown = match &self.inspector.easing.drag {
                    Some(drag) => {
                        let [x1, y1, x2, y2] = drag.handles;
                        Easing::Bezier { x1, y1, x2, y2 }
                    }
                    None => current,
                };
                rows_out.push(self.easing_graph(shown, target.clone(), cx));
                let entity = cx.entity().downgrade();
                let menu_target = target.clone();
                let picker = Button::new("easing-picker")
                    .label(current.label())
                    .small()
                    .dropdown_caret(true)
                    .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
                        easing_items(menu, entity.clone(), menu_target.clone(), current)
                    });
                rows_out.push(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(TEXT_CAPTION))
                                .font_family(FONT_MONO)
                                .text_color(rgb(TEXT_MUTED))
                                .child(format!(
                                    "{:.2}s → {:.2}s",
                                    target.time as f64 / 1e6,
                                    next as f64 / 1e6
                                )),
                        )
                        .child(picker)
                        .into_any_element(),
                );
                rows_out.push(
                    div()
                        .text_size(px(TEXT_CAPTION))
                        .text_color(rgb(TEXT_MUTED))
                        .child(
                            "Drag a handle to shape the move. Right-click a diamond for the list.",
                        )
                        .into_any_element(),
                );
            }
            _ => rows_out.push(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Put the playhead between two keyframes to shape the move.")
                    .into_any_element(),
            ),
        }
        Some(Section::new("Keyframe easing").render(
            self.collapsed("Keyframe easing"),
            rows_out,
            cx,
        ))
    }

    fn easing_graph(
        &mut self,
        easing: Easing,
        target: EasingTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bounds = Rc::clone(&self.inspector.easing.bounds);
        let entity = cx.entity().downgrade();
        let handles = easing.handles();
        let show_handles = !matches!(easing, Easing::Hold | Easing::Linear);
        div()
            .id("easing-graph")
            .w_full()
            .h(px(GRAPH_H))
            .rounded(px(R_SM))
            .bg(rgb(WELL))
            .border_1()
            .border_color(rgb(BORDER))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.easing_press(event.position, easing, target.clone(), cx)
                }),
            )
            .child(
                canvas(
                    move |b, _, _| bounds.set(b),
                    move |b, _, window, _| {
                        paint_easing(window, b, easing, handles, show_handles);
                        let up = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = entity.update(cx, |this, cx| {
                                    if this.inspector.easing.drag.is_some() {
                                        this.easing_move(event.position, cx)
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = up.update(cx, |this, cx| this.easing_release(cx));
                            }
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    /// A window point in the graph's units: time across, progress up.
    fn easing_local(&self, position: Point<Pixels>) -> (f32, f32) {
        let (ox, oy, w, h) = graph_plot(self.inspector.easing.bounds.get());
        let x = (f32::from(position.x) - ox) / w;
        let v = 1.0 - (f32::from(position.y) - oy) / h;
        (x, Y_LO + v * (Y_HI - Y_LO))
    }

    fn easing_press(
        &mut self,
        position: Point<Pixels>,
        easing: Easing,
        target: EasingTarget,
        cx: &mut Context<Self>,
    ) {
        let handles = easing.handles();
        let (x, y) = self.easing_local(position);
        let (_, _, w, h) = graph_plot(self.inspector.easing.bounds.get());
        let scale_y = h / (Y_HI - Y_LO);
        let distance =
            |hx: f32, hy: f32| (((hx - x) * w).powi(2) + ((hy - y) * scale_y).powi(2)).sqrt();
        let d0 = distance(handles[0], handles[1]);
        let d1 = distance(handles[2], handles[3]);
        let handle = if d0 <= d1 { 0 } else { 1 };
        if d0.min(d1) > GRAB_PX * 2.0 {
            return;
        }
        self.inspector.easing.drag = Some(HandleDrag {
            handle,
            handles,
            target,
        });
        self.easing_move(position, cx);
    }

    fn easing_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mut drag) = self.inspector.easing.drag.take() else {
            return;
        };
        let (x, y) = self.easing_local(position);
        let at = drag.handle * 2;
        drag.handles[at] = (x.clamp(0.0, 1.0) * 100.0).round() / 100.0;
        drag.handles[at + 1] = (y.clamp(-1.0, 2.0) * 100.0).round() / 100.0;
        let [x1, y1, x2, y2] = drag.handles;
        let easing = Easing::Bezier { x1, y1, x2, y2 };
        let target = drag.target.clone();
        self.inspector.easing.drag = Some(drag);

        let base = match &self.inspector.preview {
            Some(preview) if preview.prop == Prop::Easing => Arc::clone(&preview.base),
            _ => {
                let base = Arc::clone(&self.project);
                self.inspector.preview = Some(Preview {
                    prop: Prop::Easing,
                    base: Arc::clone(&base),
                });
                base
            }
        };
        if let Some(command) = Self::easing_command(&base, &target, easing) {
            let mut copy = (*base).clone();
            if command.apply(&mut copy).is_ok() {
                self.project = Arc::new(copy);
                self.generation += 1;
            }
        }
        cx.notify();
    }

    fn easing_release(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.inspector.easing.drag.take() else {
            return;
        };
        if let Some(preview) = self.inspector.preview.take() {
            self.project = preview.base;
            self.generation += 1;
        }
        let [x1, y1, x2, y2] = drag.handles;
        self.set_keyframe_easing(&drag.target, Easing::Bezier { x1, y1, x2, y2 }, cx);
    }
}

fn graph_plot(b: Bounds<Pixels>) -> (f32, f32, f32, f32) {
    (
        f32::from(b.origin.x) + INSET,
        f32::from(b.origin.y) + INSET,
        (f32::from(b.size.width) - 2.0 * INSET).max(1.0),
        (f32::from(b.size.height) - 2.0 * INSET).max(1.0),
    )
}

fn paint_easing(
    window: &mut Window,
    b: Bounds<Pixels>,
    easing: Easing,
    handles: [f32; 4],
    show_handles: bool,
) {
    let (ox, oy, w, h) = graph_plot(b);
    let at = |x: f32, y: f32| {
        let v = (y - Y_LO) / (Y_HI - Y_LO);
        point(px(ox + x * w), px(oy + (1.0 - v) * h))
    };
    let stroke = |window: &mut Window, a: (f32, f32), z: (f32, f32), colour: u32, width: f32| {
        let mut path = PathBuilder::stroke(px(width));
        path.move_to(at(a.0, a.1));
        path.line_to(at(z.0, z.1));
        if let Ok(path) = path.build() {
            window.paint_path(path, rgb(colour));
        }
    };
    stroke(window, (0.0, 0.0), (1.0, 0.0), HAIRLINE, 1.0);
    stroke(window, (0.0, 1.0), (1.0, 1.0), HAIRLINE, 1.0);
    stroke(window, (0.0, 0.0), (1.0, 1.0), BORDER, 1.0);

    let mut trace = PathBuilder::stroke(px(2.0));
    for i in 0..=120 {
        let t = i as f32 / 120.0;
        let p = at(t, easing.apply(t));
        if i == 0 {
            trace.move_to(p);
        } else {
            trace.line_to(p);
        }
    }
    if let Ok(path) = trace.build() {
        window.paint_path(path, rgb(ACCENT));
    }
    if show_handles {
        let [x1, y1, x2, y2] = handles;
        stroke(window, (0.0, 0.0), (x1, y1), TEXT_MUTED, 1.0);
        stroke(window, (1.0, 1.0), (x2, y2), TEXT_MUTED, 1.0);
        for (x, y) in [(x1, y1), (x2, y2)] {
            let centre = at(x, y);
            window.paint_quad(disc(centre, 5.0, rgb(WELL)));
            window.paint_quad(ring(centre, 5.0, rgb(TEXT), 1.5));
        }
    }
    window.paint_quad(disc(at(0.0, 0.0), 3.0, rgb(ACCENT)));
    window.paint_quad(disc(at(1.0, 1.0), 3.0, rgb(ACCENT)));
}
