//! The header of a collapsible group of properties, like CapCut's
//! "Transform", "Blend", "Stabilise": an optional enable checkbox, the title
//! with its caret, an optional note, and a reset at the far right.

use std::rc::Rc;

use gpui::component::checkbox::Checkbox;
use gpui::component::{Disableable as _, StyledExt as _};
use gpui::prelude::*;
use gpui::{
    div, px, rgb, AnyElement, App, ClickEvent, ElementId, FontWeight, SharedString,
    StyleRefinement, Window,
};

use super::button::IconButton;
use super::icons;
use super::OnClick;
use crate::theme::*;

type Handler<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub(crate) struct SectionHeader {
    id: SharedString,
    title: SharedString,
    collapsed: Option<bool>,
    on_toggle: Option<Handler<()>>,
    enabled: Option<bool>,
    on_enable: Option<Handler<bool>>,
    on_reset: Option<OnClick>,
    note: Option<SharedString>,
    disabled: bool,
}

impl SectionHeader {
    pub(crate) fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            collapsed: None,
            on_toggle: None,
            enabled: None,
            on_enable: None,
            on_reset: None,
            note: None,
            disabled: false,
        }
    }

    /// Make it collapsible: a caret after the title, the title toggles.
    pub(crate) fn collapsible(
        mut self,
        collapsed: bool,
        on_toggle: impl Fn((), &mut Window, &mut App) + 'static,
    ) -> Self {
        self.collapsed = Some(collapsed);
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }

    /// An enable checkbox in front of the title. The handler gets the new
    /// state.
    pub(crate) fn enable(
        mut self,
        enabled: bool,
        on_enable: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.enabled = Some(enabled);
        self.on_enable = Some(Rc::new(on_enable));
        self
    }

    /// An enable checkbox that shows a state but cannot be changed here.
    pub(crate) fn checked(mut self, enabled: bool) -> Self {
        self.enabled = Some(enabled);
        self.on_enable = None;
        self
    }

    pub(crate) fn on_reset(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_reset = Some(Rc::new(handler));
        self
    }

    /// Dim text before the reset, e.g. "Not in the engine yet".
    pub(crate) fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Greyed: nothing in it can be used.
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let id = self.id;
        let disabled = self.disabled;
        let checkbox = self.enabled.map(|enabled| {
            let on_enable = self.on_enable.clone();
            Checkbox::new(SharedString::from(format!("{id}-enable")))
                .checked(enabled)
                .disabled(disabled || on_enable.is_none())
                .on_click(move |checked: &bool, window, cx| {
                    if let Some(on_enable) = &on_enable {
                        on_enable(*checked, window, cx);
                    }
                })
        });
        let caret = self.collapsed.map(|collapsed| {
            icons::glyph(
                if collapsed {
                    icons::CHEVRON_RIGHT
                } else {
                    icons::CHEVRON_DOWN
                },
                14.0,
                rgb(TEXT_MUTED),
            )
        });
        let on_toggle = self.on_toggle;
        let title = div()
            .id(SharedString::from(format!("{id}-title")))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .text_size(px(TEXT_BODY))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(if disabled { TEXT_MUTED } else { TEXT }))
            .when_some(on_toggle, |this, on_toggle| {
                this.cursor_pointer()
                    .on_click(move |_, window, cx| on_toggle((), window, cx))
            })
            .child(self.title)
            .children(caret);
        div()
            .h(px(36.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .children(checkbox)
            .child(title)
            .child(div().flex_1())
            .children(self.note.map(|note| {
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(note)
            }))
            .children(self.on_reset.map(|on_reset| {
                IconButton::new(SharedString::from(format!("{id}-reset")), icons::RESET)
                    .small()
                    .disabled(disabled)
                    .tooltip("Reset")
                    .on_click(move |event, window, cx| on_reset(event, window, cx))
            }))
    }
}

/// A section: its header, then its rows, with a hairline under it.
#[derive(IntoElement)]
pub(crate) struct Section {
    id: ElementId,
    header: SectionHeader,
    collapsed: bool,
    rows: Vec<AnyElement>,
    style: StyleRefinement,
}

impl Section {
    pub(crate) fn new(id: impl Into<ElementId>, header: SectionHeader) -> Self {
        let collapsed = header.collapsed.unwrap_or(false);
        Self {
            id: id.into(),
            header,
            collapsed,
            rows: Vec::new(),
            style: StyleRefinement::default(),
        }
    }
}

impl ParentElement for Section {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.rows.extend(elements);
    }
}

impl Styled for Section {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Section {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let has_rows = !self.rows.is_empty();
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .pb(px(8.0))
            .border_b_1()
            .border_color(rgb(HAIRLINE))
            .refine_style(&self.style)
            .child(self.header)
            .when(!self.collapsed && has_rows, |this| {
                this.child(div().flex().flex_col().gap(px(6.0)).children(self.rows))
            })
    }
}
