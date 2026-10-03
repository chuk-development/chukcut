//! Tab controls: the segmented switch inside panels, and the icon rail
//! across the top of the asset panel.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{div, px, rgb, App, ClickEvent, FontWeight, SharedString, Window};

use super::icons::IconSrc;
use super::OnClick;
use crate::theme::*;

type OnSelect = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A row of mutually exclusive options in a sunken well; the selected one
/// is a raised chip. Inspector sub-tabs, export presets, quality pickers.
#[derive(IntoElement)]
pub(crate) struct SegmentedTabs {
    id: SharedString,
    labels: Vec<SharedString>,
    selected: usize,
    fill: bool,
    on_select: Option<OnSelect>,
}

impl SegmentedTabs {
    pub(crate) fn new(
        id: impl Into<SharedString>,
        labels: impl IntoIterator<Item = impl Into<SharedString>>,
        selected: usize,
    ) -> Self {
        Self {
            id: id.into(),
            labels: labels.into_iter().map(Into::into).collect(),
            selected,
            fill: true,
            on_select: None,
        }
    }

    /// Size each segment to its label instead of sharing the width.
    pub(crate) fn hug(mut self) -> Self {
        self.fill = false;
        self
    }

    pub(crate) fn on_select(
        mut self,
        handler: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for SegmentedTabs {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let fill = self.fill;
        div()
            .flex()
            .flex_row()
            .when(!fill, |this| this.flex_none())
            .p(px(2.0))
            .gap(px(2.0))
            .rounded(px(R_SM + 1.0))
            .bg(rgb(WELL))
            .border_1()
            .border_color(rgb(HAIRLINE))
            .children(self.labels.into_iter().enumerate().map(|(index, label)| {
                let on = index == self.selected;
                let on_select = self.on_select.clone();
                div()
                    .id(SharedString::from(format!("{}-{index}", self.id)))
                    .when(fill, |this| this.flex_1())
                    .h(px(24.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(R_SM - 1.0))
                    .text_size(px(TEXT_LABEL))
                    .whitespace_nowrap()
                    .when(on, |this| {
                        this.bg(rgb(OVERLAY))
                            .text_color(rgb(TEXT))
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!on, |this| {
                        this.text_color(rgb(TEXT_DIM))
                            .cursor_pointer()
                            .hover(|style| style.text_color(rgb(TEXT)).bg(rgb(PANEL)))
                    })
                    .when_some(on_select, |this, on_select| {
                        this.on_click(move |_, window, cx| on_select(index, window, cx))
                    })
                    .child(label)
            }))
    }
}

/// One tab of the icon rail: a glyph over a short label.
#[derive(IntoElement)]
pub(crate) struct RailTab {
    id: SharedString,
    icon: IconSrc,
    label: SharedString,
    selected: bool,
    on_click: Option<OnClick>,
}

impl RailTab {
    pub(crate) fn new(
        id: impl Into<SharedString>,
        icon: impl Into<IconSrc>,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            icon: icon.into(),
            label: label.into(),
            selected: false,
            on_click: None,
        }
    }

    pub(crate) fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for RailTab {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let on = self.selected;
        let group = SharedString::from(format!("rail-{}", self.id));
        div()
            .id(self.id)
            .group(group.clone())
            .min_w(px(56.0))
            .h(px(48.0))
            .px(px(6.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .rounded(px(R_SM))
            .cursor_pointer()
            .when(on, |this| this.bg(accent_soft()))
            .when(!on, |this| this.hover(|style| style.bg(rgb(PANEL_RAISED))))
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .child(
                self.icon
                    .svg(20.0, rgb(if on { ACCENT } else { TEXT_DIM }))
                    .group_hover(group.clone(), move |style| {
                        style.text_color(rgb(if on { ACCENT } else { TEXT }))
                    }),
            )
            .child(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .when(on, |this| this.font_weight(FontWeight::MEDIUM))
                    .text_color(rgb(if on { TEXT } else { TEXT_DIM }))
                    .group_hover(group, |style| style.text_color(rgb(TEXT)))
                    .child(self.label),
            )
    }
}
