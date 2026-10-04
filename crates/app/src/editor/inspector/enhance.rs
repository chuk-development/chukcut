//! The Video tab's "Enhance" sub-tab: "Remove object" (click the object on
//! the player, or paint over it) and "Enhance quality" (2x or 4x), and the
//! bakes' progress (`modules/enhance` in the engine, decision 0029).
//!
//! A bake runs on an engine thread. The panel follows the one it started
//! for the selected clip (progress, the CPU time warning, Stop); bakes the
//! engine starts on its own after an edit (`enhance_queue_missing`) are only
//! followed so the preview redraws as their frames land.

use chukcut_engine::modules::enhance::commands as enhance;
use chukcut_engine::modules::enhance::{self as enh, Chain};
use chukcut_engine::modules::matting::commands::CanvasPoint;
use gpui::component::button::Button;
use gpui::component::Sizable as _;
use gpui::{canvas, point, rgba, AnyElement, DispatchPhase};

use crate::ui::SegmentedTabs;

use super::controls::*;
use super::grading::disc;
use super::*;

/// The sub-tab's name under Video.
pub(super) const ENHANCE: &str = "Enhance";

/// Brush radii, as fractions of the canvas's shorter side: S, M, L.
const BRUSHES: [f32; 3] = [0.015, 0.03, 0.06];

/// What a press on the player does while the sub-tab is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    /// Presses do nothing here (the player's own handles work).
    #[default]
    Off,
    /// A click selects the object to remove (Alt-click or right-click: a
    /// part to keep).
    Select,
    /// A drag paints a stroke to remove.
    Paint,
}

/// The sub-tab's own state, one field on the inspector.
#[derive(Default)]
pub(crate) struct EnhancePanel {
    mode: Mode,
    brush: usize,
    /// The clicks so far, with the clip and the timeline time they are on.
    clicks: Option<(String, Micros, Vec<CanvasPoint>)>,
    /// The stroke being painted, in canvas fractions.
    stroke: Option<Vec<[f32; 2]>>,
    /// Where the overlay was drawn, for turning presses into its pixels.
    overlay: Rc<Cell<Bounds<Pixels>>>,
    /// The bake the panel shows progress for.
    bake: Option<Bake>,
    /// Bakes started after an edit, with the frames each had done.
    background: Vec<(u64, u32)>,
    redrawn: Option<std::time::Instant>,
}

struct Bake {
    job: u64,
    segment_id: String,
    stage: String,
    done: u32,
    total: u32,
    warning: Option<String>,
}

fn caption(text: impl Into<SharedString>, colour: u32) -> AnyElement {
    div()
        .text_size(px(TEXT_CAPTION))
        .text_color(rgb(colour))
        .child(text.into())
        .into_any_element()
}

fn right(child: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .justify_end()
        .gap(px(8.0))
        .child(child)
        .into_any_element()
}

impl Editor {
    fn enhance_open(&self) -> bool {
        self.inspector.tab.unwrap_or("Video") == "Video"
            && self.inspector.sub_tab.get("Video").copied() == Some(ENHANCE)
    }

    /// The sub-tab for a video clip.
    pub(super) fn enhance_tab(&mut self, segment: &Segment, cx: &mut Context<Self>) -> AnyElement {
        let materials = &self.project.materials;
        let removal = enh::removal_of(materials, segment);
        let upscale = enh::upscale_of(materials, segment);
        let video = materials.video(&segment.material_id).cloned();
        if video.is_none() {
            return div()
                .px(px(PAD))
                .py(px(PAD))
                .child(caption(
                    "Remove object and Enhance quality work on video clips.",
                    TEXT_MUTED,
                ))
                .into_any_element();
        }
        let frames = video
            .as_ref()
            .map(|v| (segment.source_range.duration as f64 * v.fps / 1e6).round() as u32)
            .unwrap_or(0);
        let estimate = |chain: Chain| -> String {
            let Some(v) = &video else {
                return String::new();
            };
            let (gpu, cpu) = enh::seconds_per_frame(&chain, (v.width, v.height));
            format!(
                "{frames} frames: about {} on a GPU, {} on the CPU.",
                enh::duration_words(gpu * frames as f64),
                enh::duration_words(cpu * frames as f64)
            )
        };

        // --- Remove object --------------------------------------------------------------
        let mut rows: Vec<AnyElement> = vec![caption(
            "Paints over an object in every frame with LaMa (Apache-2.0, 208 MB download), \
             on this machine. Click the object, or paint over a logo or a sign.",
            TEXT_MUTED,
        )];
        let mode = self.inspector.enhance.mode;
        let editor = cx.entity().downgrade();
        let modes = [Mode::Select, Mode::Paint];
        rows.push(
            SegmentedTabs::new(
                "enhance-remove-mode",
                ["Select on player", "Paint on player"],
                modes.iter().position(|m| *m == mode).unwrap_or(usize::MAX),
            )
            .on_select(move |index, _, cx| {
                let _ = editor.update(cx, |this, cx| {
                    let next = modes[index];
                    let panel = &mut this.inspector.enhance;
                    panel.mode = if panel.mode == next { Mode::Off } else { next };
                    panel.clicks = None;
                    panel.stroke = None;
                    cx.notify();
                });
            })
            .into_any_element(),
        );
        match mode {
            Mode::Select => rows.push(caption(
                "Click the object on the player; Alt-click or right-click a part to keep. \
                 It is followed over the whole clip.",
                ACCENT,
            )),
            Mode::Paint => {
                let editor = cx.entity().downgrade();
                rows.push(label_row(
                    "Brush",
                    SegmentedTabs::new(
                        "enhance-brush",
                        ["S", "M", "L"],
                        self.inspector.enhance.brush,
                    )
                    .on_select(move |index, _, cx| {
                        let _ = editor.update(cx, |this, cx| {
                            this.inspector.enhance.brush = index;
                            cx.notify();
                        });
                    }),
                ));
                rows.push(caption(
                    "Paint over what to remove on the player. A stroke covers the same place \
                     in every frame.",
                    ACCENT,
                ));
            }
            Mode::Off => {}
        }
        if let Some(r) = &removal {
            let mut parts = Vec::new();
            if r.prompt.is_some() {
                parts.push("the selected object".to_string());
            }
            if !r.strokes.is_empty() {
                parts.push(format!(
                    "{} painted stroke{}",
                    r.strokes.len(),
                    if r.strokes.len() == 1 { "" } else { "s" }
                ));
            }
            if !r.boxes.is_empty() {
                parts.push(format!(
                    "{} box{}",
                    r.boxes.len(),
                    if r.boxes.len() == 1 { "" } else { "es" }
                ));
            }
            rows.push(caption(format!("Removing {}.", parts.join(", ")), TEXT_DIM));
            rows.push(caption(
                estimate(Chain {
                    removal: Some(r.clone()),
                    upscale: None,
                }),
                TEXT_MUTED,
            ));
        }
        let id = segment.id.clone();
        let remove_section = Section {
            checkbox: Some(removal.is_some()),
            on_check: Some(Box::new(move |this: &mut Editor, checked, cx| {
                if checked {
                    // Nothing to remove yet: arm the player for a click.
                    this.inspector.enhance.mode = Mode::Select;
                    cx.notify();
                } else {
                    this.inspector.enhance.mode = Mode::Off;
                    let result = enhance::enhance_set_removal(&this.state, id.clone(), None);
                    this.enhance_after(&id, result.map(|r| r.job), cx);
                }
            })),
            ..Section::new("Remove object")
        }
        .render(self.collapsed("Remove object"), rows, cx);

        // --- Enhance quality ------------------------------------------------------------
        let refusal = video
            .as_ref()
            .and_then(|v| enh::upscale_refusal(v.width, v.height));
        let mut rows: Vec<AnyElement> = vec![caption(
            "Makes the picture larger and cleaner with Real-ESRGAN (BSD-3-Clause, 5 MB \
             download): less blur, noise and compression blocks. At most 3840 px.",
            TEXT_MUTED,
        )];
        if let Some(why) = &refusal {
            rows.push(caption(why.clone(), TEXT_DIM));
        } else {
            let scales = [None, Some(2u32), Some(4)];
            let current = upscale.as_ref().map(|u| u.scale);
            let editor = cx.entity().downgrade();
            let id = segment.id.clone();
            rows.push(
                SegmentedTabs::new(
                    "enhance-scale",
                    ["Off", "2x", "4x"],
                    scales.iter().position(|s| *s == current).unwrap_or(0),
                )
                .on_select(move |index, _, cx| {
                    let id = id.clone();
                    let _ = editor.update(cx, |this, cx| {
                        let result =
                            enhance::enhance_set_upscale(&this.state, id.clone(), scales[index]);
                        this.enhance_after(&id, result.map(|r| r.job), cx);
                    });
                })
                .into_any_element(),
            );
            if let (Some(up), Some(v)) = (&upscale, &video) {
                let chain = Chain {
                    removal: None,
                    upscale: Some(up.clone()),
                };
                let (w, h) = chain.out_size(v.width, v.height);
                rows.push(caption(format!("Made at {w}×{h}."), TEXT_DIM));
                rows.push(caption(estimate(chain), TEXT_MUTED));
            }
        }
        let quality_section =
            Section::new("Enhance quality").render(self.collapsed("Enhance quality"), rows, cx);

        // --- Progress ---------------------------------------------------------------------
        let mut progress: Vec<AnyElement> = Vec::new();
        match &self.inspector.enhance.bake {
            Some(bake) if bake.segment_id == segment.id => {
                progress.push(caption(
                    format!("{}\u{2026} {} of {}", bake.stage, bake.done, bake.total),
                    TEXT_DIM,
                ));
                progress.push(
                    gpui::component::progress::Progress::new("enhance-progress")
                        .value(bake.done as f32 / bake.total.max(1) as f32 * 100.0)
                        .into_any_element(),
                );
                if let Some(warning) = &bake.warning {
                    progress.push(caption(warning.clone(), WARNING));
                }
                let job = bake.job;
                progress.push(right(
                    Button::new("enhance-cancel")
                        .small()
                        .label("Stop")
                        .on_click(move |_, _, _| enhance::enhance_cancel(job)),
                ));
            }
            _ if removal.is_some() || upscale.is_some() => {
                let id = segment.id.clone();
                progress.push(right(
                    Button::new("enhance-rebake")
                        .small()
                        .label("Finish missing frames")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let result = enhance::enhance_bake(&this.state, id.clone());
                            this.enhance_after(&id, result, cx);
                        })),
                ));
            }
            _ => {}
        }

        div()
            .flex()
            .flex_col()
            .child(remove_section)
            .child(quality_section)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .px(px(PAD))
                    .py(px(8.0))
                    .children(progress),
            )
            .into_any_element()
    }

    /// After a command that may have started a bake for `segment_id`.
    fn enhance_after(
        &mut self,
        segment_id: &str,
        job: Result<Option<u64>, String>,
        cx: &mut Context<Self>,
    ) {
        self.refresh(cx);
        match job {
            Ok(Some(job)) => {
                self.inspector.enhance.background.retain(|(j, _)| *j != job);
                self.inspector.enhance.bake = Some(Bake {
                    job,
                    segment_id: segment_id.to_string(),
                    stage: "Preparing".into(),
                    done: 0,
                    total: 0,
                    warning: None,
                });
            }
            Ok(None) => {}
            Err(error) => self.report(Err(error), cx),
        }
        cx.notify();
    }

    /// A press position in overlay fractions (0..1, y down).
    fn enhance_point(&self, position: Point<Pixels>) -> Option<[f32; 2]> {
        let b = self.inspector.enhance.overlay.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        if w < 1.0 || h < 1.0 {
            return None;
        }
        Some([
            ((f32::from(position.x) - f32::from(b.origin.x)) / w).clamp(0.0, 1.0),
            ((f32::from(position.y) - f32::from(b.origin.y)) / h).clamp(0.0, 1.0),
        ])
    }

    /// A click of "Select on player": on the object (`keep`, here: remove)
    /// or on a part to keep. Each click re-selects with every click so far
    /// on this frame, as one undo step.
    fn enhance_click(&mut self, position: Point<Pixels>, keep: bool, cx: &mut Context<Self>) {
        let (Some(segment_id), Some([x, y])) =
            (self.selected.clone(), self.enhance_point(position))
        else {
            return;
        };
        let time = self.clock.position();
        let clicks = &mut self.inspector.enhance.clicks;
        if !clicks
            .as_ref()
            .is_some_and(|(id, t, _)| *id == segment_id && *t == time)
        {
            *clicks = Some((segment_id.clone(), time, Vec::new()));
        }
        let points = {
            let (_, _, points) = clicks.as_mut().expect("set above");
            points.push(CanvasPoint { x, y, keep });
            points.clone()
        };
        if !points.iter().any(|p| p.keep) {
            self.status = Some("Click the object to remove first".into());
            cx.notify();
            return;
        }
        if let Some(bake) = self.inspector.enhance.bake.take() {
            enhance::enhance_cancel(bake.job);
        }
        self.status = Some("Selecting the object\u{2026}".into());
        let result = enhance::enhance_select_object(&self.state, segment_id.clone(), time, points);
        self.enhance_after(&segment_id, result.map(|r| r.job), cx);
    }

    /// A painted stroke ends: add it to what the clip removes.
    fn enhance_stroke_end(&mut self, cx: &mut Context<Self>) {
        let Some(points) = self.inspector.enhance.stroke.take() else {
            return;
        };
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        if let Some(bake) = self.inspector.enhance.bake.take() {
            enhance::enhance_cancel(bake.job);
        }
        let radius = BRUSHES[self.inspector.enhance.brush.min(BRUSHES.len() - 1)];
        let time = self.clock.position();
        let result = enhance::enhance_paint(&self.state, segment_id.clone(), time, points, radius);
        self.enhance_after(&segment_id, result.map(|r| r.job), cx);
    }

    /// The player's catch-all while "Select on player" or "Paint on player"
    /// is armed: clicks, or a brush with the stroke drawn as it is painted.
    pub(in crate::editor) fn enhance_overlay(
        &self,
        dw: f32,
        dh: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mode = self.inspector.enhance.mode;
        if mode == Mode::Off || !self.enhance_open() || dw < 4.0 || dh < 4.0 {
            return None;
        }
        let segment = self.selected_segment().map(|(_, s)| s)?;
        self.project.materials.video(&segment.material_id)?;
        let bounds = Rc::clone(&self.inspector.enhance.overlay);
        let time = self.clock.position();
        let dots: Vec<(f32, f32, bool)> = match mode {
            Mode::Select => self
                .inspector
                .enhance
                .clicks
                .as_ref()
                .filter(|(id, t, _)| Some(id) == self.selected.as_ref() && *t == time)
                .map(|(_, _, points)| points.iter().map(|p| (p.x, p.y, p.keep)).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let stroke = self.inspector.enhance.stroke.clone().unwrap_or_default();
        let brush = BRUSHES[self.inspector.enhance.brush.min(BRUSHES.len() - 1)] * dw.min(dh);
        let entity = cx.entity().downgrade();
        let overlay = div()
            .id("enhance-overlay")
            .absolute()
            .top_0()
            .left_0()
            .w(px(dw))
            .h(px(dh))
            .cursor_crosshair()
            .border_2()
            .border_color(rgb(ACCENT))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    match mode {
                        Mode::Select => {
                            this.enhance_click(event.position, !event.modifiers.alt, cx)
                        }
                        Mode::Paint => {
                            if let Some(p) = this.enhance_point(event.position) {
                                this.inspector.enhance.stroke = Some(vec![p]);
                                cx.notify();
                            }
                        }
                        Mode::Off => {}
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if mode == Mode::Select {
                        this.enhance_click(event.position, false, cx);
                    }
                    cx.stop_propagation();
                }),
            )
            .children(dots.into_iter().map(|(x, y, keep)| {
                // Red on the object to remove, green on a part to keep.
                let colour = if keep { 0xe5484d } else { 0x2fbf71 };
                div()
                    .absolute()
                    .left(px(x * dw - 6.0))
                    .top(px(y * dh - 6.0))
                    .size(px(12.0))
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(0xffffff))
                    .bg(rgb(colour))
            }))
            .child(
                canvas(
                    move |b, _, _| bounds.set(b),
                    move |b, _, window, _| {
                        for p in &stroke {
                            let centre =
                                point(b.origin.x + px(p[0] * dw), b.origin.y + px(p[1] * dh));
                            window.paint_quad(disc(centre, brush, rgba(0xe5484d80)));
                        }
                        // Registered on every paint: a press, its moves and
                        // its release can all arrive before the next frame.
                        let moving = entity.clone();
                        let up = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = moving.update(cx, |this, cx| {
                                    let p = this.enhance_point(event.position);
                                    if let (Some(stroke), Some(p)) =
                                        (this.inspector.enhance.stroke.as_mut(), p)
                                    {
                                        stroke.push(p);
                                        cx.notify();
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = up.update(cx, |this, cx| {
                                    if this.inspector.enhance.stroke.is_some() {
                                        this.enhance_stroke_end(cx);
                                    }
                                });
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            );
        Some(overlay.into_any_element())
    }

    /// Called from `refresh` after any edit: bake the frames a trim or an
    /// undo left missing. Reads files, so it runs off the UI thread, and
    /// only when a clip has Remove object or Enhance quality on.
    pub(crate) fn queue_missing_enhanced(&mut self, cx: &mut Context<Self>) {
        let any = self
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.segments)
            .any(|s| Chain::of(&self.project.materials, s).is_some());
        if !any {
            return;
        }
        let state = Arc::clone(&self.state);
        cx.spawn(async move |this, cx| {
            let jobs = cx
                .background_executor()
                .spawn(async move { enhance::enhance_queue_missing(&state) })
                .await;
            let _ = this.update(cx, |editor, cx| match jobs {
                Ok(jobs) => {
                    let panel = editor.inspector.enhance.bake.as_ref().map(|b| b.job);
                    for job in jobs {
                        let known = &mut editor.inspector.enhance.background;
                        if Some(job) != panel && !known.iter().any(|(j, _)| *j == job) {
                            known.push((job, 0));
                        }
                    }
                    cx.notify();
                }
                Err(error) => tracing::debug!(%error, "no enhance re-bake"),
            });
        })
        .detach();
    }

    /// Called from the editor's tick: progress of the panel's bake, and a
    /// new picture whenever frames land. Returns whether anything changed.
    pub(crate) fn poll_enhance(&mut self, cx: &mut Context<Self>) -> bool {
        let mut landed = false;
        let mut ended = false;
        self.inspector.enhance.background.retain_mut(|(job, done)| {
            let Some(status) = enhance::enhance_status(*job) else {
                return false;
            };
            if let Some(Err(error)) = &status.finished {
                tracing::warn!(%error, "a remade-frame re-bake failed");
            }
            if status.finished.is_some() {
                ended = true;
                return false;
            }
            landed |= status.progress.done != *done;
            *done = status.progress.done;
            true
        });
        let mut changed = false;
        if let Some(bake) = &mut self.inspector.enhance.bake {
            match enhance::enhance_status(bake.job) {
                None => {
                    self.inspector.enhance.bake = None;
                    changed = true;
                }
                Some(status) => match status.finished {
                    Some(finished) => {
                        self.inspector.enhance.bake = None;
                        ended = true;
                        changed = true;
                        match finished {
                            Ok(done) if done.cancelled => {
                                self.status =
                                    Some(format!("Stopped after {} frames", done.written).into())
                            }
                            Ok(done) => {
                                self.status = Some(
                                    format!(
                                        "{} frames made in {:.1} s on {}",
                                        done.written,
                                        done.seconds,
                                        done.provider.as_deref().unwrap_or("the CPU")
                                    )
                                    .into(),
                                )
                            }
                            Err(error) => self.report(Err(error), cx),
                        }
                    }
                    None => {
                        let p = &status.progress;
                        landed |= p.done != bake.done;
                        changed |= p.done != bake.done
                            || status.warning != bake.warning
                            || p.stage != bake.stage;
                        bake.done = p.done;
                        bake.total = p.total;
                        bake.stage = p.stage.clone();
                        bake.warning = status.warning;
                    }
                },
            }
        }
        let due = self
            .inspector
            .enhance
            .redrawn
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(300));
        if ended || (landed && due) {
            // A new picture of the same document: frames just made.
            self.inspector.enhance.redrawn = Some(std::time::Instant::now());
            self.generation += 1;
            changed = true;
        }
        changed
    }
}
