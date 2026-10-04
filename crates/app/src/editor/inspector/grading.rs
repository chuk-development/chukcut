//! The Adjust tab's grading: Basic (sliders and the LUT), HSL, Curves and
//! Colour wheels.
//!
//! Every control ends in the engine's grade commands through
//! [`Change::Grade`], so it is one undo step and "Apply to all" carries it.
//! Sliders use the panel's ordinary row machinery; the curve editor and the
//! wheels are drawn here with GPUI's canvas and drive the same
//! preview-then-commit path a slider drag does ([`Editor::set_grade_with`]).

use std::cell::Cell;
use std::rc::Rc;

use chukcut_engine::modules::inspector::commands::LutEntry;
use chukcut_engine::modules::inspector::edit::GradeSection;
use chukcut_engine::modules::project::grade::{Wheel, HSL_BANDS};
use chukcut_engine::modules::render::grade::eval_curve;
use gpui::{
    canvas, hsla, point, quad, size, AnyElement, BorderStyle, DispatchPhase, Hsla, PathBuilder,
};

use super::controls::*;
use super::*;

/// Side of the curve editor's square, in pixels.
const CURVE_PX: f32 = 260.0;
/// Diameter of one colour wheel, in pixels.
const WHEEL_PX: f32 = 120.0;
/// How close a press must land to a curve point to grab it.
const GRAB_PX: f32 = 9.0;

/// The panel's grading state, kept between frames.
#[derive(Default)]
pub(crate) struct GradingState {
    /// The HSL band whose sliders are shown.
    hsl_band: u8,
    /// The curve shown in the editor; `None` is the master curve.
    curve: Option<CurveChannel>,
    curve_drag: Option<CurveDrag>,
    wheel_drag: Option<WheelKind>,
    /// Whether the LUT list is open, and what it lists.
    lut_menu: bool,
    lut_library: Vec<LutEntry>,
    curve_bounds: Rc<Cell<Bounds<Pixels>>>,
    wheel_bounds: [Rc<Cell<Bounds<Pixels>>>; 4],
}

/// One curve point being dragged. The points are the drag's own copy, so
/// an index stays valid while the document's copy is re-normalised under it.
#[derive(Clone)]
struct CurveDrag {
    channel: CurveChannel,
    index: usize,
    points: Vec<[f32; 2]>,
}

fn wheel_index(kind: WheelKind) -> usize {
    match kind {
        WheelKind::Lift => 0,
        WheelKind::Gamma => 1,
        WheelKind::Gain => 2,
        WheelKind::Offset => 3,
    }
}

fn wheel_title(kind: WheelKind) -> &'static str {
    match kind {
        WheelKind::Lift => "Shadows",
        WheelKind::Gamma => "Midtones",
        WheelKind::Gain => "Highlights",
        WheelKind::Offset => "Offset",
    }
}

fn channel_colour(channel: CurveChannel) -> u32 {
    match channel {
        CurveChannel::Master => CURVE_MASTER,
        CurveChannel::Red => CURVE_RED,
        CurveChannel::Green => CURVE_GREEN,
        CurveChannel::Blue => CURVE_BLUE,
    }
}

/// The swatch colour of an HSL band: its centre hue, fully saturated.
fn band_colour(band: usize) -> Hsla {
    hsla(HSL_BANDS[band].1 / 360.0, 0.85, 0.55, 1.0)
}

/// A curve as the editor draws and edits it: the identity is the two end
/// points, never an empty list.
fn editable(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if points.is_empty() {
        vec![[0.0, 0.0], [1.0, 1.0]]
    } else {
        points.to_vec()
    }
}

pub(super) fn local(bounds: Bounds<Pixels>, position: Point<Pixels>) -> (f32, f32) {
    let w = f32::from(bounds.size.width).max(1.0);
    let h = f32::from(bounds.size.height).max(1.0);
    (
        (f32::from(position.x) - f32::from(bounds.origin.x)) / w,
        (f32::from(position.y) - f32::from(bounds.origin.y)) / h,
    )
}

/// A filled circle as a fully rounded quad.
pub(super) fn disc(
    centre: Point<Pixels>,
    radius: f32,
    colour: impl Into<gpui::Background>,
) -> gpui::PaintQuad {
    quad(
        Bounds::new(
            point(centre.x - px(radius), centre.y - px(radius)),
            size(px(radius * 2.0), px(radius * 2.0)),
        ),
        px(radius),
        colour,
        px(0.0),
        gpui::transparent_black(),
        BorderStyle::Solid,
    )
}

pub(super) fn ring(
    centre: Point<Pixels>,
    radius: f32,
    colour: impl Into<Hsla>,
    width: f32,
) -> gpui::PaintQuad {
    quad(
        Bounds::new(
            point(centre.x - px(radius), centre.y - px(radius)),
            size(px(radius * 2.0), px(radius * 2.0)),
        ),
        px(radius),
        gpui::transparent_black(),
        px(width),
        colour,
        BorderStyle::Solid,
    )
}

impl Editor {
    // --- writing ---------------------------------------------------------------------------

    /// Change the selected clip's grade with `f`, as a preview (drawn, not
    /// written) or a commit (one undo step). `prop` names the gesture, so a
    /// preview is always built against the document from before the drag.
    pub(super) fn set_grade_with(
        &mut self,
        prop: Prop,
        phase: Phase,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut GradeEdit),
    ) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        if phase == Phase::Preview {
            let base = match &self.inspector.preview {
                Some(preview) if preview.prop == prop => Arc::clone(&preview.base),
                _ => {
                    let base = Arc::clone(&self.project);
                    self.inspector.preview = Some(Preview {
                        prop,
                        base: Arc::clone(&base),
                    });
                    base
                }
            };
            let Ok(mut edit) = GradeEdit::of_segment(&base, &segment_id) else {
                return;
            };
            f(&mut edit);
            if let Ok(project) = Self::previewed(&base, &segment_id, &Change::Grade(Some(edit))) {
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
        let Ok(mut edit) = GradeEdit::of_segment(&self.project, &segment_id) else {
            return;
        };
        let before = edit.clone();
        f(&mut edit);
        if edit == before {
            self.refresh(cx);
            return;
        }
        self.commit_change(segment_id, Change::Grade(Some(edit)), cx);
    }

    /// Put one section of the selected clip's grade back at rest.
    fn reset_grade_section(&mut self, section: GradeSection, cx: &mut Context<Self>) {
        self.set_grade_with(Prop::Brightness, Phase::Commit, cx, |edit| {
            *edit = edit.clone().reset(section);
        });
    }

    fn current_grade(&self, segment: &Segment) -> GradeEdit {
        GradeEdit::of(self.project.materials.color_adjust_of(segment))
    }

    // --- Basic ------------------------------------------------------------------------------

    pub(super) fn adjust_basic(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let grade = self.current_grade(segment);
        let mut sections = vec![
            self.colour_tools(segment, window, cx),
            self.lut_section(segment, &grade, window, cx),
        ];

        let mut rest = grade.clone().reset(GradeSection::Basic);
        rest.lut = grade.lut.clone();
        let graded = rest != grade;
        let mut rows = vec![group_label("Colour")];
        for prop in [
            Prop::Temperature,
            Prop::Tint,
            Prop::Saturation,
            Prop::Vibrance,
        ] {
            rows.push(self.slider_row(prop, segment, window, cx));
        }
        rows.push(group_label("Light"));
        for prop in [
            Prop::Exposure,
            Prop::Brightness,
            Prop::Contrast,
            Prop::Highlights,
            Prop::Shadows,
            Prop::Whites,
            Prop::Blacks,
        ] {
            rows.push(self.slider_row(prop, segment, window, cx));
        }
        rows.push(group_label("Effects"));
        for prop in [
            Prop::Sharpen,
            Prop::Clarity,
            Prop::Grain,
            Prop::Fade,
            Prop::Vignette,
            Prop::VignetteMidpoint,
            Prop::VignetteFeather,
        ] {
            rows.push(self.slider_row(prop, segment, window, cx));
        }
        sections.push(
            Section {
                checkbox: Some(graded),
                on_check: graded.then(|| {
                    Box::new(|this: &mut Editor, _: bool, cx: &mut Context<Editor>| {
                        this.reset_grade_section(GradeSection::Basic, cx)
                    }) as Box<dyn Fn(&mut Editor, bool, &mut Context<Editor>)>
                }),
                on_reset: Some(Box::new(|this: &mut Editor, cx| {
                    this.reset_grade_section(GradeSection::Basic, cx)
                })),
                ..Section::new("Adjust")
            }
            .render(self.collapsed("Adjust"), rows, cx),
        );
        div()
            .flex()
            .flex_col()
            .children(sections)
            .into_any_element()
    }

    fn lut_section(
        &mut self,
        segment: &Segment,
        grade: &GradeEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = grade.lut.as_ref().map(|lut| file_name(&lut.path));
        let open = self.inspector.grading.lut_menu;
        let picker = div()
            .id("lut-picker")
            .w(px(220.0))
            .h(px(CONTROL_H))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .rounded(px(R_SM))
            .bg(rgb(WELL))
            .border_1()
            .border_color(rgb(if open { ACCENT } else { BORDER }))
            .when(!open, |this| {
                this.hover(|style| style.border_color(rgb(BORDER_STRONG)))
            })
            .cursor_pointer()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(if current.is_some() { TEXT } else { TEXT_DIM }))
            .on_click(cx.listener(|this, _, _, cx| {
                let grading = &mut this.inspector.grading;
                grading.lut_menu = !grading.lut_menu;
                if grading.lut_menu {
                    grading.lut_library = inspector_commands::inspector_lut_library();
                }
                cx.notify();
            }))
            .child(current.clone().unwrap_or_else(|| "None".into()))
            .child(icon(
                if open { icons::UP } else { icons::DOWN },
                10.0,
                TEXT_DIM,
            ));

        let mut rows = vec![label_row("Name", picker)];
        if open {
            rows.push(self.lut_menu(grade, cx));
        }
        if grade.lut.is_some() {
            rows.push(self.slider_row(Prop::LutIntensity, segment, window, cx));
        }
        Section {
            checkbox: Some(grade.lut.is_some()),
            on_check: grade.lut.is_some().then(|| {
                Box::new(|this: &mut Editor, _: bool, cx: &mut Context<Editor>| {
                    this.reset_grade_section(GradeSection::Lut, cx)
                }) as Box<dyn Fn(&mut Editor, bool, &mut Context<Editor>)>
            }),
            on_reset: Some(Box::new(|this: &mut Editor, cx| {
                this.reset_grade_section(GradeSection::Lut, cx)
            })),
            ..Section::new("LUT")
        }
        .render(self.collapsed("LUT"), rows, cx)
    }

    /// The open LUT list: none, every library file, and the import button.
    fn lut_menu(&self, grade: &GradeEdit, cx: &mut Context<Self>) -> AnyElement {
        let selected = grade.lut.as_ref().map(|l| l.path.clone());
        let item = |id: SharedString, label: String, active: bool| {
            div()
                .id(id)
                .h(px(24.0))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(R_XS))
                .cursor_pointer()
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(if active { ACCENT } else { TEXT }))
                .hover(|style| style.bg(rgb(PANEL_RAISED)))
                .child(label)
        };
        let mut list = div()
            .flex()
            .flex_col()
            .p_1()
            .rounded(px(R_SM))
            .bg(rgb(OVERLAY))
            .border_1()
            .border_color(rgb(BORDER))
            .child(
                item("lut-none".into(), "None".into(), selected.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.choose_lut(None, cx))),
            );
        for (i, entry) in self.inspector.grading.lut_library.iter().enumerate() {
            let path = entry.path.clone();
            let active = selected.as_deref() == Some(entry.path.as_str());
            list = list.child(
                item(
                    SharedString::from(format!("lut-{i}")),
                    entry.name.clone(),
                    active,
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.choose_lut(Some(path.clone()), cx)),
                ),
            );
        }
        if self.inspector.grading.lut_library.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("The LUT library is empty."),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(list)
            .child(div().flex().flex_row().child(panel_button(
                "lut-import",
                "Import LUT…",
                false,
                true,
                cx.listener(|this, _, _, cx| this.import_lut(cx)),
            )))
            .into_any_element()
    }

    /// Attach a library LUT (validated first, so a broken file is refused
    /// with the parser's message before the document is touched), or none.
    fn choose_lut(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        self.inspector.grading.lut_menu = false;
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        if let Some(path) = &path {
            if let Err(error) = inspector_commands::inspector_lut_probe(path.clone()) {
                self.report(Err(error), cx);
                return;
            }
        }
        let had = self
            .selected_segment()
            .and_then(|(_, s)| self.project.materials.color_adjust_of(s))
            .and_then(|m| m.lut.clone());
        if had.as_ref().map(|l| &l.path) == path.as_ref() {
            cx.notify();
            return;
        }
        let lut = path.map(|path| chukcut_engine::modules::project::LutRef {
            path,
            intensity: had.map_or(1.0, |l| l.intensity),
        });
        let result =
            inspector_commands::inspector_set_lut(&self.state, segment_id, lut).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    /// Pick a `.cube` file, copy it into the library, and attach it.
    fn import_lut(&mut self, cx: &mut Context<Self>) {
        let picked = files::choose_one(FileRequest::open("Import LUT", Filter::Luts), cx);
        cx.spawn(async move |this, cx| {
            let Some(path) = picked.await else {
                return;
            };
            let _ = this.update(
                cx,
                |editor, cx| match inspector_commands::inspector_lut_import(
                    path.to_string_lossy().into_owned(),
                ) {
                    Ok(entry) => {
                        editor.inspector.grading.lut_library =
                            inspector_commands::inspector_lut_library();
                        editor.choose_lut(Some(entry.path), cx);
                    }
                    Err(error) => editor.report(Err(error), cx),
                },
            );
        })
        .detach();
    }

    // --- HSL --------------------------------------------------------------------------------

    pub(super) fn adjust_hsl(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let grade = self.current_grade(segment);
        let band = self.inspector.grading.hsl_band.min(7);
        let swatches = div()
            .flex()
            .flex_row()
            .gap_2()
            .py_1()
            .children((0..8).map(|i| {
                let selected = i == band as usize;
                let touched = grade.grade.hsl.bands[i] != Default::default();
                div()
                    .id(("hsl-band", i))
                    .size(px(22.0))
                    .rounded_full()
                    .border_2()
                    .border_color(if selected {
                        rgb(TEXT).into()
                    } else {
                        gpui::transparent_black()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.inspector.grading.hsl_band = i as u8;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(16.0))
                            .rounded_full()
                            .bg(band_colour(i))
                            .when(touched, |this| this.border_1().border_color(rgb(TEXT))),
                    )
            }));
        let rows = vec![
            label_row(
                HSL_BANDS[band as usize].0,
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Hue · Saturation · Luminance"),
            ),
            swatches.into_any_element(),
            self.slider_row(Prop::HslHue(band), segment, window, cx),
            self.slider_row(Prop::HslSaturation(band), segment, window, cx),
            self.slider_row(Prop::HslLuminance(band), segment, window, cx),
        ];
        let touched = grade.grade.hsl != Default::default();
        Section {
            checkbox: Some(touched),
            on_check: touched.then(|| {
                Box::new(|this: &mut Editor, _: bool, cx: &mut Context<Editor>| {
                    this.reset_grade_section(GradeSection::Hsl, cx)
                }) as Box<dyn Fn(&mut Editor, bool, &mut Context<Editor>)>
            }),
            on_reset: Some(Box::new(|this: &mut Editor, cx| {
                this.reset_grade_section(GradeSection::Hsl, cx)
            })),
            ..Section::new("HSL")
        }
        .render(self.collapsed("HSL"), rows, cx)
    }

    // --- Curves -----------------------------------------------------------------------------

    pub(super) fn adjust_curves(
        &mut self,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let grade = self.current_grade(segment);
        let channel = self.inspector.grading.curve.unwrap_or(CurveChannel::Master);
        let points = match &self.inspector.grading.curve_drag {
            Some(drag) if drag.channel == channel => drag.points.clone(),
            _ => editable(grade.grade.curves.get(channel)),
        };
        let others: Vec<(CurveChannel, Vec<[f32; 2]>)> = CurveChannel::ALL
            .into_iter()
            .filter(|&c| c != channel && !grade.grade.curves.get(c).is_empty())
            .map(|c| (c, grade.grade.curves.get(c).clone()))
            .collect();

        let tabs = div()
            .flex()
            .flex_row()
            .gap_2()
            .children(CurveChannel::ALL.into_iter().map(|c| {
                let active = c == channel;
                let edited = !grade.grade.curves.get(c).is_empty();
                div()
                    .id(SharedString::from(format!("curve-{c:?}")))
                    .size(px(24.0))
                    .rounded_full()
                    .border_2()
                    .border_color(if active {
                        rgb(ACCENT).into()
                    } else {
                        gpui::transparent_black()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.inspector.grading.curve = Some(c);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(16.0))
                            .rounded_full()
                            .bg(rgb(channel_colour(c)))
                            .when(edited, |this| this.border_2().border_color(rgb(WELL))),
                    )
            }));

        let bounds = Rc::clone(&self.inspector.grading.curve_bounds);
        let entity = cx.entity().downgrade();
        let colour = channel_colour(channel);
        let paint_points = points.clone();
        let editor_box = div()
            .id("curve-editor")
            .w(px(CURVE_PX))
            .h(px(CURVE_PX))
            .flex_none()
            .rounded(px(R_SM))
            .bg(rgb(WELL))
            .border_1()
            .border_color(rgb(BORDER))
            .cursor_crosshair()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.curve_press(channel, event.position, false, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.curve_press(channel, event.position, true, cx)
                }),
            )
            .child(
                canvas(
                    move |b, _, _| bounds.set(b),
                    move |b, _, window, _| {
                        paint_curve(window, b, &paint_points, colour, &others);
                        // Registered on every paint, not only mid-drag: a
                        // press can be followed by its moves and release
                        // before the next frame, and a release the editor
                        // never sees would leave the drag stuck. The handlers
                        // do nothing without a drag in progress.
                        let up = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = entity.update(cx, |this, cx| {
                                    if this.inspector.grading.curve_drag.is_some() {
                                        this.curve_move(event.position, cx)
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = up.update(cx, |this, cx| this.curve_release(cx));
                            }
                        });
                    },
                )
                .size_full(),
            );

        let rows = vec![
            tabs.into_any_element(),
            editor_box.into_any_element(),
            div()
                .text_size(px(TEXT_CAPTION))
                .text_color(rgb(TEXT_MUTED))
                .child("Click to add a point, drag to move it, right-click to remove it.")
                .into_any_element(),
        ];
        let touched = !grade.grade.curves.is_identity();
        Section {
            checkbox: Some(touched),
            on_check: touched.then(|| {
                Box::new(|this: &mut Editor, _: bool, cx: &mut Context<Editor>| {
                    this.reset_grade_section(GradeSection::Curves, cx)
                }) as Box<dyn Fn(&mut Editor, bool, &mut Context<Editor>)>
            }),
            on_reset: Some(Box::new(|this: &mut Editor, cx| {
                this.reset_grade_section(GradeSection::Curves, cx)
            })),
            ..Section::new("Curves")
        }
        .render(self.collapsed("Curves"), rows, cx)
    }

    /// A press in the curve editor: grab the nearest point, add one, or —
    /// with `remove` — take the point under the pointer away.
    fn curve_press(
        &mut self,
        channel: CurveChannel,
        position: Point<Pixels>,
        remove: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let mut points = editable(self.current_grade(segment).grade.curves.get(channel));
        let bounds = self.inspector.grading.curve_bounds.get();
        let (x, y) = local(bounds, position);
        let (x, y) = (x.clamp(0.0, 1.0), (1.0 - y).clamp(0.0, 1.0));
        let side = f32::from(bounds.size.width).max(1.0);
        let near = points.iter().position(|p| {
            let dx = (p[0] - x) * side;
            let dy = (p[1] - y) * side;
            (dx * dx + dy * dy).sqrt() <= GRAB_PX
        });
        if remove {
            if let Some(i) = near {
                // The end points stay: a curve needs a value at 0 and at 1.
                if i != 0 && i != points.len() - 1 {
                    points.remove(i);
                    self.set_grade_with(Prop::Curve(channel), Phase::Commit, cx, |edit| {
                        *edit.grade.curves.get_mut(channel) = points;
                    });
                }
            }
            return;
        }
        let index = match near {
            Some(i) => i,
            None => {
                let at = points.iter().position(|p| p[0] > x).unwrap_or(points.len());
                points.insert(at, [x, y]);
                at
            }
        };
        self.inspector.grading.curve_drag = Some(CurveDrag {
            channel,
            index,
            points,
        });
        self.curve_move(position, cx);
    }

    fn curve_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mut drag) = self.inspector.grading.curve_drag.take() else {
            return;
        };
        let (x, y) = local(self.inspector.grading.curve_bounds.get(), position);
        let last = drag.points.len() - 1;
        let i = drag.index;
        // The ends move only up and down; an inner point stays between its
        // neighbours, so the points never reorder under the drag.
        let x = if i == 0 {
            0.0
        } else if i == last {
            1.0
        } else {
            x.clamp(drag.points[i - 1][0] + 0.01, drag.points[i + 1][0] - 0.01)
        };
        drag.points[i] = [x, (1.0 - y).clamp(0.0, 1.0)];
        let (channel, points) = (drag.channel, drag.points.clone());
        self.inspector.grading.curve_drag = Some(drag);
        self.set_grade_with(Prop::Curve(channel), Phase::Preview, cx, |edit| {
            *edit.grade.curves.get_mut(channel) = points;
        });
    }

    fn curve_release(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.inspector.grading.curve_drag.take() else {
            return;
        };
        let (channel, points) = (drag.channel, drag.points);
        self.set_grade_with(Prop::Curve(channel), Phase::Commit, cx, |edit| {
            *edit.grade.curves.get_mut(channel) = points;
        });
    }

    // --- Colour wheels ----------------------------------------------------------------------

    pub(super) fn adjust_wheels(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let grade = self.current_grade(segment);
        let mut cells = Vec::new();
        for kind in WheelKind::ALL {
            let wheel = *grade.grade.wheels.get(kind);
            cells.push(self.wheel_cell(kind, wheel, segment, window, cx));
        }
        let mut cells = cells.into_iter();
        let mut grid = Vec::new();
        while let (Some(a), b) = (cells.next(), cells.next()) {
            grid.push(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(a)
                    .children(b)
                    .into_any_element(),
            );
        }
        let touched = WheelKind::ALL
            .iter()
            .any(|&k| !grade.grade.wheels.get(k).is_identity());
        Section {
            checkbox: Some(touched),
            on_check: touched.then(|| {
                Box::new(|this: &mut Editor, _: bool, cx: &mut Context<Editor>| {
                    this.reset_grade_section(GradeSection::Wheels, cx)
                }) as Box<dyn Fn(&mut Editor, bool, &mut Context<Editor>)>
            }),
            on_reset: Some(Box::new(|this: &mut Editor, cx| {
                this.reset_grade_section(GradeSection::Wheels, cx)
            })),
            ..Section::new("Colour wheels")
        }
        .render(self.collapsed("Colour wheels"), grid, cx)
    }

    fn wheel_cell(
        &mut self,
        kind: WheelKind,
        wheel: Wheel,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bounds = Rc::clone(&self.inspector.grading.wheel_bounds[wheel_index(kind)]);
        let entity = cx.entity().downgrade();
        let value = self.prop_value(Prop::WheelLuma(kind), segment);
        let number = self.number_box(Prop::WheelLuma(kind), value, 56.0, None, window, cx);
        let (_, slider) = self.field(Prop::WheelLuma(kind), window, cx);
        let disc_el = div()
            .id(SharedString::from(format!("wheel-{kind:?}")))
            .size(px(WHEEL_PX))
            .flex_none()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.set_grade_with(Prop::WheelPuck(kind), Phase::Commit, cx, |edit| {
                            let w = edit.grade.wheels.get_mut(kind);
                            (w.x, w.y) = (0.0, 0.0);
                        });
                        return;
                    }
                    this.inspector.grading.wheel_drag = Some(kind);
                    this.wheel_move(event.position, cx);
                }),
            )
            .child(
                canvas(
                    move |b, _, _| bounds.set(b),
                    move |b, _, window, _| {
                        paint_wheel(window, b, wheel);
                        // Always registered, for the reason the curve
                        // editor gives; each wheel answers only its own drag.
                        let up = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = entity.update(cx, |this, cx| {
                                    if this.inspector.grading.wheel_drag == Some(kind) {
                                        this.wheel_move(event.position, cx)
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = up.update(cx, |this, cx| {
                                    if this.inspector.grading.wheel_drag == Some(kind) {
                                        this.wheel_release(cx)
                                    }
                                });
                            }
                        });
                    },
                )
                .size_full(),
            );
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_2()
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(TEXT_LABEL))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .child(wheel_title(kind)),
                    )
                    .child(icon_button(
                        SharedString::from(format!("wheel-reset-{kind:?}")),
                        icons::RESET,
                        TEXT_DIM,
                        !wheel.is_identity(),
                        cx.listener(move |this, _, _, cx| {
                            this.set_grade_with(Prop::WheelPuck(kind), Phase::Commit, cx, |edit| {
                                *edit.grade.wheels.get_mut(kind) = Wheel::default();
                            })
                        }),
                    )),
            )
            .child(disc_el)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().px_1().when_some(slider, |this, slider| {
                        this.child(slider_with_track(Prop::WheelLuma(kind), &slider))
                    }))
                    .child(number),
            )
            .into_any_element()
    }

    fn wheel_puck_at(&self, kind: WheelKind, position: Point<Pixels>) -> (f32, f32) {
        let (x, y) = local(
            self.inspector.grading.wheel_bounds[wheel_index(kind)].get(),
            position,
        );
        // Screen y grows down; the wheel's y points up (towards green).
        let (mut x, mut y) = (x * 2.0 - 1.0, 1.0 - y * 2.0);
        let r = (x * x + y * y).sqrt();
        if r > 1.0 {
            x /= r;
            y /= r;
        }
        (x, y)
    }

    fn wheel_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(kind) = self.inspector.grading.wheel_drag else {
            return;
        };
        let (x, y) = self.wheel_puck_at(kind, position);
        self.set_grade_with(Prop::WheelPuck(kind), Phase::Preview, cx, |edit| {
            let w = edit.grade.wheels.get_mut(kind);
            (w.x, w.y) = (x, y);
        });
    }

    fn wheel_release(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.inspector.grading.wheel_drag.take() else {
            return;
        };
        // The puck is where the last preview put it; commit that.
        let puck = self
            .selected_segment()
            .map(|(_, s)| *self.current_grade(s).grade.wheels.get(kind));
        let Some(puck) = puck else {
            return;
        };
        self.set_grade_with(Prop::WheelPuck(kind), Phase::Commit, cx, |edit| {
            let w = edit.grade.wheels.get_mut(kind);
            (w.x, w.y) = (puck.x, puck.y);
        });
    }
}

// --- painting --------------------------------------------------------------------------------

fn paint_curve(
    window: &mut Window,
    b: Bounds<Pixels>,
    points: &[[f32; 2]],
    colour: u32,
    others: &[(CurveChannel, Vec<[f32; 2]>)],
) {
    let (ox, oy) = (f32::from(b.origin.x), f32::from(b.origin.y));
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let at = |x: f32, y: f32| point(px(ox + x * w), px(oy + (1.0 - y) * h));

    // Quarter grid and the identity diagonal.
    for i in 1..4 {
        let t = i as f32 / 4.0;
        for (a, z) in [((t, 0.0), (t, 1.0)), ((0.0, t), (1.0, t))] {
            let mut line = PathBuilder::stroke(px(1.0));
            line.move_to(at(a.0, a.1));
            line.line_to(at(z.0, z.1));
            if let Ok(path) = line.build() {
                window.paint_path(path, rgb(HAIRLINE));
            }
        }
    }
    let mut diagonal = PathBuilder::stroke(px(1.0));
    diagonal.move_to(at(0.0, 0.0));
    diagonal.line_to(at(1.0, 1.0));
    if let Ok(path) = diagonal.build() {
        window.paint_path(path, rgb(BORDER));
    }

    let trace = |window: &mut Window, points: &[[f32; 2]], colour: Hsla, width: f32| {
        let mut line = PathBuilder::stroke(px(width));
        for i in 0..=96 {
            let x = i as f32 / 96.0;
            let p = at(x, eval_curve(points, x));
            if i == 0 {
                line.move_to(p);
            } else {
                line.line_to(p);
            }
        }
        if let Ok(path) = line.build() {
            window.paint_path(path, colour);
        }
    };
    // The other edited curves, faint, so the whole grade is visible at once.
    for (channel, other) in others {
        trace(
            window,
            other,
            rgb(channel_colour(*channel)).opacity(0.3).into(),
            1.0,
        );
    }
    trace(window, points, rgb(colour).into(), 2.0);
    for p in points {
        let centre = at(p[0], p[1]);
        window.paint_quad(disc(centre, 4.5, rgb(WELL)));
        window.paint_quad(ring(centre, 4.5, rgb(colour), 1.5));
    }
}

fn paint_wheel(window: &mut Window, b: Bounds<Pixels>, wheel: Wheel) {
    let r = f32::from(b.size.width).min(f32::from(b.size.height)) * 0.5 - 2.0;
    let centre = point(
        b.origin.x + b.size.width * 0.5,
        b.origin.y + b.size.height * 0.5,
    );
    let (cx0, cy0) = (f32::from(centre.x), f32::from(centre.y));

    // The hue ring: wedges at the hue the puck pushes towards in that
    // direction, red to the right and turning counter-clockwise, the
    // layout `project::grade::Wheel` documents.
    const WEDGES: usize = 72;
    for i in 0..WEDGES {
        let a0 = i as f32 / WEDGES as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / WEDGES as f32 * std::f32::consts::TAU + 0.01;
        let mut wedge = PathBuilder::fill();
        wedge.move_to(centre);
        wedge.line_to(point(px(cx0 + r * a0.cos()), px(cy0 - r * a0.sin())));
        wedge.line_to(point(px(cx0 + r * a1.cos()), px(cy0 - r * a1.sin())));
        wedge.close();
        if let Ok(path) = wedge.build() {
            window.paint_path(path, hsla(i as f32 / WEDGES as f32, 0.7, 0.5, 1.0));
        }
    }
    // Fade the colour towards a neutral centre with stacked translucent
    // discs — GPUI has no radial gradient — so the wheel reads as "more
    // colour further out".
    for k in 0..12 {
        let radius = r * (1.0 - k as f32 / 12.0);
        window.paint_quad(disc(centre, radius, rgb(OVERLAY).opacity(0.14)));
    }
    window.paint_quad(ring(centre, r, rgb(PANEL), 1.0));
    // Cross-hair at the neutral centre.
    for (dx, dy) in [(1.0, 0.0), (0.0, 1.0)] {
        let mut line = PathBuilder::stroke(px(1.0));
        line.move_to(point(px(cx0 - 6.0 * dx), px(cy0 - 6.0 * dy)));
        line.line_to(point(px(cx0 + 6.0 * dx), px(cy0 + 6.0 * dy)));
        if let Ok(path) = line.build() {
            window.paint_path(path, rgb(TEXT_DIM));
        }
    }
    let puck = point(px(cx0 + wheel.x * r), px(cy0 - wheel.y * r));
    window.paint_quad(disc(puck, 6.0, rgb(PANEL).opacity(0.6)));
    window.paint_quad(ring(puck, 6.0, rgb(TEXT), 2.0));
}
