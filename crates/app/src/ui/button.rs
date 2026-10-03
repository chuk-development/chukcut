//! The square icon button every toolbar and header uses.

use std::rc::Rc;

use gpui::component::kbd::Kbd;
use gpui::component::menu::DropdownMenu;
use gpui::component::tooltip::Tooltip;
use gpui::component::Selectable;
use gpui::prelude::*;
use gpui::{
    div, px, rgb, App, ClickEvent, Div, ElementId, Interactivity, Keystroke, SharedString,
    Stateful, StyleRefinement, Window,
};

use super::icons::IconSrc;
use super::OnClick;
use crate::theme::*;

/// Box and glyph size of an [`IconButton`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum IconSize {
    /// 24 box, 14 glyph: inside rows and fields.
    Small,
    /// 28 box, 16 glyph: toolbars and panel headers.
    #[default]
    Medium,
    /// 36 box, 20 glyph: the play button.
    Large,
}

impl IconSize {
    fn box_px(self) -> f32 {
        match self {
            IconSize::Small => 24.0,
            IconSize::Medium => 28.0,
            IconSize::Large => 36.0,
        }
    }

    fn glyph_px(self) -> f32 {
        match self {
            IconSize::Small => 14.0,
            IconSize::Medium => 16.0,
            IconSize::Large => 20.0,
        }
    }
}

/// A square, borderless icon button: dim at rest, lighter on hover, accent
/// on a soft accent fill when toggled on. The tooltip shows its shortcut.
/// It can open a GPUI Component dropdown menu (`.dropdown_menu(…)`), which
/// draws it toggled while the menu is open.
#[derive(IntoElement)]
pub(crate) struct IconButton {
    base: Stateful<Div>,
    id: ElementId,
    icon: IconSrc,
    size: IconSize,
    toggled: bool,
    disabled: bool,
    /// The glyph's colour at rest, instead of `TEXT_DIM`.
    tint: Option<u32>,
    tooltip: Option<SharedString>,
    shortcut: Option<Keystroke>,
    on_click: Option<OnClick>,
}

impl IconButton {
    pub(crate) fn new(id: impl Into<ElementId>, icon: impl Into<IconSrc>) -> Self {
        let id = id.into();
        Self {
            base: div().id(id.clone()),
            id,
            icon: icon.into(),
            size: IconSize::default(),
            toggled: false,
            disabled: false,
            tint: None,
            tooltip: None,
            shortcut: None,
            on_click: None,
        }
    }

    pub(crate) fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    pub(crate) fn small(self) -> Self {
        self.size(IconSize::Small)
    }

    pub(crate) fn large(self) -> Self {
        self.size(IconSize::Large)
    }

    /// Draw it as switched on: a soft accent fill and an accent glyph.
    pub(crate) fn toggled(mut self, toggled: bool) -> Self {
        self.toggled = toggled;
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The glyph's colour at rest.
    pub(crate) fn tint(mut self, color: u32) -> Self {
        self.tint = Some(color);
        self
    }

    pub(crate) fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// The shortcut shown in the tooltip, in GPUI's notation (`ctrl-b`).
    /// Display only: binding the key is the caller's job.
    pub(crate) fn shortcut(mut self, keys: &str) -> Self {
        self.shortcut = Keystroke::parse(keys).ok();
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

impl RenderOnce for IconButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let group = SharedString::from(format!("icon-button-{:?}", self.id));
        let (rest, hover) = match (self.disabled, self.toggled) {
            (true, _) => (TEXT_DISABLED, TEXT_DISABLED),
            (false, true) => (ACCENT, ACCENT_HOVER),
            (false, false) => (self.tint.unwrap_or(TEXT_DIM), TEXT),
        };
        let size = self.size;
        let tooltip = self.tooltip.clone();
        let shortcut = self.shortcut.clone();
        self.base
            .group(group.clone())
            .size(px(size.box_px()))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(R_SM))
            .when(self.toggled && !self.disabled, |this| {
                this.bg(accent_soft())
            })
            .when(!self.disabled, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(rgb(PANEL_RAISED)))
                    .active(|style| style.bg(rgb(OVERLAY)))
            })
            .when_some(
                self.on_click.filter(|_| !self.disabled),
                |this, on_click| {
                    this.on_click(move |event, window, cx| on_click(event, window, cx))
                },
            )
            .when_some(tooltip, |this, text| {
                this.tooltip(move |window, cx| {
                    Tooltip::new(text.clone())
                        .key_binding(shortcut.clone().map(Kbd::new))
                        .build(window, cx)
                })
            })
            .child(
                self.icon
                    .svg(size.glyph_px(), rgb(rest))
                    .group_hover(group, move |style| style.text_color(rgb(hover))),
            )
    }
}

impl Styled for IconButton {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for IconButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl Selectable for IconButton {
    fn selected(self, selected: bool) -> Self {
        self.toggled(selected)
    }

    fn is_selected(&self) -> bool {
        self.toggled
    }
}

impl DropdownMenu for IconButton {}
