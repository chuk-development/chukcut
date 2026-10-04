//! Video › Crop: ratio presets, a free crop box with handles on the player,
//! rotate and flip, reset.
//!
//! The crop is the document's `Segment::crop` (fractions of the source
//! picture), set through `inspector_set_crop` as one undo step. It is not
//! keyframable: the engine stores one rectangle per clip.
//!
//! While the tab is open the player shows the clip **uncropped**
//! ([`Editor::crop_view_project`]), with everything outside the box dimmed,
//! the way CapCut does: the box and its handles live in the source picture's
//! own coordinates, so they stay under the pointer while it moves. A drag
//! changes only the box on screen; the document gets the new crop on
//! release.

use std::cell::Cell;
use std::rc::Rc;

use chukcut_engine::modules::project::document::Crop;
use chukcut_engine::modules::project::{Project, Segment};
use chukcut_engine::modules::render::layout;
use gpui::assets::IconName as Lucide;
use gpui::{canvas, point, AnyElement, DispatchPhase, PathBuilder};

use super::controls::*;
use super::grading::{disc, ring};
use super::*;

/// The sub-tab's name.
pub(super) const CROP: &str = "Crop";

/// How close to a handle a press must land, in pixels.
const GRAB_PX: f32 = 10.0;
/// Room around the picture where a handle on its edge can still be grabbed.
const MARGIN: f32 = 24.0;
/// The smallest box, as a fraction of the picture's side.
const MIN_SIDE: f32 = 0.02;

/// A shape the box can keep.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Ratio {
    /// Any shape.
    Free,
    /// The picture's own shape: no crop.
    Original,
    /// Width to height, in pixels.
    Fixed(f32, f32),
}

const RATIOS: [(&str, Ratio); 9] = [
    ("Free", Ratio::Free),
    ("Original", Ratio::Original),
    ("9:16", Ratio::Fixed(9.0, 16.0)),
    ("16:9", Ratio::Fixed(16.0, 9.0)),
    ("1:1", Ratio::Fixed(1.0, 1.0)),
    ("4:5", Ratio::Fixed(4.0, 5.0)),
    ("4:3", Ratio::Fixed(4.0, 3.0)),
    ("3:4", Ratio::Fixed(3.0, 4.0)),
    ("2.35:1", Ratio::Fixed(2.35, 1.0)),
];

/// The tab's state between frames.
#[derive(Default)]
pub(crate) struct CropTab {
    /// The ratio picked for the selected clip; `None` reads it off the crop.
    ratio: Option<(String, Ratio)>,
    drag: Option<CropDrag>,
    /// The picture's bounds on the player, for turning presses into pixels.
    overlay: Rc<Cell<Bounds<Pixels>>>,
    /// Whether the player last drew the uncropped view, so a change of tab
    /// re-renders it.
    shown: bool,
}

/// A press on the box, held.
struct CropDrag {
    segment_id: String,
    handle: Handle,
    /// The box when the press landed, `[left, top, right, bottom]`.
    start: [f32; 4],
    /// Where the press landed, in source fractions.
    grab: [f32; 2],
    /// The box as dragged so far.
    current: [f32; 4],
}

/// What a press grabbed. Signs say which side: −1 left or top, +1 right or
/// bottom, 0 neither.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    Move,
    Corner(i8, i8),
    Edge(i8, i8),
}

const HANDLES: [Handle; 8] = [
    Handle::Corner(-1, -1),
    Handle::Corner(1, -1),
    Handle::Corner(1, 1),
    Handle::Corner(-1, 1),
    Handle::Edge(0, -1),
    Handle::Edge(1, 0),
    Handle::Edge(0, 1),
    Handle::Edge(-1, 0),
];

fn edges(crop: Option<Crop>) -> [f32; 4] {
    crop.map_or([0.0, 0.0, 1.0, 1.0], |c| [c.left, c.top, c.right, c.bottom])
}

fn to_crop(r: [f32; 4]) -> Option<Crop> {
    let full = r[0] <= 0.0005 && r[1] <= 0.0005 && r[2] >= 0.9995 && r[3] >= 0.9995;
    (!full).then_some(Crop {
        left: r[0],
        top: r[1],
        right: r[2],
        bottom: r[3],
    })
}

/// Where a handle sits, in source fractions.
fn handle_at(r: [f32; 4], handle: Handle) -> [f32; 2] {
    let x = |s: i8| match s {
        -1 => r[0],
        1 => r[2],
        _ => (r[0] + r[2]) * 0.5,
    };
    let y = |s: i8| match s {
        -1 => r[1],
        1 => r[3],
        _ => (r[1] + r[3]) * 0.5,
    };
    match handle {
        Handle::Corner(sx, sy) | Handle::Edge(sx, sy) => [x(sx), y(sy)],
        Handle::Move => [x(0), y(0)],
    }
}

/// The largest box of `aspect` (width over height, in source fractions)
/// around the centre of `r`, kept inside the picture.
fn fitted(r: [f32; 4], aspect: f32) -> [f32; 4] {
    let (mut w, mut h) = (1.0f32, 1.0 / aspect);
    if h > 1.0 {
        h = 1.0;
        w = aspect;
    }
    let cx = ((r[0] + r[2]) * 0.5).clamp(w * 0.5, 1.0 - w * 0.5);
    let cy = ((r[1] + r[3]) * 0.5).clamp(h * 0.5, 1.0 - h * 0.5);
    [cx - w * 0.5, cy - h * 0.5, cx + w * 0.5, cy + h * 0.5]
}

/// The box after dragging `handle` from `grab` to `at`. `aspect` is the
/// shape to keep (width over height, in source fractions), if any.
fn dragged(
    start: [f32; 4],
    handle: Handle,
    grab: [f32; 2],
    at: [f32; 2],
    aspect: Option<f32>,
) -> [f32; 4] {
    let [l, t, r, b] = start;
    let (dx, dy) = (at[0] - grab[0], at[1] - grab[1]);
    match handle {
        Handle::Move => {
            let dx = dx.clamp(-l, 1.0 - r);
            let dy = dy.clamp(-t, 1.0 - b);
            [l + dx, t + dy, r + dx, b + dy]
        }
        Handle::Corner(sx, sy) => {
            // The opposite corner stays where it is.
            let anchor = [if sx < 0 { r } else { l }, if sy < 0 { b } else { t }];
            let moving = handle_at(start, handle);
            let room = [
                if sx < 0 { anchor[0] } else { 1.0 - anchor[0] },
                if sy < 0 { anchor[1] } else { 1.0 - anchor[1] },
            ];
            let mut w = ((moving[0] + dx - anchor[0]) * f32::from(sx)).clamp(MIN_SIDE, room[0]);
            let mut h = ((moving[1] + dy - anchor[1]) * f32::from(sy)).clamp(MIN_SIDE, room[1]);
            if let Some(aspect) = aspect {
                w = w.max(h * aspect);
                h = w / aspect;
                if w > room[0] {
                    w = room[0];
                    h = w / aspect;
                }
                if h > room[1] {
                    h = room[1];
                    w = h * aspect;
                }
            }
            let (x0, x1) = if sx < 0 {
                (anchor[0] - w, anchor[0])
            } else {
                (anchor[0], anchor[0] + w)
            };
            let (y0, y1) = if sy < 0 {
                (anchor[1] - h, anchor[1])
            } else {
                (anchor[1], anchor[1] + h)
            };
            [x0, y0, x1, y1]
        }
        Handle::Edge(sx, 0) => {
            let anchor = if sx < 0 { r } else { l };
            let room = if sx < 0 { anchor } else { 1.0 - anchor };
            let edge = if sx < 0 { l } else { r };
            let mut w = ((edge + dx - anchor) * f32::from(sx)).clamp(MIN_SIDE, room);
            let (mut y0, mut y1) = (t, b);
            if let Some(aspect) = aspect {
                // The other side follows, around its own centre.
                let cy = (t + b) * 0.5;
                let room_y = 2.0 * cy.min(1.0 - cy);
                let mut h = w / aspect;
                if h > room_y {
                    h = room_y;
                    w = h * aspect;
                }
                y0 = cy - h * 0.5;
                y1 = cy + h * 0.5;
            }
            let (x0, x1) = if sx < 0 {
                (anchor - w, anchor)
            } else {
                (anchor, anchor + w)
            };
            [x0, y0, x1, y1]
        }
        Handle::Edge(_, sy) => {
            let anchor = if sy < 0 { b } else { t };
            let room = if sy < 0 { anchor } else { 1.0 - anchor };
            let edge = if sy < 0 { t } else { b };
            let mut h = ((edge + dy - anchor) * f32::from(sy)).clamp(MIN_SIDE, room);
            let (mut x0, mut x1) = (l, r);
            if let Some(aspect) = aspect {
                let cx = (l + r) * 0.5;
                let room_x = 2.0 * cx.min(1.0 - cx);
                let mut w = h * aspect;
                if w > room_x {
                    w = room_x;
                    h = w / aspect;
                }
                x0 = cx - w * 0.5;
                x1 = cx + w * 0.5;
            }
            let (y0, y1) = if sy < 0 {
                (anchor - h, anchor)
            } else {
                (anchor, anchor + h)
            };
            [x0, y0, x1, y1]
        }
    }
}

/// A picture's size as the clip shows it: a video's coded size turned by its
/// rotation tag, or an image's.
fn source_size(project: &Project, segment: &Segment) -> Option<(u32, u32)> {
    let pool = &project.materials;
    if let Some(video) = pool.video(&segment.material_id) {
        return Some(if video.rotation.rem_euclid(180) == 90 {
            (video.height, video.width)
        } else {
            (video.width, video.height)
        });
    }
    pool.image(&segment.material_id)
        .map(|i| (i.width, i.height))
}

/// Source fractions to player pixels for the clip drawn uncropped, as the
/// compositor places it at `time`.
#[derive(Clone, Copy, Debug)]
struct SourceFrame {
    /// The unit quad to pixels: `px = a · (x, y) + b`.
    a: [f32; 4],
    b: [f32; 2],
    /// The part of the source the quad shows (a stabilised clip's window).
    uv: [f32; 4],
}

impl SourceFrame {
    fn of(project: &Project, segment: &Segment, time: Micros, size: (f32, f32)) -> Option<Self> {
        let source = source_size(project, segment)?;
        let mut uncropped = segment.clone();
        uncropped.crop = None;
        let followed =
            chukcut_engine::modules::tracking::follow::resolve(project, &uncropped, time);
        let resolved =
            chukcut_engine::modules::analysis::stabilise::resolve(project, followed, time);
        let keyed = layout::animated_transform(&resolved, time);
        let mut transform = chukcut_engine::modules::motion::clip_motion(
            &project.materials,
            &resolved,
            time,
            keyed,
        )
        .map_or(keyed, |m| m.transform);
        transform.opacity = 1.0;
        let canvas = (project.canvas.width, project.canvas.height);
        let placed = layout::place_quad(canvas, source, &transform, resolved.crop)?;
        let m = placed.mvp;
        let (dw, dh) = size;
        Some(Self {
            a: [
                m[0] * dw * 0.5,
                m[4] * dw * 0.5,
                -m[1] * dh * 0.5,
                -m[5] * dh * 0.5,
            ],
            b: [(m[12] + 1.0) * 0.5 * dw, (1.0 - m[13]) * 0.5 * dh],
            uv: placed.crop,
        })
    }

    /// A source point (fractions, y down) on the player.
    fn pixel(&self, p: [f32; 2]) -> [f32; 2] {
        let x = (p[0] - self.uv[0]) / (self.uv[2] - self.uv[0]).max(1e-6) - 0.5;
        let y = 0.5 - (p[1] - self.uv[1]) / (self.uv[3] - self.uv[1]).max(1e-6);
        [
            self.a[0] * x + self.a[1] * y + self.b[0],
            self.a[2] * x + self.a[3] * y + self.b[1],
        ]
    }

    /// A player pixel as a source point; outside the picture too.
    fn source(&self, px: [f32; 2]) -> Option<[f32; 2]> {
        let det = self.a[0] * self.a[3] - self.a[1] * self.a[2];
        if det.abs() < 1e-6 {
            return None;
        }
        let (dx, dy) = (px[0] - self.b[0], px[1] - self.b[1]);
        let x = (self.a[3] * dx - self.a[1] * dy) / det;
        let y = (-self.a[2] * dx + self.a[0] * dy) / det;
        Some([
            self.uv[0] + (x + 0.5) * (self.uv[2] - self.uv[0]),
            self.uv[1] + (0.5 - y) * (self.uv[3] - self.uv[1]),
        ])
    }
}

impl Editor {
    /// The clip whose crop the player shows uncropped: the selected video or
    /// image clip, while Video › Crop is open.
    fn crop_view_segment(&self) -> Option<&Segment> {
        if self.inspector.tab.unwrap_or("Video") != "Video"
            || self.inspector.sub_tab.get("Video").copied() != Some(CROP)
        {
            return None;
        }
        let (_, segment) = self.selected_segment()?;
        source_size(&self.project, segment)?;
        (!self.project.materials.is_effect_clip(segment)).then_some(segment)
    }

    /// `project`, or a copy with the cropped clip shown whole while the
    /// Crop tab is open. Runtime only: never stored, never exported.
    pub(crate) fn crop_view_project(&self, project: Arc<Project>) -> Arc<Project> {
        let Some(id) = self
            .crop_view_segment()
            .filter(|s| s.crop.is_some())
            .map(|s| s.id.clone())
        else {
            return project;
        };
        let mut copy = (*project).clone();
        if let Some(segment) = copy
            .tracks
            .iter_mut()
            .flat_map(|t| t.segments.iter_mut())
            .find(|s| s.id == id)
        {
            segment.crop = None;
        }
        Arc::new(copy)
    }

    /// A new picture when the player switches between the cropped and the
    /// uncropped view. Called before every preview request.
    pub(crate) fn sync_crop_view(&mut self) {
        let shown = self.crop_view_segment().is_some();
        if shown != self.inspector.crop.shown {
            self.inspector.crop.shown = shown;
            self.generation += 1;
        }
    }

    /// The box's shape in source fractions for `ratio` on `segment`.
    fn crop_aspect(&self, segment: &Segment, ratio: Ratio) -> Option<f32> {
        let (w, h) = source_size(&self.project, segment)?;
        match ratio {
            Ratio::Fixed(rw, rh) => Some(rw / rh * h.max(1) as f32 / w.max(1) as f32),
            _ => None,
        }
    }

    /// The ratio the tab lights: the one picked for this clip, else the one
    /// the crop has.
    fn crop_ratio(&self, segment: &Segment) -> Ratio {
        let shape = segment.crop.map(|crop| {
            source_size(&self.project, segment).map(|(w, h)| {
                (crop.right - crop.left) * w as f32 / ((crop.bottom - crop.top) * h as f32)
            })
        });
        let fits = |rw: f32, rh: f32| matches!(shape, Some(Some(shape)) if (shape / (rw / rh) - 1.0).abs() < 0.01);
        // The pick holds while the crop still has its shape: an undo that
        // takes a 9:16 crop back must not leave 9:16 lit over a free box.
        if let Some((id, ratio)) = &self.inspector.crop.ratio {
            let holds = match ratio {
                Ratio::Fixed(rw, rh) => fits(*rw, *rh),
                Ratio::Original => shape.is_none(),
                _ => true,
            };
            if *id == segment.id && holds {
                return *ratio;
            }
        }
        match shape {
            None => Ratio::Original,
            Some(None) => Ratio::Free,
            Some(Some(_)) => RATIOS
                .iter()
                .find_map(|(_, r)| match r {
                    Ratio::Fixed(rw, rh) if fits(*rw, *rh) => Some(*r),
                    _ => None,
                })
                .unwrap_or(Ratio::Free),
        }
    }

    fn set_crop(&mut self, segment_id: String, crop: Option<Crop>, cx: &mut Context<Self>) {
        let unchanged = self
            .project
            .segment(&segment_id)
            .is_some_and(|(_, s)| edges(s.crop) == edges(crop));
        if unchanged {
            cx.notify();
            return;
        }
        let result = inspector_commands::inspector_set_crop(&self.state, segment_id, crop);
        self.refresh(cx);
        self.report(result.map(|_| ()), cx);
    }

    fn pick_ratio(&mut self, ratio: Ratio, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let segment = segment.clone();
        self.inspector.crop.ratio = Some((segment.id.clone(), ratio));
        match ratio {
            Ratio::Free => cx.notify(),
            Ratio::Original => self.set_crop(segment.id.clone(), None, cx),
            Ratio::Fixed(..) => {
                let Some(aspect) = self.crop_aspect(&segment, ratio) else {
                    return;
                };
                let r = fitted(edges(segment.crop), aspect);
                self.set_crop(segment.id.clone(), to_crop(r), cx);
            }
        }
    }

    fn flip(&mut self, horizontal: bool, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let before = segment.transform;
        let mut after = before;
        if horizontal {
            after.flip_h = !after.flip_h;
        } else {
            after.flip_v = !after.flip_v;
        }
        let command = EditCommand::SetTransform {
            segment_id: segment.id.clone(),
            before,
            after,
        };
        self.apply(Ok(command), cx);
    }

    /// A quarter turn, at the playhead when rotation has keyframes.
    fn quarter_turn(&mut self, clockwise: bool, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let current = self.prop_value(Prop::Rotation, segment);
        let turned = current + if clockwise { 90.0 } else { -90.0 };
        // Kept within one turn either way, which the field can show.
        let turned = if turned > 360.0 {
            turned - 360.0
        } else if turned < -360.0 {
            turned + 360.0
        } else {
            turned
        };
        self.set_prop(Prop::Rotation, turned, Phase::Commit, cx);
    }

    fn reset_rotate_and_flip(&mut self, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let before = segment.transform;
        let has_keys = segment
            .keyframes
            .iter()
            .any(|t| t.property == chukcut_engine::modules::project::AnimatableProperty::Rotation);
        if has_keys {
            // Keyframed rotation is the Transform section's to reset.
            self.set_prop(Prop::Rotation, 0.0, Phase::Commit, cx);
        }
        let after = Transform {
            flip_h: false,
            flip_v: false,
            rotation: if has_keys { before.rotation } else { 0.0 },
            ..before
        };
        let changed = after.flip_h != before.flip_h
            || after.flip_v != before.flip_v
            || after.rotation != before.rotation;
        if changed {
            let Some((_, segment)) = self.selected_segment() else {
                return;
            };
            let command = EditCommand::SetTransform {
                segment_id: segment.id.clone(),
                before: segment.transform,
                after,
            };
            self.apply(Ok(command), cx);
        }
    }

    pub(super) fn crop_tab(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lit = self.crop_ratio(segment);
        let ratio_buttons = div().flex().flex_row().flex_wrap().gap(px(6.0)).children(
            RATIOS.iter().enumerate().map(|(i, (label, ratio))| {
                let ratio = *ratio;
                panel_button(
                    ("crop-ratio", i),
                    *label,
                    ratio == lit,
                    true,
                    cx.listener(move |this, _, _, cx| this.pick_ratio(ratio, cx)),
                )
            }),
        );
        let kept = source_size(&self.project, segment).map(|(w, h)| {
            let r = edges(segment.crop);
            let (kw, kh) = (
                ((r[2] - r[0]) * w as f32).round() as u32,
                ((r[3] - r[1]) * h as f32).round() as u32,
            );
            if segment.crop.is_some() {
                format!("Keeps {kw} \u{d7} {kh} px of {w} \u{d7} {h}.")
            } else {
                format!("The whole picture, {w} \u{d7} {h} px.")
            }
        });
        let crop_rows = vec![
            ratio_buttons.into_any_element(),
            div()
                .text_size(px(TEXT_CAPTION))
                .text_color(rgb(TEXT_MUTED))
                .child(format!(
                    "Drag the box or its handles on the player. {}",
                    kept.unwrap_or_default()
                ))
                .into_any_element(),
        ];
        let id = segment.id.clone();
        let crop = Section {
            on_reset: segment.crop.is_some().then(|| {
                Box::new(move |this: &mut Editor, cx: &mut Context<Editor>| {
                    this.inspector.crop.ratio = None;
                    this.set_crop(id.clone(), None, cx);
                }) as ResetHandler
            }),
            ..Section::new(CROP)
        }
        .render(self.collapsed(CROP), crop_rows, cx);

        let buttons = [
            (Lucide::RotateCcw, "Rotate 90° left", 0u8),
            (Lucide::RotateCw, "Rotate 90° right", 1),
            (Lucide::FlipHorizontal2, "Flip horizontally", 2),
            (Lucide::FlipVertical2, "Flip vertically", 3),
        ];
        let flips = (segment.transform.flip_h, segment.transform.flip_v);
        let turn_row = div()
            .flex()
            .flex_row()
            .gap(px(4.0))
            .children(buttons.into_iter().map(|(glyph, tip, action)| {
                let on = match action {
                    2 => flips.0,
                    3 => flips.1,
                    _ => false,
                };
                crate::ui::IconButton::new(("crop-turn", usize::from(action)), glyph)
                    .tooltip(tip)
                    .toggled(on)
                    .on_click(cx.listener(move |this, _, _, cx| match action {
                        0 => this.quarter_turn(false, cx),
                        1 => this.quarter_turn(true, cx),
                        2 => this.flip(true, cx),
                        _ => this.flip(false, cx),
                    }))
            }))
            .into_any_element();
        let rotate_rows = vec![
            turn_row,
            self.field_row("Rotate", &[(Prop::Rotation, None)], segment, window, cx),
        ];
        let turned = segment.transform.rotation != 0.0 || flips.0 || flips.1;
        let rotate = Section {
            on_reset: turned.then(|| {
                Box::new(|this: &mut Editor, cx: &mut Context<Editor>| {
                    this.reset_rotate_and_flip(cx)
                }) as ResetHandler
            }),
            ..Section::new("Rotate and flip")
        }
        .render(self.collapsed("Rotate and flip"), rotate_rows, cx);

        div()
            .flex()
            .flex_col()
            .child(crop)
            .child(rotate)
            .into_any_element()
    }

    /// The box on the player, while Video › Crop is open.
    pub(in crate::editor) fn crop_overlay(
        &self,
        dw: f32,
        dh: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if dw < 4.0 || dh < 4.0 {
            return None;
        }
        let segment = self.crop_view_segment()?;
        let frame = SourceFrame::of(&self.project, segment, self.clock.position(), (dw, dh))?;
        let r = match &self.inspector.crop.drag {
            Some(drag) if drag.segment_id == segment.id => drag.current,
            _ => edges(segment.crop),
        };
        let corners = |q: [f32; 4]| -> [[f32; 2]; 4] {
            [
                frame.pixel([q[0], q[1]]),
                frame.pixel([q[2], q[1]]),
                frame.pixel([q[2], q[3]]),
                frame.pixel([q[0], q[3]]),
            ]
        };
        // Everything outside the box, as four bands of the picture.
        let shade: Vec<[[f32; 2]; 4]> = [
            [0.0, 0.0, 1.0, r[1]],
            [0.0, r[3], 1.0, 1.0],
            [0.0, r[1], r[0], r[3]],
            [r[2], r[1], 1.0, r[3]],
        ]
        .into_iter()
        .filter(|q| q[2] - q[0] > 1e-4 && q[3] - q[1] > 1e-4)
        .map(corners)
        .collect();
        let outline = corners(r);
        let thirds: Vec<[[f32; 2]; 2]> = (1..3)
            .flat_map(|i| {
                let f = i as f32 / 3.0;
                let x = r[0] + (r[2] - r[0]) * f;
                let y = r[1] + (r[3] - r[1]) * f;
                [
                    [frame.pixel([x, r[1]]), frame.pixel([x, r[3]])],
                    [frame.pixel([r[0], y]), frame.pixel([r[2], y])],
                ]
            })
            .collect();
        let dots: Vec<(Handle, [f32; 2])> = HANDLES
            .iter()
            .map(|&h| (h, frame.pixel(handle_at(r, h))))
            .collect();
        let bounds = Rc::clone(&self.inspector.crop.overlay);
        let entity = cx.entity().downgrade();
        let overlay = div()
            .id("crop-overlay")
            .absolute()
            .top(px(-MARGIN))
            .left(px(-MARGIN))
            .w(px(dw + 2.0 * MARGIN))
            .h(px(dh + 2.0 * MARGIN))
            .cursor_move()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.crop_press(event.position, (dw, dh), cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                canvas(
                    move |b, _, _| bounds.set(b),
                    move |b, _, window, _| {
                        let at = |p: [f32; 2]| point(b.origin.x + px(p[0]), b.origin.y + px(p[1]));
                        let polygon = |points: &[[f32; 2]], fill: bool, width: f32| {
                            let mut path = if fill {
                                PathBuilder::fill()
                            } else {
                                PathBuilder::stroke(px(width))
                            };
                            path.move_to(at(points[0]));
                            for p in &points[1..] {
                                path.line_to(at(*p));
                            }
                            path.line_to(at(points[0]));
                            path.build().ok()
                        };
                        for band in &shade {
                            if let Some(path) = polygon(band, true, 0.0) {
                                window.paint_path(path, gpui::black().opacity(0.6));
                            }
                        }
                        for line in &thirds {
                            if let Some(path) = polygon(line, false, 1.0) {
                                window.paint_path(path, gpui::white().opacity(0.35));
                            }
                        }
                        if let Some(path) = polygon(&outline, false, 1.5) {
                            window.paint_path(path, rgb(ACCENT));
                        }
                        for (handle, p) in &dots {
                            let centre = at(*p);
                            let radius = if matches!(handle, Handle::Corner(..)) {
                                5.5
                            } else {
                                4.0
                            };
                            window.paint_quad(disc(centre, radius, rgb(TEXT)));
                            window.paint_quad(ring(centre, radius, rgb(ACCENT), 1.5));
                        }
                        // Registered on every paint: a press, its moves and its
                        // release can all arrive before the next frame.
                        let moving = entity.clone();
                        let up = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = moving.update(cx, |this, cx| {
                                    if this.inspector.crop.drag.is_some() {
                                        this.crop_drag(event.position, (dw, dh), false, cx)
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = up.update(cx, |this, cx| {
                                    if this.inspector.crop.drag.is_some() {
                                        this.crop_drag(event.position, (dw, dh), true, cx)
                                    }
                                });
                            }
                        });
                    },
                )
                .absolute()
                .top(px(MARGIN))
                .left(px(MARGIN))
                .w(px(dw))
                .h(px(dh)),
            );
        Some(overlay.into_any_element())
    }

    /// A window point in picture pixels.
    fn crop_point(&self, position: Point<Pixels>) -> [f32; 2] {
        let b = self.inspector.crop.overlay.get();
        [
            f32::from(position.x) - f32::from(b.origin.x),
            f32::from(position.y) - f32::from(b.origin.y),
        ]
    }

    /// A press on the player: grab the nearest handle, or the box itself.
    fn crop_press(&mut self, position: Point<Pixels>, size: (f32, f32), cx: &mut Context<Self>) {
        let Some(segment) = self.crop_view_segment().cloned() else {
            return;
        };
        let Some(frame) = SourceFrame::of(&self.project, &segment, self.clock.position(), size)
        else {
            return;
        };
        let at = self.crop_point(position);
        let Some(grab) = frame.source(at) else {
            return;
        };
        let r = edges(segment.crop);
        let handle = HANDLES
            .iter()
            .map(|&h| {
                let p = frame.pixel(handle_at(r, h));
                (h, (p[0] - at[0]).hypot(p[1] - at[1]))
            })
            .filter(|(_, d)| *d <= GRAB_PX)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h);
        let inside = grab[0] >= r[0] && grab[0] <= r[2] && grab[1] >= r[1] && grab[1] <= r[3];
        let Some(handle) = handle.or(inside.then_some(Handle::Move)) else {
            return;
        };
        self.inspector.crop.drag = Some(CropDrag {
            segment_id: segment.id.clone(),
            handle,
            start: r,
            grab,
            current: r,
        });
        cx.notify();
    }

    /// The pointer moved with the box held, or let go of it (`commit`).
    fn crop_drag(
        &mut self,
        position: Point<Pixels>,
        size: (f32, f32),
        commit: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.inspector.crop.drag.as_ref() else {
            return;
        };
        let (segment_id, handle, start, grab) =
            (drag.segment_id.clone(), drag.handle, drag.start, drag.grab);
        let Some((_, segment)) = self.project.segment(&segment_id) else {
            self.inspector.crop.drag = None;
            return;
        };
        let segment = segment.clone();
        let frame = SourceFrame::of(&self.project, &segment, self.clock.position(), size);
        let at = frame.and_then(|f| f.source(self.crop_point(position)));
        let aspect = self.crop_aspect(&segment, self.crop_ratio(&segment));
        let current = at.map_or(start, |at| dragged(start, handle, grab, at, aspect));
        if !commit {
            if let Some(drag) = self.inspector.crop.drag.as_mut() {
                drag.current = current;
            }
            cx.notify();
            return;
        }
        self.inspector.crop.drag = None;
        self.set_crop(segment_id, to_crop(current), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn a_ratio_takes_the_largest_box_around_the_centre() {
        // A square in a 16:9 picture: as tall as the picture, centred.
        let aspect = 9.0 / 16.0;
        let r = fitted([0.0, 0.0, 1.0, 1.0], aspect);
        assert!(close(r, [0.21875, 0.0, 0.78125, 1.0]), "{r:?}");
        // Centred on a box near the edge, it moves inside the picture.
        let r = fitted([0.9, 0.2, 1.0, 0.4], aspect);
        assert!(close(r, [0.4375, 0.0, 1.0, 1.0]), "{r:?}");
    }

    #[test]
    fn a_corner_drag_keeps_the_opposite_corner_and_the_shape() {
        let start = [0.2, 0.2, 0.8, 0.8];
        let free = dragged(start, Handle::Corner(1, 1), [0.8, 0.8], [0.6, 0.9], None);
        assert!(close(free, [0.2, 0.2, 0.6, 0.9]), "{free:?}");
        let square = dragged(
            start,
            Handle::Corner(1, 1),
            [0.8, 0.8],
            [0.6, 0.9],
            Some(1.0),
        );
        assert!(close(square, [0.2, 0.2, 0.9, 0.9]), "{square:?}");
        // Past the picture it stops at the edge, still square.
        let clamped = dragged(
            start,
            Handle::Corner(-1, -1),
            [0.2, 0.2],
            [-0.5, 0.1],
            Some(1.0),
        );
        assert!(close(clamped, [0.0, 0.0, 0.8, 0.8]), "{clamped:?}");
    }

    #[test]
    fn edges_move_alone_and_a_box_never_vanishes_or_leaves() {
        let start = [0.2, 0.2, 0.8, 0.8];
        let left = dragged(start, Handle::Edge(-1, 0), [0.2, 0.5], [0.1, 0.5], None);
        assert!(close(left, [0.1, 0.2, 0.8, 0.8]), "{left:?}");
        let crossed = dragged(start, Handle::Edge(1, 0), [0.8, 0.5], [0.0, 0.5], None);
        assert!(
            (crossed[2] - crossed[0] - MIN_SIDE).abs() < 1e-5,
            "{crossed:?}"
        );
        let moved = dragged(start, Handle::Move, [0.5, 0.5], [0.9, 0.0], None);
        assert!(close(moved, [0.4, 0.0, 1.0, 0.6]), "{moved:?}");
        // An edge with a shape kept grows the other side around its centre.
        let wide = dragged(start, Handle::Edge(0, 1), [0.5, 0.8], [0.5, 0.7], Some(2.0));
        assert!(close(wide, [0.0, 0.2, 1.0, 0.7]), "{wide:?}");
    }

    #[test]
    fn the_full_picture_is_no_crop() {
        assert!(to_crop([0.0, 0.0, 1.0, 1.0]).is_none());
        assert!(to_crop([0.0, 0.0, 1.0, 0.9]).is_some());
        assert_eq!(edges(None), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn a_source_point_survives_the_trip_to_the_player_and_back() {
        let frame = SourceFrame {
            a: [100.0, 20.0, -10.0, -80.0],
            b: [160.0, 90.0],
            uv: [0.1, 0.0, 0.9, 1.0],
        };
        let p = [0.3, 0.7];
        let back = frame.source(frame.pixel(p)).unwrap();
        assert!((back[0] - p[0]).abs() < 1e-4 && (back[1] - p[1]).abs() < 1e-4);
    }
}
