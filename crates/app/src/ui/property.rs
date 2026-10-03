//! A property row: label, control, and the fixed action column at the far
//! right (reset, then the keyframe slot), so every row's actions line up.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{div, px, rgb, AnyElement, App, ClickEvent, SharedString, Window};

use super::icons::{self, Glyph};
use super::OnClick;
use crate::theme::*;

/// Width of the label column of an inline row.
pub(crate) const LABEL_W: f32 = 112.0;
const ACTION: f32 = 20.0;

/// Where the playhead stands relative to a property's keyframes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum KeyMark {
    /// Not animated.
    #[default]
    None,
    /// Animated, but no keyframe under the playhead.
    Animated,
    /// A keyframe sits under the playhead.
    OnKey,
}

/// The keyframe controls of a row: previous, the diamond, next.
#[derive(IntoElement)]
pub(crate) struct KeyframeSlot {
    id: SharedString,
    mark: KeyMark,
    enabled: bool,
    on_toggle: Option<OnClick>,
    on_prev: Option<OnClick>,
    on_next: Option<OnClick>,
}

impl KeyframeSlot {
    pub(crate) fn new(id: impl Into<SharedString>, mark: KeyMark) -> Self {
        Self {
            id: id.into(),
            mark,
            enabled: true,
            on_toggle: None,
            on_prev: None,
            on_next: None,
        }
    }

    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub(crate) fn on_toggle(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    pub(crate) fn on_prev(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_prev = Some(Rc::new(handler));
        self
    }

    pub(crate) fn on_next(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_next = Some(Rc::new(handler));
        self
    }
}

/// A 20 px action glyph: dim, brighter on hover, inert when disabled.
fn action(
    id: SharedString,
    glyph: Glyph,
    color: u32,
    enabled: bool,
    on_click: Option<OnClick>,
) -> impl IntoElement {
    let group = SharedString::from(format!("{id}-group"));
    let hover = if color == TEXT_DIM { TEXT } else { color };
    div()
        .id(id)
        .group(group.clone())
        .size(px(ACTION))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(R_XS))
        .when(enabled && on_click.is_some(), |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(rgb(PANEL_RAISED)))
        })
        .when_some(on_click.filter(|_| enabled), |this, on_click| {
            this.on_click(move |event, window, cx| on_click(event, window, cx))
        })
        .child(
            icons::glyph(
                glyph,
                13.0,
                rgb(if enabled { color } else { TEXT_DISABLED }),
            )
            .when(enabled, |svg| {
                svg.group_hover(group, move |style| style.text_color(rgb(hover)))
            }),
        )
}

impl RenderOnce for KeyframeSlot {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let id = self.id;
        let animated = self.mark != KeyMark::None;
        let (diamond, color) = match self.mark {
            KeyMark::None => (icons::DIAMOND, TEXT_DIM),
            KeyMark::Animated => (icons::DIAMOND, ACCENT),
            KeyMark::OnKey => (icons::DIAMOND_FILLED, ACCENT),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .child(action(
                format!("{id}-prev").into(),
                icons::CHEVRON_LEFT,
                TEXT_DIM,
                self.enabled && animated,
                self.on_prev,
            ))
            .child(action(
                format!("{id}-key").into(),
                diamond,
                color,
                self.enabled,
                self.on_toggle,
            ))
            .child(action(
                format!("{id}-next").into(),
                icons::CHEVRON_RIGHT,
                TEXT_DIM,
                self.enabled && animated,
                self.on_next,
            ))
    }
}

/// One property: its label, its controls, a reset and a keyframe slot.
#[derive(IntoElement)]
pub(crate) struct PropertyRow {
    id: SharedString,
    label: SharedString,
    stacked: bool,
    disabled: bool,
    controls: Vec<AnyElement>,
    on_reset: Option<OnClick>,
    keyframe: Option<KeyframeSlot>,
    /// Keep the action column's width even when this row has no actions.
    reserve_actions: bool,
}

impl PropertyRow {
    pub(crate) fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            stacked: false,
            disabled: false,
            controls: Vec::new(),
            on_reset: None,
            keyframe: None,
            reserve_actions: true,
        }
    }

    /// The label above the controls, for sliders that want the full width.
    pub(crate) fn stacked(mut self) -> Self {
        self.stacked = true;
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub(crate) fn on_reset(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_reset = Some(Rc::new(handler));
        self
    }

    pub(crate) fn keyframe(mut self, slot: KeyframeSlot) -> Self {
        self.keyframe = Some(slot);
        self
    }

    /// Drop the action column entirely (dialogs, read-only details).
    pub(crate) fn no_actions(mut self) -> Self {
        self.reserve_actions = false;
        self
    }
}

impl ParentElement for PropertyRow {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.controls.extend(elements);
    }
}

impl RenderOnce for PropertyRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let id = self.id;
        let enabled = !self.disabled;
        let label = div()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(if enabled { TEXT_DIM } else { TEXT_DISABLED }))
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(self.label);
        let has_reset = self.on_reset.is_some();
        let actions = (self.reserve_actions || has_reset || self.keyframe.is_some()).then(|| {
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .child(
                    div()
                        .when(!has_reset, |this| this.invisible())
                        .child(action(
                            format!("{id}-reset").into(),
                            icons::RESET,
                            TEXT_DIM,
                            enabled,
                            self.on_reset,
                        )),
                )
                .child(match self.keyframe {
                    Some(slot) => slot.enabled(enabled).into_any_element(),
                    None => div().w(px(ACTION * 3.0)).into_any_element(),
                })
        });
        let controls = div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .children(self.controls);
        if self.stacked {
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(label)
                .child(
                    div()
                        .min_h(px(ROW_H))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .child(controls)
                        .children(actions),
                )
                .into_any_element()
        } else {
            div()
                .min_h(px(ROW_H))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .child(div().w(px(LABEL_W)).flex_none().child(label))
                .child(controls)
                .children(actions)
                .into_any_element()
        }
    }
}
