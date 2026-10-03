//! A number field: GPUI Component's text input in our sunken well, a mono
//! value, an optional axis prefix and unit suffix, and a stacked stepper.
//!
//! The field does not parse; its owner reads the `InputState` on change and
//! decides what a step means, exactly as with a plain input.

use std::rc::Rc;

use gpui::component::input::{Input, InputState};
use gpui::component::Sizable as _;
use gpui::prelude::*;
use gpui::{div, px, rgb, App, Entity, Focusable as _, SharedString, Window};

use super::icons;
use crate::theme::*;

type OnStep = Rc<dyn Fn(f32, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub(crate) struct NumberField {
    id: SharedString,
    state: Entity<InputState>,
    width: f32,
    prefix: Option<SharedString>,
    suffix: Option<SharedString>,
    disabled: bool,
    on_step: Option<OnStep>,
}

impl NumberField {
    pub(crate) fn new(id: impl Into<SharedString>, state: &Entity<InputState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            width: 76.0,
            prefix: None,
            suffix: None,
            disabled: false,
            on_step: None,
        }
    }

    /// Total width including the stepper.
    pub(crate) fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// A short dim label inside the well, before the value ("X", "Y").
    pub(crate) fn prefix(mut self, prefix: impl Into<SharedString>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// The unit after the value ("%", "°", "s").
    pub(crate) fn suffix(mut self, suffix: impl Into<SharedString>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The stepper: called with `1.0` for up, `-1.0` for down.
    pub(crate) fn on_step(
        mut self,
        handler: impl Fn(f32, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_step = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for NumberField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let enabled = !self.disabled;
        let focused = enabled && self.state.read(cx).focus_handle(cx).is_focused(window);
        let id = self.id;
        let step = |up: bool, on_step: Option<OnStep>| {
            let group = SharedString::from(format!("{id}-step-{up}"));
            div()
                .id(group.clone())
                .group(group.clone())
                .flex_1()
                .w(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .when(enabled && on_step.is_some(), |this| {
                    this.cursor_pointer()
                        .hover(|style| style.bg(rgb(PANEL_RAISED)))
                })
                .when_some(on_step.filter(|_| enabled), |this, on_step| {
                    this.on_click(move |_, window, cx| {
                        on_step(if up { 1.0 } else { -1.0 }, window, cx)
                    })
                })
                .child(
                    icons::glyph(
                        if up {
                            icons::CHEVRON_UP
                        } else {
                            icons::CHEVRON_DOWN
                        },
                        11.0,
                        rgb(if enabled { TEXT_MUTED } else { TEXT_DISABLED }),
                    )
                    .group_hover(group, |style| style.text_color(rgb(TEXT))),
                )
        };
        let has_stepper = self.on_step.is_some();
        div()
            .w(px(self.width))
            .flex_none()
            .h(px(CONTROL_H))
            .flex()
            .flex_row()
            .items_center()
            .rounded(px(R_SM))
            .overflow_hidden()
            .bg(rgb(WELL))
            .border_1()
            .border_color(rgb(if focused { ACCENT } else { BORDER }))
            .when(enabled && !focused, |this| {
                this.hover(|style| style.border_color(rgb(BORDER_STRONG)))
            })
            .children(self.prefix.map(|prefix| {
                div()
                    .pl(px(8.0))
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(prefix)
            }))
            .child(
                div().flex_1().min_w(px(0.0)).child(
                    Input::new(&self.state)
                        .appearance(false)
                        .xsmall()
                        .disabled(!enabled)
                        .font_family(FONT_MONO)
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(if enabled { TEXT } else { TEXT_DISABLED })),
                ),
            )
            .children(self.suffix.map(|suffix| {
                div()
                    .pr(px(6.0))
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(suffix)
            }))
            .when(has_stepper, |this| {
                this.child(
                    div()
                        .h_full()
                        .flex()
                        .flex_col()
                        .border_l_1()
                        .border_color(rgb(HAIRLINE))
                        .child(step(true, self.on_step.clone()))
                        .child(step(false, self.on_step.clone())),
                )
            })
    }
}
