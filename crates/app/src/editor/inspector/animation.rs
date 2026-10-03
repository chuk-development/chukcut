//! The Animation tab: In, Out and Combo presets for any picture clip, the
//! text animator for a title, and the punch-in zoom.
//!
//! Laid out like CapCut's Animation tab: segmented sub-tabs, then a grid of
//! preset tiles with "None" first, then the timing of the chosen preset. Every
//! tile previews its own preset: at rest it shows a frozen moment of the
//! motion, and under the pointer it plays it on a loop. The tile card is an
//! SVG moved by GPUI's transform, driven by the very curves the compositor
//! uses (`motion::pose`), so a tile and the player cannot disagree.
//!
//! Every change is an engine command built by `motion::edit` and applied
//! through `timeline_apply`: one undo step each. A slider drag shows its value
//! on a copy of the document and writes once on release, the inspector's
//! usual contract (see `inspector/mod.rs`).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use chukcut_engine::modules::motion::{catalog, edit as motion_edit, pose, text as motion_text};
use chukcut_engine::modules::project::animation::{
    AnimationMaterial, AnimationPreset, AnimationSlot, ClipAnimation, Ease, PunchZoom,
    StaggerOrder, TextAnimator, TextPreset, TextSlot, TextUnit,
};
use gpui::{
    rgba, svg, AnyElement, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, Transformation,
};

use gpui::component::slider::Slider;

use super::controls::*;
use super::*;

const IN: &str = "In";
const OUT: &str = "Out";
const COMBO: &str = "Combo";
const ZOOM: &str = "Zoom";
const TEXT_TAB: &str = "Text";
/// The key the sub-tab is remembered under; the same string `clip.rs` uses
/// for the top tab, so the inspector's sub-tab map keeps one entry for it.
const OWNER: &str = "Animation";

/// A tile's box and the card inside it.
const TILE: f32 = 64.0;
const CARD: f32 = 36.0;
/// Pixels a normalized-canvas unit of offset moves the card in a tile.
const TILE_TRAVEL: f32 = 26.0;

/// A slider the tab owns. Each maps one document value.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Knob {
    Duration(AnimationSlot),
    Strength(AnimationSlot),
    TextDuration(TextSlot),
    TextOverlap(TextSlot),
    TextStrength(TextSlot),
    ZoomAmount,
    ZoomRamp,
    /// The auto-zoom amount: panel state, not a document value.
    AutoAmount,
}

impl Knob {
    /// `(min, max, step)` in panel units: seconds or percent.
    fn range(self) -> (f32, f32, f32) {
        match self {
            Knob::Duration(_) | Knob::TextDuration(_) => (0.1, 5.0, 0.1),
            Knob::Strength(_) | Knob::TextStrength(_) => (0.0, 200.0, 1.0),
            Knob::TextOverlap(_) => (0.0, 100.0, 1.0),
            Knob::ZoomAmount | Knob::AutoAmount => (100.0, 250.0, 1.0),
            Knob::ZoomRamp => (0.0, 3.0, 0.05),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Knob::Duration(AnimationSlot::Combo) => "Speed (one loop)",
            Knob::Duration(_) | Knob::TextDuration(_) => "Duration",
            Knob::Strength(_) | Knob::TextStrength(_) => "Strength",
            Knob::TextOverlap(_) => "Overlap",
            Knob::ZoomAmount => "Zoom",
            Knob::ZoomRamp => "Push-in time",
            Knob::AutoAmount => "Punched-in zoom",
        }
    }

    fn format(self, value: f32) -> String {
        match self {
            Knob::Duration(_) | Knob::TextDuration(_) | Knob::ZoomRamp => format!("{value:.1}s"),
            _ => format!("{value:.0}%"),
        }
    }
}

/// Which tile is under the pointer.
#[derive(Clone, Copy, PartialEq, Debug)]
enum TileKey {
    Clip(AnimationSlot, AnimationPreset),
    Text(TextPreset),
}

struct KnobWidget {
    slider: Entity<SliderState>,
    _subscription: Subscription,
}

/// The tab's memory between frames: one field on the inspector.
pub(crate) struct AnimationTab {
    hovered: Option<(TileKey, Instant)>,
    knobs: HashMap<Knob, KnobWidget>,
    /// A knob being dragged, and the document from before the drag.
    drag: Option<(Knob, Arc<Project>)>,
    /// Which text slot the Text sub-tab edits.
    text_slot: TextSlot,
    auto_amount: f32,
    /// The document from before a pivot drag on the player began.
    pivot_drag: Option<Arc<Project>>,
}

impl Default for AnimationTab {
    fn default() -> Self {
        Self {
            hovered: None,
            knobs: HashMap::new(),
            drag: None,
            text_slot: TextSlot::In,
            auto_amount: 110.0,
            pivot_drag: None,
        }
    }
}

fn slot_of(tab: &str) -> AnimationSlot {
    match tab {
        OUT => AnimationSlot::Out,
        COMBO => AnimationSlot::Combo,
        _ => AnimationSlot::In,
    }
}

fn slot_value(material: Option<&AnimationMaterial>, slot: AnimationSlot) -> Option<ClipAnimation> {
    let m = material?;
    match slot {
        AnimationSlot::In => m.intro,
        AnimationSlot::Out => m.outro,
        AnimationSlot::Combo => m.combo,
    }
}

fn text_value(material: Option<&AnimationMaterial>, slot: TextSlot) -> Option<TextAnimator> {
    let m = material?;
    match slot {
        TextSlot::In => m.text_in,
        TextSlot::Out => m.text_out,
    }
}

fn secs(micros: Micros) -> f32 {
    micros as f32 / 1_000_000.0
}

fn micros(secs: f32) -> Micros {
    (secs as f64 * 1_000_000.0).round() as Micros
}

/// The accent at 14 %, the design language's selected-row fill.
fn accent_soft() -> Hsla {
    rgba((ACCENT << 8) | 0x24).into()
}

// --- the tile card and the curve glyphs ---------------------------------------------

/// A picture card: a rounded frame with a mountain and a sun cut out, so a
/// moving card reads as "your clip".
const CARD_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><path fill="black" fill-rule="evenodd" d="M6 6h28a4 4 0 0 1 4 4v20a4 4 0 0 1-4 4H6a4 4 0 0 1-4-4V10a4 4 0 0 1 4-4zM7 29l9-11 6 7 4-4 7 8zM28 11.5a3 3 0 1 0 0.01 0z"/></svg>"#;

/// "None": a slashed circle.
const NONE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round"><circle cx="12" cy="12" r="8"/><path d="M6.5 17.5l11-11"/></svg>"#;

/// The curve of every easing, as an SVG polyline, built once.
fn curve_svg(ease: Ease) -> &'static [u8] {
    static CURVES: OnceLock<Vec<(Ease, Vec<u8>)>> = OnceLock::new();
    let all = CURVES.get_or_init(|| {
        Ease::ALL
            .iter()
            .map(|&e| {
                let points: Vec<String> = (0..=32)
                    .map(|i| {
                        let t = i as f32 / 32.0;
                        format!("{:.2},{:.2}", 3.0 + 34.0 * t, 20.0 - 14.0 * e.apply(t))
                    })
                    .collect();
                let svg = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><polyline points="{}"/></svg>"#,
                    points.join(" ")
                );
                (e, svg.into_bytes())
            })
            .collect()
    });
    all.iter()
        .find(|(e, _)| *e == ease)
        .map(|(_, bytes)| bytes.as_slice())
        .unwrap_or(NONE_SVG)
}

/// The card in a tile, posed. `reveal` clips it; `blur` only dims, since a
/// tile has no blur.
fn posed_card(pose: &pose::Pose, color: u32) -> AnyElement {
    let opacity = (pose.opacity * (1.0 - 0.8 * pose.blur)).clamp(0.0, 1.0);
    let tint: Hsla = Hsla::from(rgb(color)).opacity(opacity);
    let transformation = Transformation::rotate(gpui::radians(pose.rotation.to_radians()))
        .with_scaling(gpui::size(pose.scale[0], pose.scale[1]))
        .with_translation(gpui::point(
            px(pose.offset[0] * TILE_TRAVEL),
            px(-pose.offset[1] * TILE_TRAVEL),
        ));
    let [x0, y0, x1, y1] = pose.reveal;
    if !pose.reveals_part() {
        // No clip box: a card that travels or grows must be free to leave
        // its own rectangle (the tile clips it).
        return svg()
            .data(CARD_SVG)
            .absolute()
            .left(px((TILE - CARD) / 2.0))
            .top(px((TILE - CARD) / 2.0))
            .size(px(CARD))
            .text_color(tint)
            .with_transformation(transformation)
            .into_any_element();
    }
    // A wipe: the card's outline, faint, and the revealed part of it solid.
    // Drawn with plain boxes: an SVG inside a clipping box does not clip
    // the way its layout box suggests, and a wipe needs no mountain to read.
    let origin = (TILE - CARD) / 2.0;
    div()
        .absolute()
        .left(px(origin))
        .top(px(origin))
        .size(px(CARD))
        .child(
            div()
                .absolute()
                .left(px(1.0))
                .top(px(5.0))
                .w(px(CARD - 2.0))
                .h(px(CARD - 10.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(Hsla::from(rgb(color)).opacity(0.25)),
        )
        .child(
            div()
                .absolute()
                .left(px(1.0 + x0 * (CARD - 2.0)))
                .top(px(5.0 + y0 * (CARD - 10.0)))
                .w(px(((x1 - x0) * (CARD - 2.0)).max(0.0)))
                .h(px(((y1 - y0) * (CARD - 10.0)).max(0.0)))
                .rounded(px(2.0))
                .bg(tint),
        )
        .into_any_element()
}

/// A clip preset's pose for a tile: a characteristic frozen moment, or the
/// loop while hovered (`elapsed` seconds into it).
fn tile_pose(
    slot: AnimationSlot,
    d: &catalog::PresetDescriptor,
    elapsed: Option<f32>,
) -> pose::Pose {
    let length = secs(d.duration).max(0.2);
    match slot {
        AnimationSlot::Combo => {
            let t = elapsed.unwrap_or(length * 0.25);
            pose::combo_pose(d.preset, t / length, t, 1.0, d.easing)
        }
        AnimationSlot::In | AnimationSlot::Out => {
            // Play, hold at rest for a beat, repeat; Out plays in reverse.
            let cycle = length + 0.7;
            let Some(t) = elapsed else {
                // At rest: the motion half done, whatever its curve.
                return pose::preset_pose(d.preset, 0.5, 0.3, 1.0);
            };
            let raw = ((t % cycle) / length).min(1.0);
            let raw = if slot == AnimationSlot::Out {
                1.0 - raw
            } else {
                raw
            };
            pose::preset_pose(d.preset, d.easing.apply(raw), t, 1.0)
        }
    }
}

// --- the tab --------------------------------------------------------------------------

impl Editor {
    /// The Animation tab of the selected clip: sub-tab row, body, footer.
    pub(super) fn animation_tab(
        &mut self,
        segment: &Segment,
        kind: ClipKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, AnyElement, Option<AnyElement>) {
        let names: &[&'static str] = if kind == ClipKind::Text {
            &[IN, OUT, COMBO, TEXT_TAB]
        } else {
            &[IN, OUT, COMBO, ZOOM]
        };
        let current = self
            .inspector
            .sub_tab
            .get(OWNER)
            .copied()
            .filter(|tab| names.contains(tab))
            .unwrap_or(IN);
        let tabs = sub_tabs(OWNER, names, current, cx).into_any_element();
        let body = match current {
            ZOOM => self.anim_zoom_body(segment, window, cx),
            TEXT_TAB => self.anim_text_body(segment, window, cx),
            tab => self.anim_slot_body(segment, slot_of(tab), window, cx),
        };
        if self.inspector.animation.hovered.is_some() {
            window.request_animation_frame();
        }
        (Some(tabs), body, None)
    }

    fn anim_material_of(&self, segment: &Segment) -> Option<AnimationMaterial> {
        self.project.materials.animation_of(segment).cloned()
    }

    /// Seconds since the pointer entered `key`'s tile, if it is the hovered one.
    fn anim_hover_time(&self, key: TileKey) -> Option<f32> {
        match self.inspector.animation.hovered {
            Some((hovered, since)) if hovered == key => Some(since.elapsed().as_secs_f32()),
            _ => None,
        }
    }

    // --- In / Out / Combo -----------------------------------------------------------

    fn anim_slot_body(
        &mut self,
        segment: &Segment,
        slot: AnimationSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let material = self.anim_material_of(segment);
        let current = slot_value(material.as_ref(), slot);
        let presets: Vec<_> = catalog::clip_presets()
            .into_iter()
            .filter(|d| d.fits(slot))
            .collect();

        let mut tiles = vec![self.anim_none_tile(
            SharedString::from(format!("anim-none-{slot:?}")),
            current.is_none(),
            cx.listener(move |this, _, _, cx| this.anim_set_slot(slot, None, cx)),
        )];
        for d in presets {
            let key = TileKey::Clip(slot, d.preset);
            let pose = tile_pose(slot, &d, self.anim_hover_time(key));
            let selected = current.is_some_and(|c| c.preset == d.preset);
            let preset = d.preset;
            tiles.push(self.anim_tile(
                SharedString::from(format!("anim-{slot:?}-{preset:?}")),
                d.label,
                selected,
                key,
                posed_card(&pose, if selected { ACCENT } else { TEXT_DIM }),
                cx.listener(move |this, _, _, cx| this.anim_pick_preset(slot, preset, cx)),
                cx,
            ));
        }

        let mut rows = Vec::new();
        if let Some(anim) = current {
            rows.push(self.anim_knob_row(Knob::Duration(slot), secs(anim.duration), window, cx));
            rows.push(self.anim_knob_row(Knob::Strength(slot), anim.strength * 100.0, window, cx));
            rows.push(self.anim_ease_picker(
                &format!("ease-{slot:?}"),
                anim.easing,
                move |this, ease, cx| {
                    this.anim_edit_slot(slot, move |a| a.easing = ease, cx);
                },
                cx,
            ));
        }
        let hint = match slot {
            AnimationSlot::In => {
                "Plays from the clip's first frame, and follows the clip through trims."
            }
            AnimationSlot::Out => {
                "Plays into the clip's last frame, and follows the clip through trims."
            }
            AnimationSlot::Combo => "Loops for the whole clip.",
        };
        self.anim_tab_body(tiles, rows, hint)
    }

    fn anim_tab_body(
        &self,
        tiles: Vec<AnyElement>,
        rows: Vec<AnyElement>,
        hint: &'static str,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .pt_1()
                    .pb_3()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(8.0))
                    .children(tiles),
            )
            .child(
                div()
                    .px_3()
                    .pb_3()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(hint),
            )
            .when(!rows.is_empty(), |this| {
                this.child(
                    div()
                        .px_3()
                        .py_3()
                        .border_t_1()
                        .border_color(rgb(HAIRLINE))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .children(rows),
                )
            })
            .into_any_element()
    }

    /// The first tile of every grid: no animation.
    fn anim_none_tile(
        &self,
        id: SharedString,
        selected: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        tile_frame(id, "None", selected)
            .child(
                div()
                    .size(px(TILE))
                    .rounded(px(R_SM))
                    .bg(rgb(WELL))
                    .border_2()
                    .border_color(rgb(if selected { ACCENT } else { HAIRLINE }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .data(NONE_SVG)
                            .size(px(24.0))
                            .text_color(rgb(if selected { ACCENT } else { TEXT_DIM })),
                    ),
            )
            .child(tile_label("None", selected))
            .on_click(on_click)
            .into_any_element()
    }

    /// One preset tile: the preview box and the name under it.
    #[allow(clippy::too_many_arguments)]
    fn anim_tile(
        &self,
        id: SharedString,
        label: &'static str,
        selected: bool,
        key: TileKey,
        art: AnyElement,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        tile_frame(id, label, selected)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let tab = &mut this.inspector.animation;
                if *hovered {
                    tab.hovered = Some((key, Instant::now()));
                } else if tab.hovered.is_some_and(|(k, _)| k == key) {
                    tab.hovered = None;
                }
                cx.notify();
            }))
            .child(
                div()
                    .relative()
                    .size(px(TILE))
                    .rounded(px(R_SM))
                    .bg(rgb(WELL))
                    .overflow_hidden()
                    .border_2()
                    .border_color(rgb(if selected { ACCENT } else { HAIRLINE }))
                    .child(art),
            )
            .child(tile_label(label, selected))
            .on_click(on_click)
            .into_any_element()
    }

    /// Click on a preset: apply it with its defaults, keeping the timing the
    /// slot already had when it had one.
    fn anim_pick_preset(
        &mut self,
        slot: AnimationSlot,
        preset: AnimationPreset,
        cx: &mut Context<Self>,
    ) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let current = slot_value(self.project.materials.animation_of(segment), slot);
        let d = catalog::clip_preset(preset);
        let mut anim = d.animation();
        if let Some(c) = current {
            if c.preset == preset {
                return;
            }
            if slot != AnimationSlot::Combo {
                anim.duration = c.duration;
            }
        }
        self.anim_set_slot(slot, Some(anim), cx);
    }

    fn anim_set_slot(
        &mut self,
        slot: AnimationSlot,
        anim: Option<ClipAnimation>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let command = motion_edit::set_slot_command(&self.project, &id, slot, anim);
        self.anim_apply_motion(command, cx);
    }

    fn anim_edit_slot(
        &mut self,
        slot: AnimationSlot,
        change: impl FnOnce(&mut ClipAnimation),
        cx: &mut Context<Self>,
    ) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let Some(mut anim) = slot_value(self.project.materials.animation_of(segment), slot) else {
            return;
        };
        change(&mut anim);
        self.anim_set_slot(slot, Some(anim), cx);
    }

    /// Apply a motion command; "nothing to change" is not an error to show.
    fn anim_apply_motion(&mut self, command: Result<EditCommand, String>, cx: &mut Context<Self>) {
        match command {
            Err(e) if e == "nothing to change" => {}
            command => self.apply(command, cx),
        }
    }

    // --- easing picker ------------------------------------------------------------------

    fn anim_ease_picker(
        &self,
        id: &str,
        current: Ease,
        on_pick: impl Fn(&mut Editor, Ease, &mut Context<Editor>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_pick = std::rc::Rc::new(on_pick);
        let chips = Ease::ALL.iter().map(|&ease| {
            let selected = ease == current;
            let on_pick = std::rc::Rc::clone(&on_pick);
            div()
                .id(SharedString::from(format!("{id}-{ease:?}")))
                .w(px(78.0))
                .h(px(48.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(2.0))
                .rounded(px(R_SM))
                .bg(if selected {
                    accent_soft()
                } else {
                    rgb(WELL).into()
                })
                .border_1()
                .border_color(rgb(if selected { ACCENT } else { HAIRLINE }))
                .cursor_pointer()
                .hover(|style| style.border_color(rgb(BORDER_STRONG)))
                .on_click(cx.listener(move |this, _, _, cx| on_pick(this, ease, cx)))
                .child(
                    svg()
                        .data(curve_svg(ease))
                        .w(px(40.0))
                        .h(px(24.0))
                        .text_color(rgb(if selected { ACCENT } else { TEXT_DIM })),
                )
                .child(
                    div()
                        .text_size(px(TEXT_BADGE))
                        .text_color(rgb(if selected { TEXT } else { TEXT_MUTED }))
                        .child(ease.label()),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(row_label("Easing"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(chips),
            )
            .into_any_element()
    }

    /// A row of mutually exclusive text chips.
    fn anim_chips<T: Copy + PartialEq + 'static>(
        &self,
        id: &str,
        label: &'static str,
        options: &[(T, &'static str)],
        current: T,
        on_pick: impl Fn(&mut Editor, T, &mut Context<Editor>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_pick = std::rc::Rc::new(on_pick);
        let buttons = options.iter().map(|&(value, name)| {
            let selected = value == current;
            let on_pick = std::rc::Rc::clone(&on_pick);
            div()
                .id(SharedString::from(format!("{id}-{name}")))
                .h(px(CONTROL_H))
                .px_3()
                .flex()
                .items_center()
                .rounded(px(R_SM))
                .text_size(px(TEXT_LABEL))
                .bg(if selected {
                    accent_soft()
                } else {
                    rgb(WELL).into()
                })
                .border_1()
                .border_color(rgb(if selected { ACCENT } else { HAIRLINE }))
                .text_color(rgb(if selected { ACCENT } else { TEXT_DIM }))
                .cursor_pointer()
                .hover(|style| style.text_color(rgb(TEXT)))
                .on_click(cx.listener(move |this, _, _, cx| on_pick(this, value, cx)))
                .child(name)
        });
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .child(div().w(px(96.0)).flex_none().child(row_label(label)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(buttons),
            )
            .into_any_element()
    }

    // --- sliders --------------------------------------------------------------------------

    /// The slider of `knob`, made on first use.
    fn anim_knob_slider(
        &mut self,
        knob: Knob,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SliderState> {
        if let Some(widget) = self.inspector.animation.knobs.get(&knob) {
            return widget.slider.clone();
        }
        let (min, max, step) = knob.range();
        let slider = cx.new(|_| {
            SliderState::new()
                .min(min)
                .max(max)
                .step(step)
                .default_value(min)
        });
        let subscription = cx.subscribe_in(
            &slider,
            window,
            move |this: &mut Editor, _, event: &SliderEvent, _, cx| match event {
                SliderEvent::Change(value) => {
                    this.anim_set_knob(knob, value.end(), Phase::Preview, cx)
                }
                SliderEvent::Release(value) => {
                    this.anim_set_knob(knob, value.end(), Phase::Commit, cx)
                }
            },
        );
        self.inspector.animation.knobs.insert(
            knob,
            KnobWidget {
                slider: slider.clone(),
                _subscription: subscription,
            },
        );
        slider
    }

    /// A labelled slider with its value, kept in step with the document.
    fn anim_knob_row(
        &mut self,
        knob: Knob,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slider = self.anim_knob_slider(knob, window, cx);
        let dragging = self
            .inspector
            .animation
            .drag
            .as_ref()
            .is_some_and(|(k, _)| *k == knob);
        let (min, max, _) = knob.range();
        let shown = value.clamp(min, max);
        if !dragging && (slider.read(cx).value().end() - shown).abs() > 1e-4 {
            slider.update(cx, |state, cx| state.set_value(shown, window, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(row_label(knob.label()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().px_1().child(Slider::new(&slider)))
                    .child(
                        div()
                            .w(px(56.0))
                            .h(px(CONTROL_H))
                            .px_2()
                            .flex()
                            .items_center()
                            .justify_end()
                            .rounded(px(R_SM))
                            .bg(rgb(WELL))
                            .font_family(FONT_MONO)
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT))
                            .child(knob.format(value)),
                    ),
            )
            .into_any_element()
    }

    /// The command that sets `knob` to `value` (panel units) on the selected
    /// clip, built against `project`.
    fn anim_knob_command(
        &self,
        project: &Project,
        knob: Knob,
        value: f32,
    ) -> Result<EditCommand, String> {
        let id = self.selected.clone().ok_or("no clip is selected")?;
        let (_, segment) = project
            .segment(&id)
            .ok_or("the clip is no longer on the timeline")?;
        let material = project.materials.animation_of(segment);
        match knob {
            Knob::Duration(slot) | Knob::Strength(slot) => {
                let mut anim = slot_value(material, slot).ok_or("nothing to change")?;
                if let Knob::Duration(_) = knob {
                    anim.duration = micros(value);
                } else {
                    anim.strength = value / 100.0;
                }
                motion_edit::set_slot_command(project, &id, slot, Some(anim))
            }
            Knob::TextDuration(slot) | Knob::TextOverlap(slot) | Knob::TextStrength(slot) => {
                let mut animator = text_value(material, slot).ok_or("nothing to change")?;
                match knob {
                    Knob::TextDuration(_) => animator.duration = micros(value),
                    Knob::TextOverlap(_) => animator.overlap = value / 100.0,
                    _ => animator.strength = value / 100.0,
                }
                motion_edit::set_text_command(project, &id, slot, Some(animator))
            }
            Knob::ZoomAmount | Knob::ZoomRamp => {
                let mut zoom = material.and_then(|m| m.zoom).ok_or("nothing to change")?;
                if knob == Knob::ZoomAmount {
                    zoom.amount = value / 100.0;
                } else {
                    zoom.duration = micros(value);
                }
                motion_edit::set_zoom_command(project, &id, Some(zoom))
            }
            Knob::AutoAmount => Err("nothing to change".into()),
        }
    }

    fn anim_set_knob(&mut self, knob: Knob, value: f32, phase: Phase, cx: &mut Context<Self>) {
        if !value.is_finite() {
            return;
        }
        if knob == Knob::AutoAmount {
            self.inspector.animation.auto_amount = value;
            cx.notify();
            return;
        }
        if phase == Phase::Preview {
            let base = match &self.inspector.animation.drag {
                Some((k, base)) if *k == knob => Arc::clone(base),
                _ => {
                    let base = Arc::clone(&self.project);
                    self.inspector.animation.drag = Some((knob, Arc::clone(&base)));
                    base
                }
            };
            let shown = self
                .anim_knob_command(&base, knob, value)
                .and_then(|command| {
                    let mut copy = (*base).clone();
                    command.apply(&mut copy).map(|_| copy)
                });
            if let Ok(project) = shown {
                self.project = Arc::new(project);
                self.generation += 1;
                cx.notify();
            }
            return;
        }
        if let Some((_, base)) = self.inspector.animation.drag.take() {
            self.project = base;
            self.generation += 1;
        }
        let project = Arc::clone(&self.project);
        let command = self.anim_knob_command(&project, knob, value);
        match command {
            Err(e) if e == "nothing to change" => self.refresh(cx),
            command => self.apply(command, cx),
        }
    }

    // --- Text -------------------------------------------------------------------------------

    fn anim_text_body(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slot = self.inspector.animation.text_slot;
        let material = self.anim_material_of(segment);
        let current = text_value(material.as_ref(), slot);

        let slot_chips = self.anim_chips(
            "text-slot",
            "Animate",
            &[(TextSlot::In, "Entrance"), (TextSlot::Out, "Exit")],
            slot,
            |this, slot, cx| {
                this.inspector.animation.text_slot = slot;
                cx.notify();
            },
            cx,
        );

        let mut tiles = vec![self.anim_none_tile(
            SharedString::from(format!("text-none-{slot:?}")),
            current.is_none(),
            cx.listener(move |this, _, _, cx| this.anim_set_text(slot, None, cx)),
        )];
        for d in catalog::text_presets() {
            let key = TileKey::Text(d.preset);
            let selected = current.is_some_and(|c| c.preset == d.preset);
            let art = text_tile_art(&d.animator, slot, self.anim_hover_time(key), selected);
            let preset = d.preset;
            tiles.push(self.anim_tile(
                SharedString::from(format!("text-{slot:?}-{preset:?}")),
                d.label,
                selected,
                key,
                art,
                cx.listener(move |this, _, _, cx| this.anim_pick_text(slot, preset, cx)),
                cx,
            ));
        }

        let mut rows = Vec::new();
        if let Some(a) = current {
            rows.push(self.anim_chips(
                "text-unit",
                "By",
                &[
                    (TextUnit::Letter, "Letter"),
                    (TextUnit::Word, "Word"),
                    (TextUnit::Line, "Line"),
                ],
                a.unit,
                move |this, unit, cx| this.anim_edit_text(slot, move |a| a.unit = unit, cx),
                cx,
            ));
            rows.push(self.anim_chips(
                "text-order",
                "Order",
                &[
                    (StaggerOrder::Forward, "Forward"),
                    (StaggerOrder::Backward, "Backward"),
                    (StaggerOrder::Centre, "From centre"),
                    (StaggerOrder::Random, "Random"),
                ],
                a.order,
                move |this, order, cx| {
                    this.anim_edit_text(
                        slot,
                        move |a| {
                            // A new shuffle each time Random is chosen again.
                            if order == StaggerOrder::Random && a.order == StaggerOrder::Random {
                                a.seed = a.seed.wrapping_add(1);
                            }
                            a.order = order;
                        },
                        cx,
                    )
                },
                cx,
            ));
            rows.push(self.anim_knob_row(Knob::TextDuration(slot), secs(a.duration), window, cx));
            rows.push(self.anim_knob_row(Knob::TextOverlap(slot), a.overlap * 100.0, window, cx));
            rows.push(self.anim_knob_row(Knob::TextStrength(slot), a.strength * 100.0, window, cx));
            rows.push(self.anim_ease_picker(
                &format!("text-ease-{slot:?}"),
                a.easing,
                move |this, ease, cx| this.anim_edit_text(slot, move |a| a.easing = ease, cx),
                cx,
            ));
        }

        div()
            .flex()
            .flex_col()
            .child(div().px_3().pb_3().child(slot_chips))
            .child(self.anim_tab_body(
                tiles,
                rows,
                "Animates the title one letter, word or line at a time.",
            ))
            .into_any_element()
    }

    fn anim_pick_text(&mut self, slot: TextSlot, preset: TextPreset, cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let current = text_value(self.project.materials.animation_of(segment), slot);
        if current.is_some_and(|c| c.preset == preset) {
            return;
        }
        self.anim_set_text(slot, Some(catalog::text_preset(preset).animator), cx);
    }

    fn anim_set_text(
        &mut self,
        slot: TextSlot,
        animator: Option<TextAnimator>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let command = motion_edit::set_text_command(&self.project, &id, slot, animator);
        self.anim_apply_motion(command, cx);
    }

    fn anim_edit_text(
        &mut self,
        slot: TextSlot,
        change: impl FnOnce(&mut TextAnimator),
        cx: &mut Context<Self>,
    ) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let Some(mut animator) = text_value(self.project.materials.animation_of(segment), slot)
        else {
            return;
        };
        change(&mut animator);
        self.anim_set_text(slot, Some(animator), cx);
    }

    // --- Zoom -------------------------------------------------------------------------------

    fn anim_zoom_body(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = self.anim_material_of(segment).and_then(|m| m.zoom);
        let enabled = zoom.is_some();

        let mut rows = vec![div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(check_box(
                "zoom-enable".into(),
                enabled,
                true,
                cx.listener(move |this, _, _, cx| {
                    let next = (!enabled).then(|| PunchZoom {
                        duration: 300_000,
                        ..PunchZoom::default()
                    });
                    this.anim_set_zoom(next, cx);
                }),
            ))
            .child(
                div()
                    .text_size(px(TEXT_BODY))
                    .text_color(rgb(TEXT))
                    .child("Punch-in zoom"),
            )
            .into_any_element()];

        if let Some(z) = zoom {
            rows.push(self.anim_knob_row(Knob::ZoomAmount, z.amount * 100.0, window, cx));
            rows.push(self.anim_knob_row(Knob::ZoomRamp, secs(z.duration), window, cx));
            rows.push(self.anim_ease_picker(
                "zoom-ease",
                z.easing,
                |this, ease, cx| this.anim_edit_zoom(move |z| z.easing = ease, cx),
                cx,
            ));
            let (hw, hh) = self.half_canvas();
            rows.push(label_row(
                "Pivot",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT))
                            .child(format!(
                                "X {:.0}  Y {:.0}",
                                z.pivot[0] * hw,
                                z.pivot[1] * hh
                            )),
                    )
                    .child(panel_button(
                        "zoom-centre",
                        "Centre",
                        false,
                        z.pivot != [0.0, 0.0],
                        cx.listener(|this, _, _, cx| {
                            this.anim_edit_zoom(|z| z.pivot = [0.0, 0.0], cx)
                        }),
                    )),
            ));
            rows.push(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Drag the crosshair on the player to put the pivot on a face.")
                    .into_any_element(),
            );
        }

        // Auto zoom across the jump cuts this clip belongs to.
        let run = motion_edit::jump_cut_run(&self.project, &segment.id).map_or(0, |r| r.len());
        let amount = self.inspector.animation.auto_amount;
        let mut auto_rows = vec![
            div()
                .text_size(px(TEXT_CAPTION))
                .text_color(rgb(TEXT_MUTED))
                .child(if run > 1 {
                    format!(
                        "{run} jump cuts from this take sit end to end. Every second one is punched in, as one undo step."
                    )
                } else {
                    "No jump cuts around this clip yet: split it or cut its silences first.".to_string()
                })
                .into_any_element(),
            self.anim_knob_row(Knob::AutoAmount, amount, window, cx),
        ];
        auto_rows.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(panel_button(
                    "auto-zoom",
                    "Auto zoom jump cuts",
                    true,
                    run > 1,
                    cx.listener(move |this, _, _, cx| {
                        let Some(id) = this.selected.clone() else {
                            return;
                        };
                        let command =
                            motion_edit::auto_zoom_command(&this.project, &id, amount / 100.0);
                        this.apply(command, cx);
                    }),
                ))
                .into_any_element(),
        );

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .children(rows),
            )
            .child(
                div()
                    .px_3()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(HAIRLINE))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .child("Auto zoom"),
                    )
                    .children(auto_rows),
            )
            .into_any_element()
    }

    fn anim_set_zoom(&mut self, zoom: Option<PunchZoom>, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let command = motion_edit::set_zoom_command(&self.project, &id, zoom);
        self.anim_apply_motion(command, cx);
    }

    fn anim_edit_zoom(&mut self, change: impl FnOnce(&mut PunchZoom), cx: &mut Context<Self>) {
        let Some((_, segment)) = self.selected_segment() else {
            return;
        };
        let Some(mut zoom) = self
            .project
            .materials
            .animation_of(segment)
            .and_then(|m| m.zoom)
        else {
            return;
        };
        change(&mut zoom);
        self.anim_set_zoom(Some(zoom), cx);
    }

    // --- the pivot on the player ----------------------------------------------------------

    /// The zoom pivot's crosshair and the frame the zoom keeps, over the
    /// player's picture (`dw` × `dh` pixels), while the Zoom sub-tab is open.
    /// Dragging moves the pivot; releasing writes it as one undo step.
    pub(in crate::editor) fn motion_overlay(
        &self,
        dw: f32,
        dh: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.inspector.tab != Some(OWNER)
            || self.inspector.sub_tab.get(OWNER).copied() != Some(ZOOM)
        {
            return None;
        }
        let (_, segment) = self.selected_segment()?;
        if self.clip_kind(segment) == ClipKind::Text || dw < 4.0 || dh < 4.0 {
            return None;
        }
        let zoom = self.project.materials.animation_of(segment)?.zoom?;
        // Normalized canvas (+y up) to overlay pixels.
        let to_px = |x: f32, y: f32| ((x + 1.0) * 0.5 * dw, (1.0 - y) * 0.5 * dh);
        let (px_x, px_y) = to_px(zoom.pivot[0], zoom.pivot[1]);
        // What stays in frame once zoomed: the canvas scaled about the pivot
        // by the inverse of the amount.
        let s = zoom.amount.max(0.01);
        let edge = |c: f32, p: f32| p + (c - p) / s;
        let (l, t) = to_px(edge(-1.0, zoom.pivot[0]), edge(1.0, zoom.pivot[1]));
        let (r, b) = to_px(edge(1.0, zoom.pivot[0]), edge(-1.0, zoom.pivot[1]));
        const MARK: f32 = 22.0;

        let overlay = div()
            .id("zoom-pivot-overlay")
            .absolute()
            .top_0()
            .left_0()
            .w(px(dw))
            .h(px(dh))
            .cursor_crosshair()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.inspector.animation.pivot_drag = Some(Arc::clone(&this.project));
                    this.anim_drag_pivot(event.position, dw, dh, false, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if this.inspector.animation.pivot_drag.is_none() {
                    return;
                }
                // A release outside the picture never reaches us; the next
                // move without the button down finishes the drag.
                let held = event.pressed_button == Some(MouseButton::Left);
                this.anim_drag_pivot(event.position, dw, dh, !held, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseUpEvent, _, cx| {
                    if this.inspector.animation.pivot_drag.is_some() {
                        this.anim_drag_pivot(event.position, dw, dh, true, cx);
                    }
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(l))
                    .top(px(t))
                    .w(px((r - l).max(0.0)))
                    .h(px((b - t).max(0.0)))
                    .border_1()
                    .border_color(Hsla::from(rgb(ACCENT)).opacity(0.7)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(px_x - MARK / 2.0))
                    .top(px(px_y - MARK / 2.0))
                    .size(px(MARK))
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(ACCENT))
                    .bg(Hsla::from(rgb(ACCENT)).opacity(0.18))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px(4.0)).rounded_full().bg(rgb(ACCENT))),
            );
        Some(overlay.into_any_element())
    }

    /// Move the pivot to the window point `position`: shown on a copy while
    /// dragging, written once when `finish`.
    fn anim_drag_pivot(
        &mut self,
        position: Point<Pixels>,
        dw: f32,
        dh: f32,
        finish: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(base) = self.inspector.animation.pivot_drag.clone() else {
            return;
        };
        let bounds = self.viewer.get();
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        // The picture is centred in the viewer; see `render_preview`.
        let left = f32::from(bounds.origin.x) + (bw - dw) / 2.0;
        let top = f32::from(bounds.origin.y) + (bh - dh) / 2.0;
        let x = ((f32::from(position.x) - left) / dw).clamp(0.0, 1.0) * 2.0 - 1.0;
        let y = 1.0 - ((f32::from(position.y) - top) / dh).clamp(0.0, 1.0) * 2.0;

        let Some(id) = self.selected.clone() else {
            return;
        };
        let build = |project: &Project| {
            let (_, segment) = project.segment(&id).ok_or("the clip is gone")?;
            let mut zoom = project
                .materials
                .animation_of(segment)
                .and_then(|m| m.zoom)
                .ok_or("the clip has no zoom")?;
            zoom.pivot = [x, y];
            motion_edit::set_zoom_command(project, &id, Some(zoom))
        };
        if finish {
            self.inspector.animation.pivot_drag = None;
            self.project = base;
            self.generation += 1;
            let project = Arc::clone(&self.project);
            let command = build(&project);
            self.anim_apply_motion(command, cx);
            return;
        }
        let shown = build(&base).and_then(|command| {
            let mut copy = (*base).clone();
            command.apply(&mut copy).map(|_| copy)
        });
        if let Ok(project) = shown {
            self.project = Arc::new(project);
            self.generation += 1;
            cx.notify();
        }
    }
}

// --- small pieces --------------------------------------------------------------------------

fn tile_frame(
    id: SharedString,
    _label: &'static str,
    _selected: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w(px(TILE))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.0))
        .cursor_pointer()
}

fn tile_label(label: &'static str, selected: bool) -> AnyElement {
    div()
        .w(px(TILE + 6.0))
        .text_center()
        .truncate()
        .text_size(px(TEXT_CAPTION))
        .text_color(rgb(if selected { ACCENT } else { TEXT_DIM }))
        .child(label)
        .into_any_element()
}

fn row_label(label: &'static str) -> AnyElement {
    div()
        .text_size(px(TEXT_LABEL))
        .text_color(rgb(TEXT_DIM))
        .child(label)
        .into_any_element()
}

/// A text tile: "Abc" with each letter posed by the animator's own stagger
/// and curves, frozen part-way or looping while hovered.
fn text_tile_art(
    animator: &TextAnimator,
    slot: TextSlot,
    elapsed: Option<f32>,
    selected: bool,
) -> AnyElement {
    const LETTERS: [&str; 3] = ["A", "b", "c"];
    const EM: f32 = 22.0;
    let window = secs(animator.duration).max(0.2);
    let cycle = window + 0.7;
    let t = match elapsed {
        Some(e) => (e % cycle).min(window),
        None => window * 0.45,
    };
    let t = if slot == TextSlot::Out { window - t } else { t };
    let ranks = motion_text::ranks(LETTERS.len(), animator.order, animator.seed);
    let max_rank = ranks.iter().copied().fold(0.0, f32::max);
    let color = if selected { ACCENT } else { TEXT };
    let letters = LETTERS.iter().zip(&ranks).map(|(letter, &rank)| {
        let raw =
            motion_text::unit_progress(micros(t), micros(window), rank, max_rank, animator.overlap);
        let p = animator.easing.apply(raw);
        let g = motion_text::unit_pose(animator.preset, p, raw, animator.strength, EM);
        let size = (EM * g.scale.clamp(0.0, 3.0)).max(0.1);
        div()
            .w(px(15.0))
            .h(px(40.0))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .top(px(g.dy * 0.6))
                    .left(px(g.dx * 0.6))
                    .text_size(px(size))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(Hsla::from(rgb(color)).opacity(g.opacity.clamp(0.0, 1.0)))
                    .child(*letter),
            )
    });
    div()
        .size_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .children(letters)
        .into_any_element()
}
