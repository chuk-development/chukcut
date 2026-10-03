//! The inspector's building blocks: tabs, sections, property rows, the number
//! box with its stepper, and the reset and keyframe buttons. Drawn here rather
//! than taken whole from GPUI Component so they keep CapCut's proportions; the
//! slider, the text field and the switch are the component ones.

use gpui::component::input::Input;
use gpui::component::slider::Slider;
use gpui::component::{Icon, Sizable};
use gpui::{AnyElement, ClickEvent};

use super::*;

/// Our own line icons, drawn as SVG and tinted by the text colour.
pub(crate) mod icons {
    macro_rules! icon {
        ($name:ident, $body:literal) => {
            pub const $name: &[u8] = concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</svg>"
            )
            .as_bytes();
        };
    }
    icon!(
        RESET,
        r#"<path d="M5 13a7 7 0 1 0 2-5.3"/><path d="M5 4v4.5h4.5"/>"#
    );
    icon!(DIAMOND, r#"<path d="M12 5l7 7-7 7-7-7z"/>"#);
    icon!(
        DIAMOND_FILLED,
        r#"<path d="M12 5l7 7-7 7-7-7z" fill="black"/>"#
    );
    icon!(PREV, r#"<path d="M14 7l-5 5 5 5"/>"#);
    icon!(NEXT, r#"<path d="M10 7l5 5-5 5"/>"#);
    icon!(UP, r#"<path d="M7 14l5-5 5 5"/>"#);
    icon!(DOWN, r#"<path d="M7 10l5 5 5-5"/>"#);
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
pub(crate) const DISABLED: u32 = 0x5c5c5c;
/// The number box's well.
const FIELD_BG: u32 = 0x1d1d1d;
/// Height of a property row's controls.
const ROW_H: f32 = 26.0;

/// A small square icon button.
pub(crate) fn icon_button(
    id: impl Into<gpui::ElementId>,
    data: &'static [u8],
    color: u32,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(rgb(PANEL_RAISED)))
                .on_click(on_click)
        })
        .child(icon(data, 14.0, if enabled { color } else { DISABLED }))
}

/// CapCut's top tab row: plain labels, the active one in the accent colour.
pub(crate) fn top_tabs(
    tabs: &[&'static str],
    active: &'static str,
    cx: &mut Context<Editor>,
) -> impl IntoElement {
    div()
        .h(px(40.0))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(22.0))
        .px_3()
        .border_b_1()
        .border_color(rgb(BG))
        .children(tabs.iter().map(|&tab| {
            let selected = tab == active;
            div()
                .id(SharedString::from(format!("top-tab-{tab}")))
                .h_full()
                .flex()
                .items_center()
                .text_sm()
                .when(selected, |this| this.font_weight(gpui::FontWeight::MEDIUM))
                .text_color(rgb(if selected { ACCENT } else { TEXT }))
                .cursor_pointer()
                .hover(|style| style.text_color(rgb(if selected { ACCENT } else { 0xffffff })))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.inspector.tab = Some(tab);
                    cx.notify();
                }))
                .child(tab)
        }))
}

/// The segmented control under the top tabs.
pub(crate) fn sub_tabs(
    owner: &'static str,
    tabs: &[&'static str],
    active: &'static str,
    cx: &mut Context<Editor>,
) -> impl IntoElement {
    div().flex_none().px_3().pt_3().pb_2().child(
        div()
            .flex()
            .flex_row()
            .p(px(2.0))
            .rounded(px(5.0))
            .bg(rgb(0x1f1f1f))
            .children(tabs.iter().map(|&tab| {
                let selected = tab == active;
                div()
                    .id(SharedString::from(format!("sub-tab-{owner}-{tab}")))
                    .flex_1()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .text_xs()
                    .text_color(rgb(if selected { TEXT } else { TEXT_DIM }))
                    .when(selected, |this| this.bg(rgb(PANEL_RAISED)))
                    .cursor_pointer()
                    .hover(|style| style.text_color(rgb(TEXT)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.inspector.sub_tab.insert(owner, tab);
                        cx.notify();
                    }))
                    .child(tab)
            })),
    )
}

/// A section: header with a collapse caret, an optional enable checkbox and
/// reset, then its rows.
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
        let enabled = self.enabled;
        let has_rows = !rows.is_empty();
        let mut header = div().h(px(36.0)).flex().flex_row().items_center().gap_2();
        if let Some(checked) = self.checkbox {
            let on_check = self.on_check.map(std::rc::Rc::new);
            header = header.child(check_box(
                SharedString::from(format!("check-{title}")),
                checked,
                enabled && on_check.is_some(),
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(on_check) = &on_check {
                        on_check(this, !checked, cx);
                    }
                }),
            ));
        }
        header = header.child(
            div()
                .id(SharedString::from(format!("section-{title}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .text_sm()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(rgb(if enabled { TEXT } else { TEXT_DIM }))
                .when(has_rows, |this| {
                    this.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.inspector.collapsed.remove(title) {
                                this.inspector.collapsed.insert(title);
                            }
                            cx.notify();
                        }))
                })
                .child(title)
                .when(has_rows, |this| {
                    this.child(icon(
                        if collapsed { icons::DOWN } else { icons::UP },
                        12.0,
                        TEXT_DIM,
                    ))
                }),
        );
        header = header.child(div().flex_1());
        if let Some(note) = self.note {
            header = header.child(div().text_xs().text_color(rgb(DISABLED)).child(note));
        }
        if let Some(on_reset) = self.on_reset {
            let on_reset = std::rc::Rc::new(on_reset);
            header = header.child(icon_button(
                SharedString::from(format!("reset-{title}")),
                icons::RESET,
                TEXT_DIM,
                enabled,
                cx.listener(move |this, _, _, cx| on_reset(this, cx)),
            ));
        }

        div()
            .flex()
            .flex_col()
            .px_3()
            .pb_2()
            .border_b_1()
            .border_color(rgb(0x2f2f2f))
            .child(header)
            .when(!collapsed, |this| {
                this.child(div().flex().flex_col().gap(px(10.0)).pb_1().children(rows))
            })
            .into_any_element()
    }
}

pub(crate) fn check_box(
    id: SharedString,
    checked: bool,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .size(px(14.0))
        .flex_none()
        .rounded(px(3.0))
        .border_1()
        .border_color(rgb(if checked && enabled { ACCENT } else { 0x5a5a5a }))
        .when(checked, |this| this.bg(rgb(if enabled { ACCENT } else { 0x4a4a4a })))
        .flex()
        .items_center()
        .justify_center()
        .when(checked, |this| {
            this.child(
                Icon::default()
                    .data(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>"#)
                    .size(px(11.0))
                    .text_color(rgb(0x0b1214)),
            )
        })
        .when(enabled, |this| this.cursor_pointer().on_click(on_click))
}

/// A sub-heading inside a section ("Colour", "Light", "Effects").
pub(crate) fn group_label(label: &'static str) -> AnyElement {
    div()
        .pt_1()
        .pb_1()
        .border_b_1()
        .border_color(rgb(0x2f2f2f))
        .text_xs()
        .text_color(rgb(TEXT_DIM))
        .child(label)
        .into_any_element()
}

/// Shown in place of a tab the engine has nothing behind yet.
pub(crate) fn not_yet(what: &str) -> AnyElement {
    div()
        .p_6()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(rgb(TEXT_DIM))
                .child(what.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(DISABLED))
                .child("Not in the engine yet."),
        )
        .into_any_element()
}

impl Editor {
    /// The boxed number field with CapCut's stacked stepper on its right.
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
        let enabled = spec.supported;
        let stepper = |up: bool, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(format!(
                    "step-{prop:?}-{}",
                    if up { "up" } else { "down" }
                )))
                .flex_1()
                .w(px(14.0))
                .flex()
                .items_center()
                .justify_center()
                .when(enabled, |this| {
                    this.cursor_pointer()
                        .hover(|style| style.bg(rgb(BORDER)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.step_prop(prop, if up { 1.0 } else { -1.0 }, cx)
                        }))
                })
                .child(icon(
                    if up { icons::UP } else { icons::DOWN },
                    10.0,
                    if enabled { TEXT_DIM } else { DISABLED },
                ))
        };
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .when_some(prefix, |this, prefix| {
                this.child(div().text_xs().text_color(rgb(TEXT_DIM)).child(prefix))
            })
            .child(
                div()
                    .w(px(width))
                    .h(px(ROW_H))
                    .flex()
                    .flex_row()
                    .items_center()
                    .rounded(px(4.0))
                    .bg(rgb(FIELD_BG))
                    .border_1()
                    .border_color(rgb(0x333333))
                    .child(
                        div().flex_1().min_w(px(0.0)).child(
                            Input::new(&input)
                                .appearance(false)
                                .xsmall()
                                .disabled(!enabled)
                                .text_xs()
                                .text_color(rgb(if enabled { TEXT } else { DISABLED })),
                        ),
                    )
                    .when(!spec.suffix.is_empty(), |this| {
                        this.child(
                            div()
                                .pr_1()
                                .text_xs()
                                .text_color(rgb(if enabled { TEXT } else { DISABLED }))
                                .child(spec.suffix),
                        )
                    }),
            )
            .child(
                div()
                    .h(px(ROW_H))
                    .flex()
                    .flex_col()
                    .rounded(px(3.0))
                    .bg(rgb(FIELD_BG))
                    .child(stepper(true, cx))
                    .child(stepper(false, cx)),
            )
            .into_any_element()
    }

    /// The reset and keyframe buttons at the right of a row. Rows without a
    /// keyframeable property keep the space, so every column lines up.
    pub(crate) fn row_actions(
        &self,
        prop: Prop,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let spec = prop.spec();
        let animatable = !prop.animated().is_empty();
        let (animated, at) = self.keyframe_state(prop, segment);
        let diamond_color = if at || animated { ACCENT } else { TEXT_DIM };
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .child(icon_button(
                SharedString::from(format!("reset-{prop:?}")),
                icons::RESET,
                TEXT_DIM,
                spec.supported,
                cx.listener(move |this, _, _, cx| this.reset_prop(prop, cx)),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .when(!animatable, |this| this.invisible())
                    .child(icon_button(
                        SharedString::from(format!("kf-prev-{prop:?}")),
                        icons::PREV,
                        TEXT_DIM,
                        animated,
                        cx.listener(move |this, _, _, cx| this.jump_keyframe(prop, false, cx)),
                    ))
                    .child(icon_button(
                        SharedString::from(format!("kf-{prop:?}")),
                        if at {
                            icons::DIAMOND_FILLED
                        } else {
                            icons::DIAMOND
                        },
                        diamond_color,
                        animatable && spec.supported,
                        cx.listener(move |this, _, _, cx| this.toggle_keyframe(prop, cx)),
                    ))
                    .child(icon_button(
                        SharedString::from(format!("kf-next-{prop:?}")),
                        icons::NEXT,
                        TEXT_DIM,
                        animated,
                        cx.listener(move |this, _, _, cx| this.jump_keyframe(prop, true, cx)),
                    )),
            )
    }

    /// CapCut's slider row: the label above, then slider · number · actions.
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
        let actions = self.row_actions(prop, segment, cx);
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(if spec.supported { TEXT } else { TEXT_DIM }))
                    .child(spec.label),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().px_1().when_some(slider, |this, slider| {
                        this.child(slider_with_track(prop, &slider))
                    }))
                    .child(number)
                    .child(actions),
            )
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
            boxes.push(self.number_box(prop, value, 72.0, prefix, window, cx));
        }
        let actions = self.row_actions(props[0].0, segment, cx);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(110.0))
                    .text_xs()
                    .text_color(rgb(TEXT))
                    .child(label),
            )
            .child(div().flex_1().flex().flex_row().gap_4().children(boxes))
            .child(actions)
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
        _ if !spec.supported => rgb(0x3a3a3a).into(),
        Prop::Temperature => strip(0x3d6bff, 0xf2d33a),
        Prop::Tint => strip(0x3fc24f, 0xd84ad8),
        Prop::Saturation => strip(0x8a8a8a, 0xe23b3b),
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
        _ => rgb(0x555555).into(),
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
                .text_color(rgb(0xf0f0f0)),
        )
        .into_any_element()
}

/// A label on the left, anything on the right.
pub(crate) fn label_row(label: impl Into<SharedString>, right: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .min_h(px(ROW_H))
        .child(div().text_xs().text_color(rgb(TEXT)).child(label.into()))
        .child(right)
        .into_any_element()
}

/// A flat button in the panel's style. `primary` is the accent fill.
pub(crate) fn panel_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (bg, fg, hover) = match (primary, enabled) {
        (_, false) => (PANEL_RAISED, DISABLED, PANEL_RAISED),
        (true, true) => (ACCENT, 0x0b1214, ACCENT_HOVER),
        (false, true) => (0x3a3a3a, TEXT, 0x454545),
    };
    div()
        .id(id.into())
        .h(px(26.0))
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(bg))
        .text_xs()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(fg))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |style| style.bg(rgb(hover)))
                .on_click(on_click)
        })
        .child(label.into())
}
