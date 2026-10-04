//! Masks, the chroma key and the blend mode: the Video tab's Mask and
//! Remove background sub-tabs, the Blend row of Basic, and the mask handles
//! and the eyedropper on the player.
//!
//! Every value goes through a `compositing_*` engine command, so it is one
//! undo step. A slider or a handle drag previews the way the rest of the
//! inspector does: the edit is built against the document as it was when the
//! drag started and applied to a copy the preview renders; on release the
//! copy is dropped and the edit is committed once.

use std::collections::HashMap;

use chukcut_engine::modules::compositing::commands as compositing;
use chukcut_engine::modules::compositing::edit::{self as comp_edit, Minted, ResetPart};
use chukcut_engine::modules::project::compositing::{
    BlendMode, ChromaKey, CompositingMaterial, Mask, MaskOp, MaskShape, FEATHER_SPAN,
};
use chukcut_engine::modules::render::layout;
use chukcut_engine::modules::render::matte::{heart_polygon, star_polygon};
use gpui::assets::IconName;
use gpui::component::button::Button;
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::component::switch::Switch;
use gpui::component::Sizable as _;
use gpui::{canvas, point, AnyElement, DispatchPhase, PathBuilder, Subscription};

use crate::ui::{self, ColorEvent, ColorPicker, EmptyState, IconButton};

use super::controls::*;
use super::grading::{disc, ring};
use super::*;

/// The slider key of the chroma key's controls; masks use their index.
const KEY: usize = usize::MAX;
/// How close a press must land to a handle to grab it, in pixels.
const GRAB_PX: f32 = 10.0;
/// How far the rotate handle sits beyond the top edge, in short-side units.
const ROTATE_GAP: f32 = 0.12;
/// How far the feather handle sits below the bottom edge at no feather.
const FEATHER_GAP: f32 = 0.06;
/// How far past the picture the mask overlay still takes presses, in pixels.
const HANDLE_MARGIN: f32 = 48.0;

// --- glyphs ------------------------------------------------------------------------

mod glyphs {
    macro_rules! icon {
        ($name:ident, $body:literal) => {
            pub const $name: &[u8] = concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</svg>"
            )
            .as_bytes();
        };
    }
    icon!(
        LINEAR,
        r#"<path d="M3 12h18"/><path d="M6 15l3 3M10 15l3 3M14 15l3 3" stroke-width="1.25"/>"#
    );
    icon!(MIRROR, r#"<path d="M3 8h18M3 16h18"/>"#);
    icon!(ELLIPSE, r#"<circle cx="12" cy="12" r="8"/>"#);
    icon!(
        RECTANGLE,
        r#"<rect x="4" y="6" width="16" height="12" rx="1.5"/>"#
    );
    icon!(
        STAR,
        r#"<path d="M12 3.5l2.6 5.6 6 .7-4.5 4.1 1.3 6-5.4-3.1-5.4 3.1 1.3-6-4.5-4.1 6-.7z"/>"#
    );
    icon!(
        HEART,
        r#"<path d="M12 20s-7.5-4.6-7.5-10A4.3 4.3 0 0 1 12 7.4 4.3 4.3 0 0 1 19.5 10c0 5.4-7.5 10-7.5 10z"/>"#
    );
    icon!(
        EYEDROPPER,
        r#"<path d="M14.5 4.5l5 5M17 2.5a2 2 0 0 1 2.8 0l1.7 1.7a2 2 0 0 1 0 2.8l-2.5 2.5-4.5-4.5z"/><path d="M14 7l-9 9v3h3l9-9"/>"#
    );
}

fn shape_glyph(shape: &MaskShape) -> &'static [u8] {
    match shape {
        MaskShape::Linear => glyphs::LINEAR,
        MaskShape::Mirror => glyphs::MIRROR,
        MaskShape::Rectangle => glyphs::RECTANGLE,
        MaskShape::Star => glyphs::STAR,
        MaskShape::Heart => glyphs::HEART,
        _ => glyphs::ELLIPSE,
    }
}

// --- state -------------------------------------------------------------------------

struct ParamSlider {
    state: Entity<SliderState>,
    _subscription: Subscription,
}

/// A slider drag in progress.
struct Drag {
    key: (usize, &'static str),
    base: Arc<Project>,
}

/// What a press on the player grabbed.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Handle {
    /// The whole mask, with the press this far from its centre.
    Move([f32; 2]),
    Width,
    Height,
    Corner,
    Rotate,
    Feather,
}

/// A handle drag in progress on the player.
struct HandleDrag {
    handle: Handle,
    index: usize,
    mask: Mask,
    base: Arc<Project>,
}

/// The tab's own state, one field on the inspector.
#[derive(Default)]
pub(crate) struct MasksPanel {
    sliders: HashMap<(usize, &'static str), ParamSlider>,
    key_picker: Option<(Entity<ColorPicker>, Subscription)>,
    drag: Option<Drag>,
    handle: Option<HandleDrag>,
    /// The mask whose handles are on the player.
    active: usize,
    /// "Show matte" in Remove background: the preview draws the clip's alpha.
    matte_view: bool,
    /// The eyedropper is armed: the next press on the player picks the key.
    picking: bool,
    /// Where the overlay was drawn, for turning presses into its pixels.
    overlay: Rc<Cell<Bounds<Pixels>>>,
    shown: bool,
    was_shown: bool,
}

impl MasksPanel {
    pub(super) fn begin_frame(&mut self) {
        self.was_shown = std::mem::take(&mut self.shown);
    }
}

/// One numeric control as the panel shows it: `scale` turns the document's
/// value into the number on screen.
struct UiParam {
    id: &'static str,
    label: &'static str,
    scale: f32,
    min: f32,
    max: f32,
    unit: &'static str,
}

const fn p(
    id: &'static str,
    label: &'static str,
    scale: f32,
    min: f32,
    max: f32,
    unit: &'static str,
) -> UiParam {
    UiParam {
        id,
        label,
        scale,
        min,
        max,
        unit,
    }
}

const MASK_UI: [UiParam; 7] = [
    p("x", "Position X", 100.0, -100.0, 100.0, "%"),
    p("y", "Position Y", 100.0, -100.0, 100.0, "%"),
    p("width", "Width", 100.0, 1.0, 200.0, "%"),
    p("height", "Height", 100.0, 1.0, 200.0, "%"),
    p("rotation", "Rotate", 1.0, -180.0, 180.0, "°"),
    p("feather", "Feather", 100.0, 0.0, 100.0, ""),
    p("roundness", "Round corners", 100.0, 0.0, 100.0, ""),
];

const KEY_UI: [UiParam; 4] = [
    p("tolerance", "Intensity", 100.0, 0.0, 100.0, ""),
    p("softness", "Softness", 100.0, 0.0, 100.0, ""),
    p("spill", "Spill removal", 100.0, 0.0, 100.0, ""),
    p("shrink", "Edge shrink", 100.0, 0.0, 100.0, ""),
];

/// Which of a mask's values mean anything for its shape.
fn shows(shape: &MaskShape, param: &str) -> bool {
    match param {
        "width" => !matches!(shape, MaskShape::Linear | MaskShape::Mirror),
        "height" => !matches!(shape, MaskShape::Linear),
        "roundness" => matches!(shape, MaskShape::Rectangle),
        _ => true,
    }
}

fn key_value(key: &ChromaKey, param: &str) -> f32 {
    match param {
        "tolerance" => key.tolerance,
        "softness" => key.softness,
        "spill" => key.spill,
        _ => key.shrink,
    }
}

fn set_key_value(key: &mut ChromaKey, param: &str, value: f32) {
    let value = value.clamp(0.0, 1.0);
    match param {
        "tolerance" => key.tolerance = value,
        "softness" => key.softness = value,
        "spill" => key.spill = value,
        _ => key.shrink = value,
    }
}

// --- the clip's frame on the player ----------------------------------------------------

/// Where a clip is drawn on the player, as the map from its mask space
/// (`render::matte`: short-side units, y up, from its centre) to overlay
/// pixels.
#[derive(Debug, Clone, Copy)]
struct ClipFrame {
    /// The unit quad to overlay pixels: `px = a · (x, y) + b`.
    a: [f32; 4],
    b: [f32; 2],
    /// The quad's sides in overlay pixels, and the shorter one.
    w: f32,
    h: f32,
    s: f32,
}

impl ClipFrame {
    fn of(project: &Project, segment: &Segment, time: Micros, size: (f32, f32)) -> Option<Self> {
        let pool = &project.materials;
        let source = if let Some(video) = pool.video(&segment.material_id) {
            if video.rotation.rem_euclid(180) == 90 {
                (video.height, video.width)
            } else {
                (video.width, video.height)
            }
        } else {
            let image = pool.image(&segment.material_id)?;
            (image.width, image.height)
        };
        // Placed the way the compositor places it: a followed clip by its
        // track, a stabilised one through its crop window, then keyframes and
        // keyframe-free animation.
        let followed = chukcut_engine::modules::tracking::follow::resolve(project, segment, time);
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
        let m = layout::place_quad(canvas, source, &transform, resolved.crop)?.mvp;
        let (dw, dh) = size;
        let a = [
            m[0] * dw * 0.5,
            m[4] * dw * 0.5,
            -m[1] * dh * 0.5,
            -m[5] * dh * 0.5,
        ];
        let b = [(m[12] + 1.0) * 0.5 * dw, (1.0 - m[13]) * 0.5 * dh];
        let w = a[0].hypot(a[2]);
        let h = a[1].hypot(a[3]);
        let s = w.min(h);
        (s > 1.0).then_some(Self { a, b, w, h, s })
    }

    /// Mask space to overlay pixels.
    fn pixel_of(&self, p: [f32; 2]) -> [f32; 2] {
        let x = p[0] * self.s / self.w;
        let y = p[1] * self.s / self.h;
        [
            self.a[0] * x + self.a[1] * y + self.b[0],
            self.a[2] * x + self.a[3] * y + self.b[1],
        ]
    }

    /// Overlay pixels to mask space.
    fn point_at(&self, px: [f32; 2]) -> [f32; 2] {
        let det = self.a[0] * self.a[3] - self.a[1] * self.a[2];
        if det.abs() < 1e-6 {
            return [0.0, 0.0];
        }
        let (dx, dy) = (px[0] - self.b[0], px[1] - self.b[1]);
        let x = (self.a[3] * dx - self.a[1] * dy) / det;
        let y = (-self.a[2] * dx + self.a[0] * dy) / det;
        [x * self.w / self.s, y * self.h / self.s]
    }

    /// A mask's centre in mask space.
    fn centre(&self, pose: &MaskGeometry) -> [f32; 2] {
        [pose.x * self.w / self.s, pose.y * self.h / self.s]
    }
}

/// A mask's values at the playhead, the ones the handles need.
#[derive(Debug, Clone, Copy)]
struct MaskGeometry {
    x: f32,
    y: f32,
    half: [f32; 2],
    rotation: f32,
    feather: f32,
    roundness: f32,
}

impl MaskGeometry {
    fn of(mask: &Mask, source_time: Micros) -> Self {
        let pose = mask.pose_at(source_time);
        Self {
            x: pose.x,
            y: pose.y,
            half: [pose.width * 0.5, pose.height * 0.5],
            rotation: pose.rotation,
            feather: pose.feather,
            roundness: pose.roundness,
        }
    }

    /// The mask's own frame to mask space: turn clockwise, then move.
    fn place(&self, frame: &ClipFrame, q: [f32; 2]) -> [f32; 2] {
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let c = frame.centre(self);
        [
            c[0] + q[0] * cos + q[1] * sin,
            c[1] - q[0] * sin + q[1] * cos,
        ]
    }

    /// Mask space into the mask's own frame.
    fn local(&self, frame: &ClipFrame, p: [f32; 2]) -> [f32; 2] {
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let c = frame.centre(self);
        let d = [p[0] - c[0], p[1] - c[1]];
        [d[0] * cos - d[1] * sin, d[0] * sin + d[1] * cos]
    }
}

/// The outline of a shape in its own frame, as polylines (closed when the
/// flag says so).
fn outline(shape: &MaskShape, g: &MaskGeometry) -> Vec<(Vec<[f32; 2]>, bool)> {
    let [a, b] = g.half;
    match shape {
        MaskShape::Linear => vec![(vec![[-3.0, 0.0], [3.0, 0.0]], false)],
        MaskShape::Mirror => vec![
            (vec![[-3.0, b], [3.0, b]], false),
            (vec![[-3.0, -b], [3.0, -b]], false),
        ],
        MaskShape::Rectangle => {
            let r = g.roundness.clamp(0.0, 1.0) * a.min(b);
            let mut points = Vec::new();
            for (cx, cy, start) in [
                (a - r, b - r, 0.0f32),
                (-a + r, b - r, 90.0),
                (-a + r, -b + r, 180.0),
                (a - r, -b + r, 270.0),
            ] {
                for k in 0..=8 {
                    let t = (start + k as f32 * 90.0 / 8.0).to_radians();
                    points.push([cx + r * t.cos(), cy + r * t.sin()]);
                }
            }
            vec![(points, true)]
        }
        MaskShape::Star => vec![(
            star_polygon()
                .iter()
                .map(|p| [p[0] * a, p[1] * b])
                .collect(),
            true,
        )],
        MaskShape::Heart => vec![(
            heart_polygon()
                .iter()
                .map(|p| [p[0] * a, p[1] * b])
                .collect(),
            true,
        )],
        _ => vec![(
            (0..64)
                .map(|k| {
                    let t = k as f32 / 64.0 * std::f32::consts::TAU;
                    [a * t.cos(), b * t.sin()]
                })
                .collect(),
            true,
        )],
    }
}

/// The handles a shape has, in its own frame.
fn handles(shape: &MaskShape, g: &MaskGeometry) -> Vec<(Handle, [f32; 2])> {
    let [a, b] = g.half;
    let edge = if matches!(shape, MaskShape::Linear) {
        0.0
    } else {
        b
    };
    let mut out = vec![(Handle::Move([0.0, 0.0]), [0.0, 0.0])];
    if shows(shape, "width") {
        out.push((Handle::Width, [a, 0.0]));
        out.push((Handle::Corner, [a, b]));
    }
    if shows(shape, "height") {
        out.push((Handle::Height, [0.0, b]));
    }
    out.push((Handle::Rotate, [0.0, edge + ROTATE_GAP]));
    out.push((
        Handle::Feather,
        [0.0, -(edge + FEATHER_GAP + g.feather * FEATHER_SPAN)],
    ));
    out
}

impl Editor {
    fn masks_material(&self, segment: &Segment) -> CompositingMaterial {
        self.project
            .materials
            .compositing_of(segment)
            .cloned()
            .unwrap_or_default()
    }

    /// The Video tab's open sub-tab, when the Video tab is the one shown.
    /// No tab chosen yet means the first, which is Video for a picture.
    fn video_sub_tab(&self) -> Option<&'static str> {
        if self.inspector.tab.unwrap_or("Video") != "Video" {
            return None;
        }
        self.inspector.sub_tab.get("Video").copied()
    }

    fn source_time_of(&self, project: &Project, segment: &Segment) -> Micros {
        comp_edit::source_time_in(project, segment, self.clock.position())
    }

    /// Show an edit built against `base` on a copy, without writing it.
    fn show_minted(
        &mut self,
        base: &Arc<Project>,
        minted: Result<Minted, String>,
        cx: &mut Context<Self>,
    ) {
        let shown = minted.and_then(|(material, command)| {
            let mut copy = (**base).clone();
            if let Some(material) = material {
                copy.materials.compositing.push(material);
            }
            command.apply(&mut copy)?;
            Ok(copy)
        });
        if let Ok(project) = shown {
            self.project = Arc::new(project);
            self.generation += 1;
            self.audio.set_project(Arc::clone(&self.project));
            cx.notify();
        }
    }

    fn after_command<T>(&mut self, result: Result<T, String>, cx: &mut Context<Self>) {
        self.refresh(cx);
        self.report(result.map(|_| ()), cx);
    }

    /// The project the preview should draw: the document, or — while "Show
    /// matte" is on — a copy with the selected clip's matte view switched on.
    /// The flag is runtime-only, so it never reaches the file or an export.
    pub(crate) fn preview_project(&self) -> Arc<Project> {
        let matte =
            self.inspector.masks.matte_view && self.video_sub_tab() == Some("Remove background");
        let Some(id) = self.selected.as_ref().filter(|_| matte) else {
            return Arc::clone(&self.project);
        };
        let Some(material_id) = self
            .project
            .segment(id)
            .and_then(|(_, s)| self.project.materials.compositing_of(s))
            .map(|m| m.id.clone())
        else {
            return Arc::clone(&self.project);
        };
        let mut copy = (*self.project).clone();
        if let Some(m) = copy
            .materials
            .compositing
            .iter_mut()
            .find(|m| m.id == material_id)
        {
            m.view_matte = true;
        }
        Arc::new(copy)
    }

    // --- Video › Mask ---------------------------------------------------------------

    pub(super) fn mask_tab(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let material = self.masks_material(segment);
        let tiles = MaskShape::all().into_iter().map(|shape| {
            let label = shape.label().to_string();
            let glyph = shape_glyph(&shape);
            let id = segment.id.clone();
            div()
                .id(SharedString::from(format!("mask-add-{}", shape.name())))
                .w(px(64.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .group("mask-tile")
                .on_click(cx.listener(move |this, _, _, cx| {
                    let result =
                        compositing::compositing_add_mask(&this.state, id.clone(), shape.clone());
                    if let Ok((response, _)) = &result {
                        let count = response
                            .project
                            .segment(&id)
                            .and_then(|(_, s)| response.project.materials.compositing_of(s))
                            .map_or(0, |m| m.masks.len());
                        this.inspector.masks.active = count.saturating_sub(1);
                    }
                    this.after_command(result, cx);
                }))
                .child(
                    div()
                        .size(px(56.0))
                        .rounded(px(R_SM))
                        .bg(rgb(WELL))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .hover(|s| s.border_color(rgb(ACCENT)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(glyph, 24.0, TEXT_DIM)),
                )
                .child(
                    div()
                        .text_size(px(TEXT_CAPTION))
                        .text_color(rgb(TEXT_DIM))
                        .child(label),
                )
        });
        let add = div()
            .px(px(PAD))
            .pt(px(PAD))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(group_label("Add mask"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(8.0))
                    .children(tiles),
            );

        let mut body = div().flex().flex_col().child(add);
        if material.masks.is_empty() {
            body = body.child(
                div().p(px(PAD)).child(
                    EmptyState::new("mask-empty", IconName::Frame, "No masks on this clip")
                        .hint("Pick a shape above, then drag its handles on the player."),
                ),
            );
            return body.into_any_element();
        }
        let active = self.inspector.masks.active.min(material.masks.len() - 1);
        self.inspector.masks.active = active;
        let source = self.source_time_of(&self.project.clone(), segment);
        for (index, mask) in material.masks.iter().enumerate() {
            body = body.child(self.mask_section(
                segment,
                index,
                mask,
                index == active,
                material.masks.len(),
                source,
                window,
                cx,
            ));
        }
        body.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn mask_section(
        &mut self,
        segment: &Segment,
        index: usize,
        mask: &Mask,
        active: bool,
        count: usize,
        source: Micros,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let segment_id = segment.id.clone();
        let mask_id = mask.id.clone();
        let enabled = mask.enabled;
        let options = |shape: Option<MaskShape>,
                       op: Option<MaskOp>,
                       invert: Option<bool>,
                       enabled: Option<bool>| {
            let (s, m) = (segment_id.clone(), mask_id.clone());
            move |this: &mut Editor,
                  _: &gpui::ClickEvent,
                  _: &mut Window,
                  cx: &mut Context<Editor>| {
                let result = compositing::compositing_set_mask_options(
                    &this.state,
                    s.clone(),
                    m.clone(),
                    shape.clone(),
                    op.clone(),
                    invert,
                    enabled,
                );
                this.after_command(result, cx);
            }
        };

        // The state is in the id, so an open tooltip does not keep the old
        // words after a click (see the effect eye in `effects.rs`).
        let eye = IconButton::new(
            SharedString::from(format!("mask-eye-{index}-{enabled}")),
            if enabled {
                IconName::Eye
            } else {
                IconName::EyeOff
            },
        )
        .small()
        .tint(if enabled { TEXT_DIM } else { TEXT_MUTED })
        .tooltip(if enabled { "Turn off" } else { "Turn on" })
        .on_click(cx.listener(options(None, None, None, Some(!enabled))));
        let mover = |delta: isize| {
            let (s, m) = (segment_id.clone(), mask_id.clone());
            cx.listener(move |this, _, _, cx| {
                let to = (index as isize + delta).max(0) as usize;
                let result =
                    compositing::compositing_move_mask(&this.state, s.clone(), m.clone(), to);
                this.inspector.masks.active = to;
                this.after_command(result, cx);
            })
        };
        let (s, m) = (segment_id.clone(), mask_id.clone());
        let delete = IconButton::new(
            SharedString::from(format!("mask-delete-{index}")),
            IconName::Trash,
        )
        .small()
        .tint(TEXT_DIM)
        .tooltip("Remove mask")
        .on_click(cx.listener(move |this, _, _, cx| {
            let result = compositing::compositing_remove_mask(&this.state, s.clone(), m.clone());
            this.after_command(result, cx);
        }));
        let title = format!("{} {}", mask.shape.label(), index + 1);
        let header = div()
            .id(SharedString::from(format!("mask-header-{index}")))
            .h(px(PANEL_HEADER_H - 4.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.inspector.masks.active = index;
                cx.notify();
            }))
            .child(eye)
            .child(icon(
                shape_glyph(&mask.shape),
                16.0,
                if active { ACCENT } else { TEXT_DIM },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(TEXT_BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(if active { TEXT } else { TEXT_DIM }))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(title),
            )
            .child(icon_button(
                SharedString::from(format!("mask-up-{index}")),
                icons::UP,
                TEXT_DIM,
                index > 0,
                mover(-1),
            ))
            .child(icon_button(
                SharedString::from(format!("mask-down-{index}")),
                icons::DOWN,
                TEXT_DIM,
                index + 1 < count,
                mover(1),
            ))
            .child(delete);

        // Combine and invert, as pills.
        let pill = |id: String, label: &'static str, on: bool| {
            div()
                .id(SharedString::from(id))
                .px(px(8.0))
                .h(px(CONTROL_H - 2.0))
                .flex()
                .items_center()
                .rounded(px(R_SM))
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(if on { ACCENT } else { TEXT_DIM }))
                .border_1()
                .border_color(rgb(if on { ACCENT } else { BORDER }))
                .bg(if on { accent_soft() } else { rgb(WELL).into() })
                .cursor_pointer()
                .hover(|style| style.text_color(rgb(TEXT)))
                .child(label)
        };
        let mut pills = Vec::new();
        for op in MaskOp::all() {
            let on = mask.op == op;
            let label = match op {
                MaskOp::Add => "Add",
                MaskOp::Subtract => "Subtract",
                _ => "Intersect",
            };
            pills.push(
                pill(format!("mask-op-{index}-{}", op.name()), label, on)
                    .on_click(cx.listener(options(None, Some(op), None, None)))
                    .into_any_element(),
            );
        }
        pills.push(div().w(px(8.0)).into_any_element());
        pills.push(
            pill(format!("mask-invert-{index}"), "Invert", mask.invert)
                .on_click(cx.listener(options(None, None, Some(!mask.invert), None)))
                .into_any_element(),
        );

        let mut rows = vec![div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(4.0))
            .children(pills)
            .into_any_element()];
        if active {
            for param in MASK_UI.iter().filter(|p| shows(&mask.shape, p.id)) {
                let value = mask.value_at(param.id, source);
                let keys = mask.keyframes.get(param.id);
                rows.push(self.compositing_row(index, param, value, keys, source, window, cx));
            }
        } else {
            rows.push(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Click the name to edit this mask and show its handles.")
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .px(px(PAD))
            .pb(px(PAD))
            .border_b_1()
            .border_color(rgb(HAIRLINE))
            .when(active, |this| this.bg(rgb(PANEL_RAISED)))
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .when(!enabled, |this| this.opacity(0.5))
                    .children(rows),
            )
            .into_any_element()
    }

    /// One slider row: label, slider, value, reset and — for masks — the
    /// keyframe diamond.
    #[allow(clippy::too_many_arguments)]
    fn compositing_row(
        &mut self,
        index: usize,
        param: &'static UiParam,
        value: f32,
        keys: Option<&Vec<chukcut_engine::modules::project::Keyframe>>,
        source: Micros,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = value * param.scale;
        let slider = self.compositing_slider(index, param, shown, window, cx);
        let label = div()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(TEXT_DIM))
            .child(param.label);
        let id = param.id;
        let rest = match id {
            "width" | "height" => 50.0,
            "tolerance" => 30.0,
            "softness" => 10.0,
            "spill" => 50.0,
            _ => 0.0,
        };
        let reset = icon_button(
            SharedString::from(format!("comp-reset-{index}-{id}")),
            icons::RESET,
            TEXT_DIM,
            true,
            cx.listener(move |this, _, _, cx| this.drag_compositing((index, id), rest, true, cx)),
        );
        let mut controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .child(div().flex_1().px(px(4.0)).child(Slider::new(&slider)))
            .child(
                div()
                    .w(px(56.0))
                    .text_right()
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT))
                    .child(format!("{shown:.0}{}", param.unit)),
            )
            .child(reset);
        if index != KEY {
            let tolerance = (500_000.0 / self.project.fps.max(1.0)) as Micros;
            let at_key =
                keys.is_some_and(|k| k.iter().any(|k| (k.time - source).abs() <= tolerance));
            let animated = keys.is_some_and(|k| !k.is_empty());
            controls = controls.child(icon_button(
                SharedString::from(format!("comp-kf-{index}-{id}")),
                if at_key {
                    icons::DIAMOND_FILLED
                } else {
                    icons::DIAMOND
                },
                if at_key || animated { ACCENT } else { TEXT_DIM },
                true,
                cx.listener(move |this, _, _, cx| this.toggle_mask_keyframe(index, id, cx)),
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(label)
            .child(controls)
            .into_any_element()
    }

    fn toggle_mask_keyframe(&mut self, index: usize, param: &'static str, cx: &mut Context<Self>) {
        let Some(segment) = self.selected_segment().map(|(_, s)| s.clone()) else {
            return;
        };
        let Some(mask) = self.masks_material(&segment).masks.get(index).cloned() else {
            return;
        };
        let result = compositing::compositing_toggle_mask_keyframe(
            &self.state,
            segment.id.clone(),
            mask.id,
            param.to_string(),
            self.clock.position(),
        );
        self.after_command(result, cx);
    }

    /// The slider of one control, made on first use and kept in step with
    /// the document unless it is being dragged.
    fn compositing_slider(
        &mut self,
        index: usize,
        param: &'static UiParam,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SliderState> {
        let key = (index, param.id);
        if let std::collections::hash_map::Entry::Vacant(slot) =
            self.inspector.masks.sliders.entry(key)
        {
            let state = cx.new(|_| {
                SliderState::new()
                    .min(param.min)
                    .max(param.max)
                    .step(1.0)
                    .default_value(value.clamp(param.min, param.max))
            });
            let subscription = cx.subscribe_in(
                &state,
                window,
                move |this: &mut Editor, _, event: &SliderEvent, _, cx| match event {
                    SliderEvent::Change(v) => this.drag_compositing(key, v.end(), false, cx),
                    SliderEvent::Release(v) => this.drag_compositing(key, v.end(), true, cx),
                },
            );
            slot.insert(ParamSlider {
                state,
                _subscription: subscription,
            });
        }
        let state = self.inspector.masks.sliders[&key].state.clone();
        let dragging = self
            .inspector
            .masks
            .drag
            .as_ref()
            .is_some_and(|d| d.key == key);
        let clamped = value.clamp(param.min, param.max);
        if !dragging && (state.read(cx).value().end() - clamped).abs() > 1e-3 {
            state.update(cx, |s, cx| s.set_value(clamped, window, cx));
        }
        state
    }

    /// A slider moved (`commit` false) or was let go (`commit` true), in the
    /// panel's units: shown on a copy while it moves, written once when it
    /// settles.
    fn drag_compositing(
        &mut self,
        key: (usize, &'static str),
        shown: f32,
        commit: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let table: &[UiParam] = if key.0 == KEY { &KEY_UI } else { &MASK_UI };
        let Some(param) = table.iter().find(|p| p.id == key.1) else {
            return;
        };
        let value = shown / param.scale;
        let base = match &self.inspector.masks.drag {
            Some(drag) if drag.key == key => Arc::clone(&drag.base),
            _ => {
                let base = Arc::clone(&self.project);
                self.inspector.masks.drag = Some(Drag {
                    key,
                    base: Arc::clone(&base),
                });
                base
            }
        };
        let Some((_, segment)) = base.segment(&segment_id) else {
            return;
        };
        let material = base
            .materials
            .compositing_of(segment)
            .cloned()
            .unwrap_or_default();
        let playhead = self.clock.position();
        let source = comp_edit::source_time_in(&base, segment, playhead);

        if key.0 == KEY {
            let Some(mut chroma) = material.key.clone() else {
                return;
            };
            set_key_value(&mut chroma, key.1, value);
            if !commit {
                let minted = comp_edit::set_key_command(&base, &segment_id, Some(chroma));
                self.show_minted(&base, minted, cx);
                return;
            }
            self.inspector.masks.drag = None;
            self.project = base;
            self.generation += 1;
            let result = compositing::compositing_set_key(&self.state, segment_id, Some(chroma));
            self.after_command(result, cx);
            return;
        }

        let Some(mask) = material.masks.get(key.0) else {
            return;
        };
        let mask_id = mask.id.clone();
        if !commit {
            let minted = comp_edit::set_mask_value_command(
                &base,
                &segment_id,
                &mask_id,
                key.1,
                value,
                Some(source),
            );
            self.show_minted(&base, minted, cx);
            return;
        }
        self.inspector.masks.drag = None;
        self.project = base;
        self.generation += 1;
        let result = compositing::compositing_set_mask_value(
            &self.state,
            segment_id,
            mask_id,
            key.1.to_string(),
            value,
            Some(playhead),
        );
        // Letting go where the slider started changes nothing; that is not
        // an error worth a status line.
        let result = match result {
            Err(e) if e == "nothing changed" => Ok(()),
            other => other.map(|_| ()),
        };
        self.after_command(result, cx);
    }

    // --- Video › Remove background -------------------------------------------------------

    pub(super) fn remove_background_tab(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.inspector.masks.was_shown {
            if let Some((picker, _)) = &self.inspector.masks.key_picker {
                picker.update(cx, |picker, _| picker.open = false);
            }
        }
        self.inspector.masks.shown = true;
        let material = self.masks_material(segment);
        let key = material.key.clone();
        let on = key.as_ref().is_some_and(|k| k.enabled);

        let mut rows = Vec::new();
        if let Some(key) = &key {
            let picker = self.key_colour_picker(window, cx);
            let colour = [key.color[0], key.color[1], key.color[2], 1.0];
            picker.update(cx, |picker, _| picker.sync(colour));
            let picking = self.inspector.masks.picking;
            let dropper = IconButton::new(
                SharedString::from(format!("key-eyedropper-{picking}")),
                ui::Glyph(glyphs::EYEDROPPER),
            )
            .small()
            .tint(if picking { ACCENT } else { TEXT_DIM })
            .tooltip(if picking {
                "Click the colour on the player"
            } else {
                "Pick the colour on the player"
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.inspector.masks.picking = !this.inspector.masks.picking;
                cx.notify();
            }));
            rows.push(label_row(
                "Colour",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(dropper)
                    .child(ui::color_button("key-colour", &picker, None, true, cx)),
            ));
            for param in KEY_UI.iter() {
                let value = key_value(key, param.id);
                rows.push(self.compositing_row(KEY, param, value, None, 0, window, cx));
            }
            let matte = self.inspector.masks.matte_view;
            rows.push(label_row(
                "Show matte",
                Switch::new("key-matte")
                    .checked(matte)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.inspector.masks.matte_view = *checked;
                        // A new picture for the same document.
                        this.generation += 1;
                        cx.notify();
                    })),
            ));
        } else {
            rows.push(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Tick the box, then pick the background colour on the player.")
                    .into_any_element(),
            );
        }
        let id = segment.id.clone();
        let reset_id = segment.id.clone();
        let chroma = Section {
            checkbox: Some(on),
            on_check: Some(Box::new(move |this: &mut Editor, checked, cx| {
                let current = this
                    .project
                    .segment(&id)
                    .and_then(|(_, s)| this.project.materials.compositing_of(s))
                    .and_then(|m| m.key.clone());
                let next = match (current, checked) {
                    (Some(mut key), on) => {
                        key.enabled = on;
                        Some(key)
                    }
                    // A fresh key starts on green, the usual screen, with the
                    // eyedropper armed to pick the real one.
                    (None, true) => {
                        this.inspector.masks.picking = true;
                        Some(ChromaKey::new([0.0, 1.0, 0.0]))
                    }
                    (None, false) => None,
                };
                if let Some(next) = next {
                    let result =
                        compositing::compositing_set_key(&this.state, id.clone(), Some(next));
                    this.after_command(result, cx);
                }
            })),
            on_reset: key.is_some().then(|| {
                Box::new(move |this: &mut Editor, cx: &mut Context<Editor>| {
                    this.inspector.masks.matte_view = false;
                    let result =
                        compositing::compositing_set_key(&this.state, reset_id.clone(), None);
                    this.after_command(result, cx);
                }) as Box<dyn Fn(&mut Editor, &mut Context<Editor>)>
            }),
            ..Section::new("Chroma key")
        }
        .render(self.collapsed("Chroma key"), rows, cx);

        div()
            .flex()
            .flex_col()
            .child(chroma)
            .child(
                Section::missing("Auto remove", "Needs the ML worker (backlog 3)").render(
                    true,
                    Vec::new(),
                    cx,
                ),
            )
            .into_any_element()
    }

    /// The key colour's picker, made on first use.
    fn key_colour_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ColorPicker> {
        if let Some((picker, _)) = &self.inspector.masks.key_picker {
            return picker.clone();
        }
        let picker = cx.new(|cx| ColorPicker::new(window, cx));
        let subscription = cx.subscribe(&picker, move |this: &mut Editor, _, event, cx| {
            let (colour, commit) = match *event {
                ColorEvent::Preview(c) => (c, false),
                ColorEvent::Commit(c) => (c, true),
            };
            this.set_key_colour([colour[0], colour[1], colour[2]], commit, cx);
        });
        self.inspector.masks.key_picker = Some((picker.clone(), subscription));
        picker
    }

    /// Set the key colour (encoded sRGB): previewed, or written as one step.
    fn set_key_colour(&mut self, colour: [f32; 3], commit: bool, cx: &mut Context<Self>) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let key_slot = (KEY, "colour");
        let base = match &self.inspector.masks.drag {
            Some(drag) if drag.key == key_slot => Arc::clone(&drag.base),
            _ => {
                let base = Arc::clone(&self.project);
                if !commit {
                    self.inspector.masks.drag = Some(Drag {
                        key: key_slot,
                        base: Arc::clone(&base),
                    });
                }
                base
            }
        };
        let current = base
            .segment(&segment_id)
            .and_then(|(_, s)| base.materials.compositing_of(s))
            .and_then(|m| m.key.clone());
        let mut key = current.unwrap_or_else(|| ChromaKey::new(colour));
        key.color = colour;
        if !commit {
            let minted = comp_edit::set_key_command(&base, &segment_id, Some(key));
            self.show_minted(&base, minted, cx);
            return;
        }
        self.inspector.masks.drag = None;
        self.project = base;
        self.generation += 1;
        let result = compositing::compositing_set_key(&self.state, segment_id, Some(key));
        let result = match result {
            Err(e) if e == "nothing changed" => Ok(()),
            other => other.map(|_| ()),
        };
        self.after_command(result, cx);
    }

    // --- Video › Basic › Blend ------------------------------------------------------------

    /// The blend mode menu of the Blend section.
    pub(super) fn blend_mode_control(
        &self,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.masks_material(segment).blend;
        let editor = cx.entity().downgrade();
        let id = segment.id.clone();
        Button::new("blend-mode")
            .label(current.label().to_string())
            .outline()
            .xsmall()
            .w(px(160.0))
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                BlendMode::all()
                    .into_iter()
                    .fold(menu.min_w(px(180.0)), |menu, mode| {
                        let editor = editor.clone();
                        let id = id.clone();
                        let checked = mode == current;
                        menu.item(
                            PopupMenuItem::new(mode.label().to_string())
                                .checked(checked)
                                .on_click(move |_, _, cx| {
                                    let mode = mode.clone();
                                    let id = id.clone();
                                    let _ = editor.update(cx, |this, cx| {
                                        let result = compositing::compositing_set_blend(
                                            &this.state,
                                            id,
                                            mode,
                                        );
                                        let result = match result {
                                            Err(e) if e == "nothing changed" => Ok(()),
                                            other => other.map(|_| ()),
                                        };
                                        this.after_command(result, cx);
                                    });
                                }),
                        )
                    })
            })
            .into_any_element()
    }

    /// The Blend section's reset: back to normal, when it is not already.
    pub(super) fn reset_blend_mode(&mut self, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        if self.masks_material(segment).blend.is_normal() {
            return;
        }
        let id = segment.id.clone();
        let result = compositing::compositing_reset(&self.state, id, ResetPart::Blend);
        self.after_command(result, cx);
    }

    // --- the player -------------------------------------------------------------------------

    /// The selected clip's active mask, its outline and handles, over the
    /// player's picture (`dw` × `dh`), while the Mask sub-tab is open; or
    /// the eyedropper's catch-all while it is armed.
    pub(in crate::editor) fn mask_overlay(
        &self,
        dw: f32,
        dh: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if dw < 4.0 || dh < 4.0 {
            return None;
        }
        let bounds = Rc::clone(&self.inspector.masks.overlay);
        if self.inspector.masks.picking && self.selected.is_some() {
            let bounds_paint = Rc::clone(&bounds);
            return Some(
                div()
                    .id("key-eyedropper-overlay")
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
                            this.pick_key_colour(event.position, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        canvas(move |b, _, _| bounds_paint.set(b), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .into_any_element(),
            );
        }
        if self.video_sub_tab() != Some("Mask") {
            return None;
        }
        let (_, segment) = self.selected_segment()?;
        let material = self.project.materials.compositing_of(segment)?;
        let index = self
            .inspector
            .masks
            .active
            .min(material.masks.len().checked_sub(1)?);
        let mask = material.masks.get(index)?.clone();
        let time = self.clock.position();
        let frame = ClipFrame::of(&self.project, segment, time, (dw, dh))?;
        let source = comp_edit::source_time_in(&self.project, segment, time);
        let geometry = MaskGeometry::of(&mask, source);
        let lines: Vec<(Vec<[f32; 2]>, bool)> = outline(&mask.shape, &geometry)
            .into_iter()
            .map(|(points, closed)| {
                (
                    points
                        .into_iter()
                        .map(|q| frame.pixel_of(geometry.place(&frame, q)))
                        .collect(),
                    closed,
                )
            })
            .collect();
        let dots: Vec<(Handle, [f32; 2])> = handles(&mask.shape, &geometry)
            .into_iter()
            .map(|(h, q)| (h, frame.pixel_of(geometry.place(&frame, q))))
            .collect();
        let rotate_from = frame.pixel_of(geometry.place(
            &frame,
            [
                0.0,
                if matches!(mask.shape, MaskShape::Linear) {
                    0.0
                } else {
                    geometry.half[1]
                },
            ],
        ));
        let entity = cx.entity().downgrade();

        // Wider than the picture by a margin, so a rotate or feather handle
        // that sits past its edge can still be grabbed. The canvas inside is
        // the picture exactly, and its bounds are what presses are measured
        // from.
        let overlay = div()
            .id("mask-overlay")
            .absolute()
            .top(px(-HANDLE_MARGIN))
            .left(px(-HANDLE_MARGIN))
            .w(px(dw + 2.0 * HANDLE_MARGIN))
            .h(px(dh + 2.0 * HANDLE_MARGIN))
            .cursor_move()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.mask_press(event.position, (dw, dh), cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                canvas(
                    move |b, _, _| bounds.set(b),
                    move |b, _, window, _| {
                        let at = |p: [f32; 2]| point(b.origin.x + px(p[0]), b.origin.y + px(p[1]));
                        for (points, closed) in &lines {
                            let mut path = PathBuilder::stroke(px(1.5));
                            for (i, p) in points.iter().enumerate() {
                                if i == 0 {
                                    path.move_to(at(*p));
                                } else {
                                    path.line_to(at(*p));
                                }
                            }
                            if *closed {
                                if let Some(first) = points.first() {
                                    path.line_to(at(*first));
                                }
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, rgb(ACCENT));
                            }
                        }
                        // The rotate handle hangs off the top edge.
                        if let Some((_, top)) = dots.iter().find(|(h, _)| *h == Handle::Rotate) {
                            let mut stem = PathBuilder::stroke(px(1.0));
                            stem.move_to(at(rotate_from));
                            stem.line_to(at(*top));
                            if let Ok(path) = stem.build() {
                                window.paint_path(path, rgb(ACCENT));
                            }
                        }
                        for (handle, p) in &dots {
                            let centre = at(*p);
                            match handle {
                                Handle::Move(_) => {
                                    window.paint_quad(ring(centre, 7.0, rgb(ACCENT), 1.5));
                                    window.paint_quad(disc(centre, 2.0, rgb(ACCENT)));
                                }
                                Handle::Rotate => {
                                    window.paint_quad(disc(centre, 5.0, rgb(PANEL)));
                                    window.paint_quad(ring(centre, 5.0, rgb(ACCENT), 1.5));
                                }
                                Handle::Feather => {
                                    window.paint_quad(disc(centre, 4.5, rgb(ACCENT)));
                                }
                                _ => {
                                    window.paint_quad(disc(centre, 4.5, rgb(TEXT)));
                                    window.paint_quad(ring(centre, 4.5, rgb(ACCENT), 1.5));
                                }
                            }
                        }
                        // Registered on every paint, not only mid-drag: a
                        // press, its moves and its release can all arrive
                        // before the next frame (the curve editor's lesson).
                        let up = entity.clone();
                        let moving = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = moving.update(cx, |this, cx| {
                                    if this.inspector.masks.handle.is_some() {
                                        this.mask_drag(event.position, (dw, dh), false, cx)
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = up.update(cx, |this, cx| {
                                    if this.inspector.masks.handle.is_some() {
                                        this.mask_drag(event.position, (dw, dh), true, cx)
                                    }
                                });
                            }
                        });
                    },
                )
                .absolute()
                .top(px(HANDLE_MARGIN))
                .left(px(HANDLE_MARGIN))
                .w(px(dw))
                .h(px(dh)),
            );
        Some(overlay.into_any_element())
    }

    /// A window point in overlay pixels.
    fn overlay_point(&self, position: Point<Pixels>) -> [f32; 2] {
        let b = self.inspector.masks.overlay.get();
        [
            f32::from(position.x) - f32::from(b.origin.x),
            f32::from(position.y) - f32::from(b.origin.y),
        ]
    }

    /// A press on the player: grab the nearest handle, or the whole mask.
    fn mask_press(&mut self, position: Point<Pixels>, size: (f32, f32), cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let segment = segment.clone();
        let material = self.masks_material(&segment);
        let index = self.inspector.masks.active;
        let Some(mask) = material.masks.get(index).cloned() else {
            return;
        };
        let time = self.clock.position();
        let Some(frame) = ClipFrame::of(&self.project, &segment, time, size) else {
            return;
        };
        let source = comp_edit::source_time_in(&self.project, &segment, time);
        let geometry = MaskGeometry::of(&mask, source);
        let at = self.overlay_point(position);
        let grabbed = handles(&mask.shape, &geometry)
            .into_iter()
            .map(|(h, q)| {
                let p = frame.pixel_of(geometry.place(&frame, q));
                (h, (p[0] - at[0]).hypot(p[1] - at[1]))
            })
            .filter(|(_, d)| *d <= GRAB_PX)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h);
        let handle = match grabbed {
            Some(Handle::Move(_)) | None => {
                let p = frame.point_at(at);
                let c = frame.centre(&geometry);
                Handle::Move([p[0] - c[0], p[1] - c[1]])
            }
            Some(h) => h,
        };
        self.inspector.masks.handle = Some(HandleDrag {
            handle,
            index,
            mask,
            base: Arc::clone(&self.project),
        });
        cx.notify();
    }

    /// The pointer moved with a handle held, or let go of it (`commit`).
    fn mask_drag(
        &mut self,
        position: Point<Pixels>,
        size: (f32, f32),
        commit: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.inspector.masks.handle.as_ref() else {
            return;
        };
        let (handle, index, original, base) = (
            drag.handle,
            drag.index,
            drag.mask.clone(),
            Arc::clone(&drag.base),
        );
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let Some((_, segment)) = base.segment(&segment_id) else {
            return;
        };
        let time = self.clock.position();
        let Some(frame) = ClipFrame::of(&base, segment, time, size) else {
            return;
        };
        let source = comp_edit::source_time_in(&base, segment, time);
        let geometry = MaskGeometry::of(&original, source);
        let p = frame.point_at(self.overlay_point(position));
        let q = geometry.local(&frame, p);
        let mut mask = original.clone();
        let at = Some(source);
        match handle {
            Handle::Move(grab) => {
                let c = [p[0] - grab[0], p[1] - grab[1]];
                comp_edit::pose_mask(&mut mask, "x", c[0] * frame.s / frame.w, at);
                comp_edit::pose_mask(&mut mask, "y", c[1] * frame.s / frame.h, at);
            }
            Handle::Width => comp_edit::pose_mask(&mut mask, "width", 2.0 * q[0].abs(), at),
            Handle::Height => comp_edit::pose_mask(&mut mask, "height", 2.0 * q[1].abs(), at),
            Handle::Corner => {
                comp_edit::pose_mask(&mut mask, "width", 2.0 * q[0].abs(), at);
                comp_edit::pose_mask(&mut mask, "height", 2.0 * q[1].abs(), at);
            }
            Handle::Rotate => {
                let c = frame.centre(&geometry);
                let angle = (p[0] - c[0]).atan2(p[1] - c[1]).to_degrees();
                comp_edit::pose_mask(&mut mask, "rotation", angle, at);
            }
            Handle::Feather => {
                let edge = if matches!(mask.shape, MaskShape::Linear) {
                    0.0
                } else {
                    geometry.half[1]
                };
                let feather = ((-q[1]) - edge - FEATHER_GAP) / FEATHER_SPAN;
                comp_edit::pose_mask(&mut mask, "feather", feather.clamp(0.0, 1.0), at);
            }
        }
        let _ = index;
        if !commit {
            let minted = comp_edit::replace_mask_command(&base, &segment_id, mask, "Move mask");
            self.show_minted(&base, minted, cx);
            return;
        }
        self.inspector.masks.handle = None;
        self.project = base;
        self.generation += 1;
        if mask == original {
            cx.notify();
            return;
        }
        let result = compositing::compositing_set_mask(&self.state, segment_id, mask);
        self.after_command(result, cx);
    }

    /// The eyedropper: the footage's colour under `position` becomes the
    /// key colour, as one undo step.
    fn pick_key_colour(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let b = self.inspector.masks.overlay.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        if w < 1.0 || h < 1.0 {
            return;
        }
        let at = self.overlay_point(position);
        let point = [(at[0] / w).clamp(0.0, 1.0), (at[1] / h).clamp(0.0, 1.0)];
        self.inspector.masks.picking = false;
        self.status = Some("Picking the key colour\u{2026}".into());
        cx.notify();
        let state = Arc::clone(&self.state);
        let time = self.clock.position();
        cx.spawn(async move |this, cx| {
            let picked =
                compositing::compositing_pick_key_color(&state, segment_id.clone(), time, point)
                    .await;
            let _ = this.update(cx, |editor, cx| match picked {
                Ok(colour) => {
                    editor.status = None;
                    editor.set_key_colour(colour, true, cx);
                }
                Err(error) => editor.report(Err(error), cx),
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> ClipFrame {
        // A 200 x 100 quad turned a quarter clockwise around (150, 80).
        ClipFrame {
            a: [0.0, -100.0, 200.0, 0.0],
            b: [150.0, 80.0],
            w: 200.0,
            h: 100.0,
            s: 100.0,
        }
    }

    #[test]
    fn pixels_and_mask_space_round_trip() {
        let f = frame();
        for p in [[0.0, 0.0], [0.3, -0.2], [-0.9, 0.45]] {
            let back = f.point_at(f.pixel_of(p));
            assert!((back[0] - p[0]).abs() < 1e-5 && (back[1] - p[1]).abs() < 1e-5);
        }
    }

    #[test]
    fn a_mask_frame_turns_clockwise_and_back() {
        let f = ClipFrame {
            a: [100.0, 0.0, 0.0, -100.0],
            b: [50.0, 50.0],
            w: 100.0,
            h: 100.0,
            s: 100.0,
        };
        let g = MaskGeometry {
            x: 0.0,
            y: 0.0,
            half: [0.25, 0.25],
            rotation: 90.0,
            feather: 0.0,
            roundness: 0.0,
        };
        // Its own +x points down after a clockwise quarter turn.
        let p = g.place(&f, [1.0, 0.0]);
        assert!(p[0].abs() < 1e-5 && (p[1] + 1.0).abs() < 1e-5);
        let q = g.local(&f, p);
        assert!((q[0] - 1.0).abs() < 1e-5 && q[1].abs() < 1e-5);
    }

    #[test]
    fn every_shape_has_a_move_rotate_and_feather_handle() {
        let g = MaskGeometry {
            x: 0.0,
            y: 0.0,
            half: [0.25, 0.25],
            rotation: 0.0,
            feather: 0.5,
            roundness: 0.0,
        };
        for shape in MaskShape::all() {
            let hs: Vec<Handle> = handles(&shape, &g).into_iter().map(|(h, _)| h).collect();
            assert!(hs.contains(&Handle::Rotate) && hs.contains(&Handle::Feather));
            assert!(!outline(&shape, &g).is_empty());
        }
    }
}
