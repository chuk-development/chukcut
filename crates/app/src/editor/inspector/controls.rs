//! The inspector's building blocks, on the design kit (`crate::ui`): the
//! top tabs are a `PanelHeader`, the sub-tabs `SegmentedTabs`, a section is
//! a kit `Section` with its `SectionHeader`, a number box a `NumberField`,
//! and every property row a `PropertyRow` with its reset and `KeyframeSlot`.
//! The helpers keep their old signatures so every tab builds its rows the
//! same way; only the drawing moved to the kit.

use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::slider::Slider;
use gpui::component::{Disableable as _, Icon, Sizable};
use gpui::{AnyElement, ClickEvent};

use super::*;
use crate::ui::{
    self, EmptyState, Glyph, IconButton, KeyMark, KeyframeSlot, NumberField, PanelHeader,
    PropertyRow, SectionHeader, SegmentedTabs,
};

/// The inspector's glyphs: the kit's, plus the alignment set, drawn on the
/// kit's grid and stroke.
pub(crate) mod icons {
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
    pub const RESET: &[u8] = crate::ui::icons::RESET.0;
    // Kept for tabs that draw their own keyframe buttons (Effects).
    #[allow(dead_code)]
    pub const DIAMOND: &[u8] = crate::ui::icons::DIAMOND.0;
    #[allow(dead_code)]
    pub const DIAMOND_FILLED: &[u8] = crate::ui::icons::DIAMOND_FILLED.0;
    #[allow(dead_code)]
    pub const PREV: &[u8] = crate::ui::icons::CHEVRON_LEFT.0;
    pub const NEXT: &[u8] = crate::ui::icons::CHEVRON_RIGHT.0;
    pub const UP: &[u8] = crate::ui::icons::CHEVRON_UP.0;
    pub const DOWN: &[u8] = crate::ui::icons::CHEVRON_DOWN.0;
    icon!(
        ALIGN_LEFT,
        r#"<path d="M4 4v16"/><path d="M8 8h11v3H8zM8 13h7v3H8z"/>"#
    );
    icon!(
        ALIGN_HCENTER,
        r#"<path d="M12 4v16"/><path d="M6 8h12v3H6zM8 13h8v3H8z"/>"#
    );
    icon!(
        ALIGN_RIGHT,
        r#"<path d="M20 4v16"/><path d="M5 8h11v3H5zM9 13h7v3H9z"/>"#
    );
    icon!(
        ALIGN_TOP,
        r#"<path d="M4 4h16"/><path d="M8 8h3v11H8zM13 8h3v7h-3z"/>"#
    );
    icon!(
        ALIGN_VCENTER,
        r#"<path d="M4 12h16"/><path d="M8 6h3v12H8zM13 8h3v8h-3z"/>"#
    );
    icon!(
        ALIGN_BOTTOM,
        r#"<path d="M4 20h16"/><path d="M8 5h3v11H8zM13 9h3v7h-3z"/>"#
    );
}

pub(crate) fn icon(data: &'static [u8], size: f32, color: u32) -> Icon {
    Icon::default()
        .data(data)
        .size(px(size))
        .text_color(rgb(color))
}

/// Text colour of a control that does nothing yet.
pub(crate) const DISABLED: u32 = TEXT_DISABLED;

/// A small square icon button: the kit's, with a resting tint.
pub(crate) fn icon_button(
    id: impl Into<gpui::ElementId>,
    data: &'static [u8],
    color: u32,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    IconButton::new(id, Glyph(data))
        .small()
        .tint(color)
        .disabled(!enabled)
        .on_click(on_click)
}

/// The top tab row: the kit's panel header with text tabs.
pub(crate) fn top_tabs(
    tabs: &[&'static str],
    active: &'static str,
    cx: &mut Context<Editor>,
) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    let all: Vec<&'static str> = tabs.to_vec();
    let selected = tabs.iter().position(|&tab| tab == active).unwrap_or(0);
    PanelHeader::new().tabs(
        "top-tab",
        tabs.iter().copied(),
        selected,
        move |index, _, cx| {
            let tab = all[index];
            let _ = entity.update(cx, |this, cx| {
                this.inspector.tab = Some(tab);
                cx.notify();
            });
        },
    )
}

/// The segmented control under the top tabs.
pub(crate) fn sub_tabs(
    owner: &'static str,
    tabs: &[&'static str],
    active: &'static str,
    cx: &mut Context<Editor>,
) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    let all: Vec<&'static str> = tabs.to_vec();
    let selected = tabs.iter().position(|&tab| tab == active).unwrap_or(0);
    div().flex_none().px(px(PAD)).pt(px(PAD)).pb(px(4.0)).child(
        SegmentedTabs::new(format!("sub-tab-{owner}"), tabs.iter().copied(), selected).on_select(
            move |index, _, cx| {
                let tab = all[index];
                let _ = entity.update(cx, |this, cx| {
                    this.inspector.sub_tab.insert(owner, tab);
                    cx.notify();
                });
            },
        ),
    )
}

/// A section: header with a collapse caret, an optional enable checkbox and
/// reset, then its rows — drawn by the kit's `Section`.
pub(crate) struct Section {
    pub title: &'static str,
    pub checkbox: Option<bool>,
    pub enabled: bool,
    pub on_reset: Option<Box<dyn Fn(&mut Editor, &mut Context<Editor>)>>,
    pub on_check: Option<Box<dyn Fn(&mut Editor, bool, &mut Context<Editor>)>>,
    pub note: Option<&'static str>,
}

impl Section {
    pub fn new(title: &'static str) -> Self {
        Self {
            title,
            checkbox: None,
            enabled: true,
            on_reset: None,
            on_check: None,
            note: None,
        }
    }

    /// A section for something the engine cannot do yet: drawn, greyed,
    /// with `note` saying so on hover-free text.
    pub fn missing(title: &'static str, note: &'static str) -> Self {
        Self {
            checkbox: Some(false),
            enabled: false,
            note: Some(note),
            ..Self::new(title)
        }
    }

    pub fn render(
        self,
        collapsed: bool,
        rows: Vec<AnyElement>,
        cx: &mut Context<Editor>,
    ) -> AnyElement {
        let title = self.title;
        let entity = cx.entity().downgrade();
        let mut header =
            SectionHeader::new(format!("section-{title}"), title).disabled(!self.enabled);
        if !rows.is_empty() {
            let entity = entity.clone();
            header = header.collapsible(collapsed, move |_, _, cx| {
                let _ = entity.update(cx, |this, cx| {
                    if !this.inspector.collapsed.remove(title) {
                        this.inspector.collapsed.insert(title);
                    }
                    cx.notify();
                });
            });
        }
        if let Some(checked) = self.checkbox {
            header = match self.on_check {
                Some(on_check) => {
                    let entity = entity.clone();
                    header.enable(checked, move |value, _, cx| {
                        let _ = entity.update(cx, |this, cx| on_check(this, value, cx));
                    })
                }
                None => header.checked(checked),
            };
        }
        if let Some(note) = self.note {
            header = header.note(note);
        }
        if let Some(on_reset) = self.on_reset {
            header = header.on_reset(move |_, _, cx| {
                let _ = entity.update(cx, |this, cx| on_reset(this, cx));
            });
        }
        ui::Section::new(SharedString::from(format!("section-body-{title}")), header)
            .mx(px(PAD))
            .children(rows)
            .into_any_element()
    }
}

/// A checkbox in the kit's proportions, for rows that are not a section
/// header.
pub(crate) fn check_box(
    id: SharedString,
    checked: bool,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let fill = match (checked, enabled) {
        (true, true) => Some(ACCENT),
        (true, false) => Some(BORDER_STRONG),
        (false, _) => None,
    };
    div()
        .id(id)
        .size(px(14.0))
        .flex_none()
        .rounded(px(R_XS))
        .border_1()
        .border_color(rgb(match fill {
            Some(color) => color,
            None if enabled => BORDER_STRONG,
            None => BORDER,
        }))
        .when_some(fill, |this, color| this.bg(rgb(color)))
        .flex()
        .items_center()
        .justify_center()
        .when(checked, |this| {
            this.child(ui::icons::glyph(ui::icons::CHECK, 12.0, rgb(ON_ACCENT)))
        })
        .when(enabled, |this| this.cursor_pointer().on_click(on_click))
}

/// A sub-heading inside a section ("Colour", "Light", "Effects").
pub(crate) fn group_label(label: &'static str) -> AnyElement {
    div()
        .pt(px(6.0))
        .pb(px(4.0))
        .border_b_1()
        .border_color(rgb(HAIRLINE))
        .text_size(px(TEXT_CAPTION))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(TEXT_MUTED))
        .child(label)
        .into_any_element()
}

/// Shown in place of a tab the engine has nothing behind yet.
pub(crate) fn not_yet(what: &str) -> AnyElement {
    div()
        .p(px(24.0))
        .child(
            EmptyState::new(
                SharedString::from(format!("not-yet-{what}")),
                gpui::assets::IconName::Construction,
                what.to_string(),
            )
            .hint("Not in the engine yet."),
        )
        .into_any_element()
}

impl Editor {
    /// The number box: the kit's field with its stacked stepper. `width` is
    /// the value's own width; the stepper and a prefix add to it.
    pub(crate) fn number_box(
        &mut self,
        prop: Prop,
        value: f32,
        width: f32,
        prefix: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spec = prop.spec();
        self.sync_field(prop, value, window, cx);
        let (input, _) = self.field(prop, window, cx);
        let entity = cx.entity().downgrade();
        let extra = 18.0 + if prefix.is_some() { 14.0 } else { 0.0 };
        NumberField::new(format!("number-{prop:?}"), &input)
            .width(width + extra)
            .disabled(!spec.supported)
            .when_some(prefix, |field, prefix| field.prefix(prefix))
            .when(!spec.suffix.is_empty(), |field| field.suffix(spec.suffix))
            .on_step(move |direction, _, cx| {
                let _ = entity.update(cx, |this, cx| this.step_prop(prop, direction, cx));
            })
            .into_any_element()
    }

    /// The keyframe slot of a property that can be animated.
    fn keyframe_slot(
        &self,
        prop: Prop,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> Option<KeyframeSlot> {
        if prop.animated().is_empty() {
            return None;
        }
        let (animated, at) = self.keyframe_state(prop, segment);
        let mark = match (animated, at) {
            (_, true) => KeyMark::OnKey,
            (true, false) => KeyMark::Animated,
            (false, false) => KeyMark::None,
        };
        let mut slot = KeyframeSlot::new(format!("kf-{prop:?}"), mark)
            .on_toggle(cx.listener(move |this, _, _, cx| this.toggle_keyframe(prop, cx)))
            .on_prev(cx.listener(move |this, _, _, cx| this.jump_keyframe(prop, false, cx)))
            .on_next(cx.listener(move |this, _, _, cx| this.jump_keyframe(prop, true, cx)));
        // Right-click on the diamond: the easing of the keyframe the
        // playhead is on, or moving away from.
        if let Some((target, current, _)) = self.easing_target(prop, segment) {
            let editor = cx.entity().downgrade();
            slot = slot.context_menu(std::rc::Rc::new(move |menu, _, _| {
                super::easing::easing_items(menu, editor.clone(), target.clone(), current)
            }));
        }
        Some(slot)
    }

    /// The reset and keyframe buttons at the right of a row. Rows without a
    /// keyframeable property keep the space, so every column lines up. For
    /// rows that lay out their own label and controls.
    #[allow(dead_code)]
    pub(crate) fn row_actions(
        &self,
        prop: Prop,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let reset: ui::OnClick =
            std::rc::Rc::new(cx.listener(move |this, _, _, cx| this.reset_prop(prop, cx)));
        ui::row_actions(
            format!("row-{prop:?}").into(),
            prop.spec().supported,
            Some(reset),
            self.keyframe_slot(prop, segment, cx),
        )
    }

    /// A slider row: the label above, then slider · number · actions.
    pub(crate) fn slider_row(
        &mut self,
        prop: Prop,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spec = prop.spec();
        let value = self.prop_value(prop, segment);
        let number = self.number_box(prop, value, 64.0, None, window, cx);
        let (_, slider) = self.field(prop, window, cx);
        let keyframe = self.keyframe_slot(prop, segment, cx);
        PropertyRow::new(format!("row-{prop:?}"), spec.label)
            .stacked()
            .disabled(!spec.supported)
            .on_reset(cx.listener(move |this, _, _, cx| this.reset_prop(prop, cx)))
            .when_some(keyframe, |row, slot| row.keyframe(slot))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .px(px(4.0))
                    .when_some(slider, |this, slider| {
                        this.child(slider_with_track(prop, &slider))
                    }),
            )
            .child(number)
            .into_any_element()
    }

    /// A row with the label on the left and one or more number boxes, for
    /// values CapCut gives no slider (position, rotation).
    pub(crate) fn field_row(
        &mut self,
        label: &'static str,
        props: &[(Prop, Option<&'static str>)],
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut boxes = Vec::new();
        for &(prop, prefix) in props {
            let value = self.prop_value(prop, segment);
            boxes.push(self.number_box(prop, value, 56.0, prefix, window, cx));
        }
        let first = props[0].0;
        let keyframe = self.keyframe_slot(first, segment, cx);
        PropertyRow::new(format!("row-{label}"), label)
            .disabled(!first.spec().supported)
            .on_reset(cx.listener(move |this, _, _, cx| this.reset_prop(first, cx)))
            .when_some(keyframe, |row, slot| row.keyframe(slot))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .gap(px(12.0))
                    .children(boxes),
            )
            .into_any_element()
    }
}

/// The slider of a row. A value that rests in the middle (the colour
/// adjustments) or does nothing yet gets no fill from the left, which would
/// read as "some of it is applied"; it gets CapCut's colour strip or a plain
/// rail drawn under a transparent bar instead.
pub(crate) fn slider_with_track(prop: Prop, slider: &Entity<SliderState>) -> AnyElement {
    let spec = prop.spec();
    let centred = spec.min < 0.0 && prop != Prop::Volume;
    if spec.supported && !centred {
        return Slider::new(slider).into_any_element();
    }
    let strip = |from: u32, to: u32| {
        gpui::linear_gradient(
            90.0,
            gpui::linear_color_stop(rgb(from), 0.0),
            gpui::linear_color_stop(rgb(to), 1.0),
        )
    };
    let track: gpui::Background = match prop {
        _ if !spec.supported => rgb(BORDER).into(),
        Prop::Temperature => strip(STRIP_COOL, STRIP_WARM),
        Prop::Tint => strip(STRIP_GREEN, STRIP_MAGENTA),
        Prop::Saturation => strip(STRIP_GREY, STRIP_RED),
        // The HSL rows draw the band they act on: its neighbouring hues, its
        // colour from grey to full, and from dark to light.
        Prop::HslHue(band) | Prop::HslSaturation(band) | Prop::HslLuminance(band) => {
            let centre = chukcut_engine::modules::project::grade::HSL_BANDS[(band as usize).min(7)]
                .1
                / 360.0;
            let hue = |h: f32, s: f32, l: f32| gpui::hsla(h.rem_euclid(1.0), s, l, 1.0);
            let (from, to) = match prop {
                Prop::HslHue(_) => (
                    hue(centre - 30.0 / 360.0, 0.8, 0.5),
                    hue(centre + 30.0 / 360.0, 0.8, 0.5),
                ),
                Prop::HslSaturation(_) => (hue(centre, 0.0, 0.5), hue(centre, 0.85, 0.5)),
                _ => (hue(centre, 0.7, 0.15), hue(centre, 0.7, 0.8)),
            };
            gpui::linear_gradient(
                90.0,
                gpui::linear_color_stop(from, 0.0),
                gpui::linear_color_stop(to, 1.0),
            )
        }
        _ => rgb(BORDER_STRONG).into(),
    };
    div()
        .relative()
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(3.0))
                .rounded_full()
                .bg(track),
        )
        .child(
            Slider::new(slider)
                .disabled(!spec.supported)
                .bg(gpui::transparent_black())
                .text_color(rgb(TEXT)),
        )
        .into_any_element()
}

/// A label on the left, anything on the right, at the row height.
pub(crate) fn label_row(label: impl Into<SharedString>, right: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .min_h(px(ROW_H))
        .child(
            div()
                .min_w(px(0.0))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(TEXT_DIM))
                .child(label.into()),
        )
        .child(right)
        .into_any_element()
}

/// The bar under a tab's body: its buttons, right-aligned, on a hairline.
pub(crate) fn panel_footer(content: impl IntoElement) -> AnyElement {
    div()
        .flex_none()
        .h(px(48.0))
        .px(px(PAD))
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(8.0))
        .border_t_1()
        .border_color(rgb(HAIRLINE))
        .child(content)
        .into_any_element()
}

/// A text button: GPUI Component's, at the size every inspector footer and
/// row uses. `primary` is the accent fill.
pub(crate) fn panel_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    Button::new(id)
        .small()
        .label(label.into())
        .when(primary, |button| button.primary())
        .disabled(!enabled)
        .on_click(on_click)
}
