//! What a region shows when it has nothing yet: a glyph in a soft disc, a
//! title that says what to do, a hint, and the one action that does it.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    div, px, rgb, AnyElement, App, ClickEvent, ExternalPaths, FontWeight, SharedString, Window,
};

use super::icons::IconSrc;
use super::OnClick;
use crate::theme::*;

#[derive(IntoElement)]
pub(crate) struct EmptyState {
    id: SharedString,
    icon: IconSrc,
    title: SharedString,
    hint: Option<SharedString>,
    action: Option<AnyElement>,
    /// Draw it as a drop zone: a well with a border that lights up while
    /// files are dragged over it, clickable as a whole.
    drop_zone: bool,
    on_click: Option<OnClick>,
}

impl EmptyState {
    pub(crate) fn new(
        id: impl Into<SharedString>,
        icon: impl Into<IconSrc>,
        title: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            icon: icon.into(),
            title: title.into(),
            hint: None,
            action: None,
            drop_zone: false,
            on_click: None,
        }
    }

    pub(crate) fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub(crate) fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// A drop zone for files from the desktop, clickable as a whole.
    pub(crate) fn drop_zone(
        mut self,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.drop_zone = true;
        self.on_click = Some(Rc::new(on_click));
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let group = SharedString::from(format!("{}-group", self.id));
        let zone = self.drop_zone;
        div()
            .id(self.id)
            .group(group.clone())
            .flex_1()
            .min_h(px(160.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .p(px(24.0))
            .when(zone, |this| {
                this.rounded(px(R_MD))
                    .bg(rgb(WELL))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .cursor_pointer()
                    .hover(|style| style.border_color(rgb(BORDER_STRONG)))
                    .drag_over::<ExternalPaths>(|style, _, _, _| {
                        style.border_color(rgb(ACCENT)).bg(accent_drop())
                    })
            })
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .child(
                div()
                    .size(px(44.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if zone {
                        accent_soft()
                    } else {
                        hsla(PANEL_RAISED)
                    })
                    .child(
                        self.icon
                            .svg(22.0, rgb(if zone { ACCENT } else { TEXT_DIM })),
                    ),
            )
            .child(
                div()
                    .text_size(px(TEXT_DISPLAY - 1.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT))
                    .child(self.title),
            )
            .children(self.hint.map(|hint| {
                div()
                    .max_w(px(320.0))
                    .text_center()
                    .text_size(px(TEXT_CAPTION + 1.0))
                    .text_color(rgb(TEXT_MUTED))
                    .child(hint)
            }))
            .children(self.action)
    }
}
