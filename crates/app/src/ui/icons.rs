//! Our own icon set, for the glyphs that carry the product's identity: the
//! logo mark, the asset rail, the transport, keyframes. Generic glyphs (undo,
//! trash, search, folders) come from Lucide.
//!
//! Every glyph is drawn on a 24 grid with a 1.75 stroke, round caps and
//! joins, and no fill except an "on" state. The stroke is black because GPUI
//! uses the SVG as a mask and tints it with the element's text colour.

use gpui::assets::IconName;
use gpui::prelude::*;
use gpui::{px, svg, Hsla, Svg};

/// A glyph of our own set: the inner markup of a 24×24 SVG.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Glyph(pub &'static [u8]);

macro_rules! glyphs {
    ($($(#[$doc:meta])* $name:ident = $body:literal;)*) => {
        $(
            $(#[$doc])*
            pub(crate) const $name: Glyph = Glyph(concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</svg>"
            ).as_bytes());
        )*
    };
}

glyphs! {
    /// The chukcut mark: a "c" opened by a cut.
    LOGO = r#"<path d="M17.6 7.4A7.2 7.2 0 1 0 17.6 16.6"/><path d="M13 13.5l7-7"/>"#;

    /// Asset rail: a frame with a play mark.
    MEDIA = r#"<rect x="3.5" y="5" width="17" height="14" rx="3"/><path d="M10.25 9.4v5.2l4.4-2.6z"/>"#;
    /// Asset rail: bars of a level meter.
    AUDIO = r#"<path d="M4.5 10.5v3M8.25 7.5v9M12 4.5v15M15.75 8.5v7M19.5 11v2"/>"#;
    /// Asset rail: a type tool.
    TEXT = r#"<path d="M5.5 7.5V5h13v2.5M12 5v14M9.25 19h5.5"/>"#;
    /// Asset rail: two frames sliding over each other.
    TRANSITIONS = r#"<rect x="3.5" y="4.5" width="11" height="11" rx="2.5"/><path d="M17.5 8.5h.5a2.5 2.5 0 0 1 2.5 2.5v6a2.5 2.5 0 0 1-2.5 2.5h-6a2.5 2.5 0 0 1-2.5-2.5v-.5"/>"#;
    /// Asset rail: three overlapping light circles.
    FILTERS = r#"<circle cx="12" cy="8.75" r="4.75"/><circle cx="8.75" cy="14.75" r="4.75"/><circle cx="15.25" cy="14.75" r="4.75"/>"#;
    /// Asset rail: a spark.
    EFFECTS = r#"<path d="M12 3.5l1.9 5.1 5.1 1.9-5.1 1.9L12 17.5l-1.9-5.1-5.1-1.9 5.1-1.9z"/><path d="M18.5 16v4M16.5 18h4"/>"#;

    /// Transport: play, filled.
    PLAY = r#"<path d="M8 5.6v12.8a.8.8 0 0 0 1.2.7l10.3-6.4a.8.8 0 0 0 0-1.4L9.2 4.9A.8.8 0 0 0 8 5.6z" fill="black"/>"#;
    /// Transport: pause, filled.
    PAUSE = r#"<rect x="6.5" y="5" width="3.5" height="14" rx="1" fill="black"/><rect x="14" y="5" width="3.5" height="14" rx="1" fill="black"/>"#;
    /// Transport: one frame back.
    STEP_BACK = r#"<path d="M6 6v12"/><path d="M18 7v10l-7.5-5z" fill="black"/>"#;
    /// Transport: one frame forward.
    STEP_FORWARD = r#"<path d="M18 6v12"/><path d="M6 7v10l7.5-5z" fill="black"/>"#;
    /// Player: fit the canvas to the viewer.
    FIT = r#"<path d="M4 9V6a2 2 0 0 1 2-2h3M15 4h3a2 2 0 0 1 2 2v3M20 15v3a2 2 0 0 1-2 2h-3M9 20H6a2 2 0 0 1-2-2v-3"/><rect x="8.5" y="8.5" width="7" height="7" rx="1.5"/>"#;
    /// Player: full screen.
    FULLSCREEN = r#"<path d="M14 4h6v6M10 20H4v-6M20 4l-6 6M4 20l6-6"/>"#;
    /// Player: canvas ratio.
    RATIO = r#"<rect x="3.5" y="6" width="17" height="12" rx="2.5"/><path d="M8 9.5H6.5V11M16 14.5h1.5V13"/>"#;

    /// Keyframe, off.
    DIAMOND = r#"<path d="M12 5.5l6.5 6.5-6.5 6.5L5.5 12z"/>"#;
    /// Keyframe, on.
    DIAMOND_FILLED = r#"<path d="M12 5.5l6.5 6.5-6.5 6.5L5.5 12z" fill="black"/>"#;
    /// Reset a value to its default.
    RESET = r#"<path d="M5 12.5a7 7 0 1 0 2.1-5"/><path d="M5 4.5v4h4"/>"#;
    CHEVRON_LEFT = r#"<path d="M14.5 7l-5 5 5 5"/>"#;
    CHEVRON_RIGHT = r#"<path d="M9.5 7l5 5-5 5"/>"#;
    CHEVRON_UP = r#"<path d="M7 14.5l5-5 5 5"/>"#;
    CHEVRON_DOWN = r#"<path d="M7 9.5l5 5 5-5"/>"#;
    CHECK = r#"<path d="M5.5 12.5l4.25 4.25L18.5 8"/>"#;
    /// Import into the library.
    IMPORT = r#"<path d="M12 4v11M7.5 10.5L12 15l4.5-4.5"/><path d="M5 19.5h14"/>"#;
    /// Export a file.
    EXPORT = r#"<path d="M12 15.5V4.5M7.5 9L12 4.5 16.5 9"/><path d="M5 14v3.5A2 2 0 0 0 7 19.5h10a2 2 0 0 0 2-2V14"/>"#;
    PLUS = r#"<path d="M12 5.5v13M5.5 12h13"/>"#;
}

/// Anything the kit can draw as an icon: one of ours, or a Lucide glyph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IconSrc {
    Glyph(Glyph),
    Lucide(IconName),
}

impl From<Glyph> for IconSrc {
    fn from(glyph: Glyph) -> Self {
        IconSrc::Glyph(glyph)
    }
}

impl From<IconName> for IconSrc {
    fn from(name: IconName) -> Self {
        IconSrc::Lucide(name)
    }
}

impl IconSrc {
    /// The icon as an SVG element of `size` pixels in `color`. A plain
    /// element, so callers can add `group_hover` to recolour it.
    pub(crate) fn svg(self, size: f32, color: impl Into<Hsla>) -> Svg {
        let element = svg().flex_none().size(px(size)).text_color(color.into());
        match self {
            IconSrc::Glyph(Glyph(data)) => element.data(data),
            IconSrc::Lucide(name) => element.path(name.path()),
        }
    }
}

/// One of our glyphs, sized and tinted.
pub(crate) fn glyph(glyph: Glyph, size: f32, color: impl Into<Hsla>) -> Svg {
    IconSrc::Glyph(glyph).svg(size, color)
}
