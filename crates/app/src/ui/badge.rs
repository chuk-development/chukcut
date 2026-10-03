//! A small label: a duration on a tile, "Added", "GPU", a count.

use gpui::prelude::*;
use gpui::{div, px, rgb, App, FontWeight, Hsla, SharedString, Window};

use super::icons::IconSrc;
use crate::theme::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    /// Light text on a dark scrim, for badges laid over a picture.
    OnMedia,
}

impl Tone {
    fn colors(self) -> (Hsla, u32) {
        match self {
            Tone::Neutral => (hsla(OVERLAY), TEXT_DIM),
            Tone::Accent => (with_alpha(ACCENT, 0.16), ACCENT),
            Tone::Success => (with_alpha(SUCCESS, 0.16), SUCCESS),
            Tone::Warning => (with_alpha(WARNING, 0.16), WARNING),
            Tone::Danger => (with_alpha(DANGER, 0.16), DANGER),
            Tone::OnMedia => (scrim(0.62), TEXT),
        }
    }
}

#[derive(IntoElement)]
pub(crate) struct Badge {
    text: SharedString,
    tone: Tone,
    icon: Option<IconSrc>,
    mono: bool,
}

impl Badge {
    pub(crate) fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::default(),
            icon: None,
            mono: false,
        }
    }

    pub(crate) fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub(crate) fn icon(mut self, icon: impl Into<IconSrc>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set the text in the mono face: durations, sizes.
    pub(crate) fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (background, foreground) = self.tone.colors();
        div()
            .flex_none()
            .h(px(16.0))
            .px(px(5.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(3.0))
            .rounded(px(R_XS))
            .bg(background)
            .text_size(px(TEXT_BADGE))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(foreground))
            .when(self.mono, |this| this.font_family(FONT_MONO))
            .children(self.icon.map(|icon| icon.svg(10.0, rgb(foreground))))
            .child(self.text)
    }
}
