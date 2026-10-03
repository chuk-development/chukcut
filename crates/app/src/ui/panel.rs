//! A panel: the rounded graphite surface every region of the editor sits
//! on, with an optional header that holds a title or tabs and actions.

use std::rc::Rc;

use gpui::component::StyledExt as _;
use gpui::prelude::*;
use gpui::{
    div, px, rgb, AnyElement, App, ElementId, FontWeight, SharedString, StyleRefinement, Window,
};

use crate::theme::*;

/// A rounded surface one step above the window, outlined by a hairline.
/// Children stack in a column; the header, if any, comes first.
#[derive(IntoElement)]
pub(crate) struct Panel {
    id: ElementId,
    header: Option<AnyElement>,
    children: Vec<AnyElement>,
    style: StyleRefinement,
}

impl Panel {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            header: None,
            children: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    pub(crate) fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }
}

impl ParentElement for Panel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Panel {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Panel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .rounded(px(R_MD))
            .overflow_hidden()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(HAIRLINE))
            .refine_style(&self.style)
            .children(self.header)
            .children(self.children)
    }
}

type OnSelect = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The 40 px bar at the top of a panel: a title or a row of tabs on the
/// left, actions on the right, a hairline under it.
#[derive(IntoElement, Default)]
pub(crate) struct PanelHeader {
    title: Option<SharedString>,
    detail: Option<SharedString>,
    tabs: Option<(SharedString, Vec<SharedString>, usize, OnSelect)>,
    actions: Vec<AnyElement>,
    leading: Vec<AnyElement>,
}

impl PanelHeader {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Dim text after the title, e.g. the timeline a player shows.
    pub(crate) fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Text tabs in place of the title; the selected one is bright and
    /// underlined in the accent.
    pub(crate) fn tabs(
        mut self,
        id: impl Into<SharedString>,
        labels: impl IntoIterator<Item = impl Into<SharedString>>,
        selected: usize,
        on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.tabs = Some((
            id.into(),
            labels.into_iter().map(Into::into).collect(),
            selected,
            Rc::new(on_select),
        ));
        self
    }

    /// Something before the title, e.g. a status dot.
    pub(crate) fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading.push(element.into_any_element());
        self
    }

    /// Something at the right end, in order.
    pub(crate) fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }
}

impl RenderOnce for PanelHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tabs = self.tabs.map(|(id, labels, selected, on_select)| {
            div()
                .h_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(20.0))
                .children(labels.into_iter().enumerate().map(|(index, label)| {
                    let on = index == selected;
                    let on_select = Rc::clone(&on_select);
                    div()
                        .id(SharedString::from(format!("{id}-{index}")))
                        .h_full()
                        .relative()
                        .flex()
                        .items_center()
                        .text_size(px(TEXT_BODY))
                        .when(on, |this| this.font_weight(FontWeight::MEDIUM))
                        .text_color(rgb(if on { TEXT } else { TEXT_DIM }))
                        .cursor_pointer()
                        .hover(|style| style.text_color(rgb(TEXT)))
                        .on_click(move |_, window, cx| on_select(index, window, cx))
                        .child(label)
                        .when(on, |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .bottom_0()
                                    .left_0()
                                    .right_0()
                                    .h(px(2.0))
                                    .rounded_t(px(2.0))
                                    .bg(rgb(ACCENT)),
                            )
                        })
                }))
        });
        div()
            .h(px(PANEL_HEADER_H))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(PAD))
            .border_b_1()
            .border_color(rgb(HAIRLINE))
            .children(self.leading)
            .children(self.title.map(|title| {
                div()
                    .flex_none()
                    .text_size(px(TEXT_BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .child(title)
            }))
            .children(self.detail.map(|detail| {
                div()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(TEXT_BODY))
                    .text_color(rgb(TEXT_MUTED))
                    .child(detail)
            }))
            .children(tabs)
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .children(self.actions),
            )
    }
}
