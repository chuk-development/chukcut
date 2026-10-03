//! The Speed tab's Curve sub-tab: speed-ramp presets and a curve editor.
//!
//! The engine owns the model (`project::speed`): points are source instants
//! with a speed each, the clip's length is the curve's integral. Here the
//! editor shows the clip's own stretch of the curve, its x axis the clip's
//! source range and its y axis the speed on a log scale (0.1x at the bottom,
//! 1x in the middle, 10x at the top), so a slow-down and a speed-up of the
//! same factor look the same size. Every gesture is a
//! `speed::edit::set_curve_command`: previewed on a copy while dragging,
//! written once on release, like a slider.

use std::cell::Cell;
use std::rc::Rc;

use chukcut_engine::modules::project::speed::{
    self as curve_math, SpeedPoint, SpeedPreset, MAX_CURVE_SPEED, MIN_CURVE_SPEED,
};
use chukcut_engine::modules::project::TimeRange;
use chukcut_engine::modules::speed::commands as speed_commands;
use chukcut_engine::modules::speed::edit::{self as speed_edit, CurveChange};
use gpui::{canvas, point, AnyElement, DispatchPhase, PathBuilder};

use super::grading::{disc, ring};
use super::*;

/// Height of the curve editor, in pixels.
const EDITOR_H: f32 = 180.0;
/// Inset of the plot inside the editor, so points on the edges stay whole.
const INSET: f32 = 10.0;
/// How close a press must land to a point to grab it.
const GRAB_PX: f32 = 9.0;
/// A preset tile's thumbnail.
const TILE_W: f32 = 64.0;
const TILE_H: f32 = 44.0;

/// The tab's state between frames.
#[derive(Default)]
pub(crate) struct SpeedTab {
    drag: Option<PointDrag>,
    /// The point last pressed, whose speed the readout shows.
    selected: Option<usize>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

/// A point being dragged, on the drag's own copy of the curve.
struct PointDrag {
    index: usize,
    points: Vec<SpeedPoint>,
}

/// Speed to the editor's vertical unit: `0` at 0.1x, `0.5` at 1x, `1` at 10x.
fn speed_to_v(speed: f64) -> f32 {
    ((speed
        .clamp(MIN_CURVE_SPEED as f64, MAX_CURVE_SPEED as f64)
        .log10()
        + 1.0)
        / 2.0) as f32
}

fn v_to_speed(v: f32) -> f32 {
    let speed = 10f32.powf(v.clamp(0.0, 1.0) * 2.0 - 1.0);
    // Sticky at real time, as the Standard slider's marks are.
    if (speed - 1.0).abs() < 0.04 {
        1.0
    } else {
        (speed * 100.0).round() / 100.0
    }
}

/// The plot rectangle inside the editor's bounds.
fn plot(b: Bounds<Pixels>) -> (f32, f32, f32, f32) {
    (
        f32::from(b.origin.x) + INSET,
        f32::from(b.origin.y) + INSET,
        (f32::from(b.size.width) - 2.0 * INSET).max(1.0),
        (f32::from(b.size.height) - 2.0 * INSET).max(1.0),
    )
}

/// Paint `points` over `source` into `b`: the scale lines, the curve, the
/// points and the playhead.
fn paint_editor(
    window: &mut Window,
    b: Bounds<Pixels>,
    points: &[SpeedPoint],
    source: TimeRange,
    playhead: Option<f32>,
    selected: Option<usize>,
) {
    let (ox, oy, w, h) = plot(b);
    let at = |u: f32, v: f32| point(px(ox + u * w), px(oy + (1.0 - v) * h));
    let line = |window: &mut Window, a: (f32, f32), z: (f32, f32), colour: u32| {
        let mut path = PathBuilder::stroke(px(1.0));
        path.move_to(at(a.0, a.1));
        path.line_to(at(z.0, z.1));
        if let Ok(path) = path.build() {
            window.paint_path(path, rgb(colour));
        }
    };
    for speed in [0.25_f64, 0.5, 2.0, 4.0] {
        let v = speed_to_v(speed);
        line(window, (0.0, v), (1.0, v), HAIRLINE);
    }
    line(window, (0.0, 0.5), (1.0, 0.5), BORDER_STRONG);
    if let Some(u) = playhead {
        line(window, (u, 0.0), (u, 1.0), TEXT_MUTED);
    }

    let span = source.duration.max(1) as f64;
    let mut trace = PathBuilder::stroke(px(2.0));
    for i in 0..=160 {
        let u = i as f32 / 160.0;
        let x = source.start as f64 + u as f64 * span;
        let p = at(u, speed_to_v(curve_math::speed_at(points, x)));
        if i == 0 {
            trace.move_to(p);
        } else {
            trace.line_to(p);
        }
    }
    if let Ok(path) = trace.build() {
        window.paint_path(path, rgb(ACCENT));
    }
    for (i, p) in points.iter().enumerate() {
        let u = ((p.source - source.start) as f64 / span) as f32;
        if !(-0.001..=1.001).contains(&u) {
            continue;
        }
        let centre = at(u.clamp(0.0, 1.0), speed_to_v(p.speed as f64));
        let on = selected == Some(i);
        window.paint_quad(disc(centre, 5.0, rgb(if on { ACCENT } else { WELL })));
        window.paint_quad(ring(centre, 5.0, rgb(ACCENT), 1.5));
    }
}

/// A preset's thumbnail: its shape over a unit source.
fn paint_thumbnail(window: &mut Window, b: Bounds<Pixels>, shape: &[(f32, f32)], colour: u32) {
    let points = curve_math::points_from_shape(shape, TimeRange::new(0, 1_000_000));
    let (ox, oy) = (f32::from(b.origin.x) + 4.0, f32::from(b.origin.y) + 4.0);
    let (w, h) = (
        f32::from(b.size.width) - 8.0,
        f32::from(b.size.height) - 8.0,
    );
    let at = |u: f32, v: f32| point(px(ox + u * w), px(oy + (1.0 - v) * h));
    let mut mid = PathBuilder::stroke(px(1.0));
    mid.move_to(at(0.0, 0.5));
    mid.line_to(at(1.0, 0.5));
    if let Ok(path) = mid.build() {
        window.paint_path(path, rgb(BORDER));
    }
    if points.is_empty() {
        return;
    }
    let mut trace = PathBuilder::stroke(px(1.5));
    for i in 0..=48 {
        let u = i as f32 / 48.0;
        let p = at(
            u,
            speed_to_v(curve_math::speed_at(&points, u as f64 * 1_000_000.0)),
        );
        if i == 0 {
            trace.move_to(p);
        } else {
            trace.line_to(p);
        }
    }
    if let Ok(path) = trace.build() {
        window.paint_path(path, rgb(colour));
    }
}

/// Which tile is lit.
#[derive(Clone, Copy, PartialEq)]
enum Tile {
    None,
    Custom,
    Preset(SpeedPreset),
}

fn seconds(micros: Micros) -> String {
    format!("{:.1}s", micros as f64 / 1_000_000.0)
}

impl Editor {
    pub(super) fn speed_curve(&mut self, segment: &Segment, cx: &mut Context<Self>) -> AnyElement {
        let curve = self.project.materials.speed_curve_of(segment).cloned();
        let lit = match &curve {
            None => Tile::None,
            Some(c) => c.preset.map_or(Tile::Custom, Tile::Preset),
        };
        let mut tiles: Vec<(Tile, &'static str)> =
            vec![(Tile::None, "None"), (Tile::Custom, "Custom")];
        tiles.extend(
            SpeedPreset::ALL
                .iter()
                .map(|&p| (Tile::Preset(p), p.label())),
        );
        let custom_shape: Vec<(f32, f32)> = match &curve {
            Some(c) if c.preset.is_none() => {
                let span = segment.source_range.duration.max(1) as f32;
                c.points
                    .iter()
                    .map(|p| {
                        (
                            (p.source - segment.source_range.start) as f32 / span,
                            p.speed,
                        )
                    })
                    .collect()
            }
            _ => vec![(0.0, 1.0), (1.0, 1.0)],
        };
        let grid = div().flex().flex_row().flex_wrap().gap(px(8.0)).children(
            tiles.into_iter().enumerate().map(|(i, (tile, label))| {
                let on = tile == lit;
                let shape: Vec<(f32, f32)> = match tile {
                    Tile::None => Vec::new(),
                    Tile::Custom => custom_shape.clone(),
                    Tile::Preset(p) => p.shape().to_vec(),
                };
                let colour = if on { ACCENT } else { TEXT_DIM };
                let change = match tile {
                    Tile::None => CurveChange::Remove,
                    Tile::Custom => CurveChange::Custom,
                    Tile::Preset(preset) => CurveChange::Preset { preset },
                };
                div()
                    .id(("speed-tile", i))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.0))
                    .w(px(TILE_W + 4.0))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !on {
                            this.set_speed_curve(change.clone(), Phase::Commit, cx);
                        }
                    }))
                    .child(
                        div()
                            .w(px(TILE_W))
                            .h(px(TILE_H))
                            .rounded(px(R_SM))
                            .bg(rgb(WELL))
                            .border_1()
                            .border_color(rgb(if on { ACCENT } else { HAIRLINE }))
                            .when(on, |d| d.border_2())
                            .hover(|d| d.bg(rgb(PANEL_RAISED)))
                            .when(tile == Tile::None, |d| {
                                d.flex().items_center().justify_center().child(
                                    div()
                                        .text_size(px(TEXT_CAPTION))
                                        .text_color(rgb(colour))
                                        .child("Off"),
                                )
                            })
                            .when(tile != Tile::None, |d| {
                                d.child(
                                    canvas(
                                        |_, _, _| {},
                                        move |b, _, window, _| {
                                            paint_thumbnail(window, b, &shape, colour)
                                        },
                                    )
                                    .size_full(),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .flex()
                            .justify_center()
                            .text_size(px(TEXT_CAPTION))
                            .text_color(rgb(if on { TEXT } else { TEXT_MUTED }))
                            .child(label),
                    )
            }),
        );

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .px(px(PAD))
            .py(px(PAD))
            .child(grid);

        if let Some(curve) = curve {
            body = body.child(self.speed_curve_editor(segment, &curve.points, cx));
        } else {
            body = body.child(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Pick a ramp, or Custom to draw your own."),
            );
        }
        body.into_any_element()
    }

    fn speed_curve_editor(
        &mut self,
        segment: &Segment,
        stored: &[SpeedPoint],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let points: Vec<SpeedPoint> = match &self.inspector.speed.drag {
            Some(drag) => drag.points.clone(),
            None => stored.to_vec(),
        };
        let source = segment.source_range;
        let map = self.project.materials.time_map(segment);
        let playhead = map
            .source_time_at(self.clock.position())
            .map(|s| ((s - source.start) as f64 / source.duration.max(1) as f64) as f32);
        let selected = self.inspector.speed.selected.filter(|&i| i < points.len());
        let bounds = Rc::clone(&self.inspector.speed.bounds);
        let entity = cx.entity().downgrade();
        let paint_points = points.clone();
        let editor = div()
            .id("speed-curve-editor")
            .w_full()
            .h(px(EDITOR_H))
            .rounded(px(R_SM))
            .bg(rgb(WELL))
            .border_1()
            .border_color(rgb(BORDER))
            .cursor_crosshair()
            .relative()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.speed_press(event.position, false, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.speed_press(event.position, true, cx)
                }),
            )
            .child(
                canvas(
                    move |b, _, _| bounds.set(b),
                    move |b, _, window, _| {
                        paint_editor(window, b, &paint_points, source, playhead, selected);
                        // Registered on every paint, as the colour curves do:
                        // a press, its moves and its release can all land
                        // before the next frame.
                        let up = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = entity.update(cx, |this, cx| {
                                    if this.inspector.speed.drag.is_some() {
                                        this.speed_move(event.position, cx)
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = up.update(cx, |this, cx| this.speed_release(cx));
                            }
                        });
                    },
                )
                .size_full(),
            )
            .children(
                [("10x", 0.0_f32), ("1x", 0.5), ("0.1x", 1.0)].map(|(label, at)| {
                    div()
                        .absolute()
                        .left(px(INSET + 8.0))
                        .top(px(INSET + at * (EDITOR_H - 2.0 * INSET) - 14.0))
                        .text_size(px(TEXT_BADGE))
                        .font_family(FONT_MONO)
                        .text_color(rgb(TEXT_MUTED))
                        .child(label)
                }),
            );

        let readout = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(TEXT_DIM))
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(TEXT_LABEL))
                        .font_family(FONT_MONO)
                        .text_color(rgb(TEXT))
                        .child(value),
                )
        };
        let length = curve_math::curve_target_duration(&points, source);
        let mut readouts = div().flex().flex_row().justify_between().child(readout(
            "Duration",
            format!("{} → {}", seconds(source.duration), seconds(length)),
        ));
        if let Some(point) = selected.and_then(|i| points.get(i)) {
            readouts = readouts.child(readout("Point", format!("{:.2}x", point.speed)));
        }

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(editor)
            .child(readouts)
            .child(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Click to add a point, drag to move it, right-click to remove it."),
            )
            .child(self.pitch_switch(cx))
            .into_any_element()
    }

    /// The editor's normalised position of a window point: `u` across the
    /// clip's source, `v` up the speed scale.
    fn speed_local(&self, position: Point<Pixels>) -> (f32, f32) {
        let b = self.inspector.speed.bounds.get();
        let (ox, oy, w, h) = plot(b);
        (
            (f32::from(position.x) - ox) / w,
            1.0 - (f32::from(position.y) - oy) / h,
        )
    }

    fn speed_press(&mut self, position: Point<Pixels>, remove: bool, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let Some(curve) = self.project.materials.speed_curve_of(segment) else {
            return;
        };
        let source = segment.source_range;
        let mut points = curve.points.clone();
        let (u, v) = self.speed_local(position);
        let (_, _, w, h) = plot(self.inspector.speed.bounds.get());
        let span = source.duration.max(1) as f64;
        let near = points.iter().position(|p| {
            let pu = ((p.source - source.start) as f64 / span) as f32;
            let dx = (pu - u) * w;
            let dy = (speed_to_v(p.speed as f64) - v) * h;
            (dx * dx + dy * dy).sqrt() <= GRAB_PX
        });
        if remove {
            if let Some(i) = near {
                if points.len() > 2 {
                    points.remove(i);
                    self.inspector.speed.selected = None;
                    self.set_speed_curve(CurveChange::Points { points }, Phase::Commit, cx);
                }
            }
            return;
        }
        let index = match near {
            Some(i) => i,
            None => {
                let at = source.start + (u.clamp(0.0, 1.0) as f64 * span).round() as Micros;
                if points.iter().any(|p| (p.source - at).abs() < 1_000) {
                    return;
                }
                let i = points.partition_point(|p| p.source < at);
                points.insert(
                    i,
                    SpeedPoint {
                        source: at,
                        speed: v_to_speed(v),
                    },
                );
                i
            }
        };
        self.inspector.speed.selected = Some(index);
        self.inspector.speed.drag = Some(PointDrag { index, points });
        self.speed_move(position, cx);
    }

    fn speed_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mut drag) = self.inspector.speed.drag.take() else {
            return;
        };
        // A curve never changes the source range, so the one shown (the
        // preview's copy, mid-drag) is the clip's own.
        let Some(source) = self.selected_segment().map(|(_, s)| s.source_range) else {
            return;
        };
        let (u, v) = self.speed_local(position);
        let i = drag.index;
        let last = drag.points.len() - 1;
        // The outermost points keep their place in the footage and move only
        // up and down; an inner point stays between its neighbours, a
        // millisecond clear of each, so the points never reorder.
        if i != 0 && i != last {
            let lo = drag.points[i - 1].source + 1_000;
            let hi = drag.points[i + 1].source - 1_000;
            if lo <= hi {
                let at = source.start
                    + (u.clamp(0.0, 1.0) as f64 * source.duration as f64).round() as Micros;
                drag.points[i].source = at.clamp(lo, hi);
            }
        }
        drag.points[i].speed = v_to_speed(v);
        let points = drag.points.clone();
        self.inspector.speed.drag = Some(drag);
        self.set_speed_curve(CurveChange::Points { points }, Phase::Preview, cx);
    }

    fn speed_release(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.inspector.speed.drag.take() else {
            return;
        };
        self.set_speed_curve(
            CurveChange::Points {
                points: drag.points,
            },
            Phase::Commit,
            cx,
        );
    }

    /// Change the selected clip's speed curve: shown on a copy while
    /// dragging, written as one undo step on commit.
    pub(super) fn set_speed_curve(
        &mut self,
        change: CurveChange,
        phase: Phase,
        cx: &mut Context<Self>,
    ) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        if phase == Phase::Preview {
            let base = match &self.inspector.preview {
                Some(preview) if preview.prop == Prop::SpeedCurve => Arc::clone(&preview.base),
                _ => {
                    let base = Arc::clone(&self.project);
                    self.inspector.preview = Some(Preview {
                        prop: Prop::SpeedCurve,
                        base: Arc::clone(&base),
                    });
                    base
                }
            };
            let shown =
                speed_edit::set_curve_command(&base, &segment_id, change).and_then(|command| {
                    let mut copy = (*base).clone();
                    command.apply(&mut copy).map(|_| copy)
                });
            if let Ok(project) = shown {
                self.project = Arc::new(project);
                self.generation += 1;
                self.audio.set_project(Arc::clone(&self.project));
                cx.notify();
            }
            return;
        }
        if let Some(preview) = self.inspector.preview.take() {
            self.project = preview.base;
            self.generation += 1;
        }
        let result = speed_commands::speed_set_curve(&self.state, segment_id, change).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }
}
