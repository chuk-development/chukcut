//! Title styles and text templates: our own looks, as data.
//!
//! A **style** is a function of a font size: it sets the face, colours,
//! outline, shadow, box and spacing of a [`TextMaterial`] and leaves its id,
//! words and caption data alone. Everything that has a length is scaled by
//! the size, so a style applied to a 40 px subtitle and to a 160 px headline
//! looks like the same style. That is also how a style is applied to a title
//! that is already on the timeline: at that title's own size.
//!
//! A **template** is a style plus an animation from `motion` — a per-letter,
//! word or line entrance, and sometimes a clip-level exit or loop — added as
//! one clip in one undo step.
//!
//! Every look here is ours: plain parameter sets over the renderer's own
//! features, no fonts or artwork from anyone else. Families are the generic
//! ones (`sans-serif`, `serif`, `monospace`), which every machine resolves,
//! so a template looks the same on the machine a project is opened on.
//!
//! The tiles the asset panel shows are drawn by the compositor, through the
//! same text rasteriser and the same animation code as the preview, so a tile
//! is what the title will look like and not a picture of it.

use std::path::PathBuf;

use serde::Serialize;

use crate::modules::project::animation::{
    AnimationMaterial, AnimationPreset, ClipAnimation, Ease, StaggerOrder, TextAnimator,
    TextPreset, TextUnit,
};
use crate::modules::project::document::{
    Micros, Project, TextAlign, TextMaterial, TextShadow, Transform,
};

/// A group of styles: the asset panel's category column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleCategory {
    Basic,
    Outline,
    Box,
    Glow,
    Retro,
}

impl StyleCategory {
    pub const ALL: [StyleCategory; 5] = [
        StyleCategory::Basic,
        StyleCategory::Outline,
        StyleCategory::Box,
        StyleCategory::Glow,
        StyleCategory::Retro,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StyleCategory::Basic => "Basic",
            StyleCategory::Outline => "Outline",
            StyleCategory::Box => "Box",
            StyleCategory::Glow => "Glow",
            StyleCategory::Retro => "Retro",
        }
    }
}

/// One title style.
#[derive(Clone, Copy, Serialize)]
pub struct TitleStyle {
    /// Stable: what a CLI or a saved preference names it by.
    pub id: &'static str,
    pub name: &'static str,
    pub category: StyleCategory,
    /// What its tile, and a new title in it, says.
    pub sample: &'static str,
    /// A new title's size, as a multiple of the canvas's default title size.
    pub scale: f32,
    /// Where a new title in this style lands: the segment's position, in
    /// half-canvas units, y up. A style applied to an existing title does
    /// not move it.
    pub position: [f32; 2],
    #[serde(skip)]
    style: fn(&mut TextMaterial, f32),
}

impl std::fmt::Debug for TitleStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TitleStyle").field("id", &self.id).finish()
    }
}

impl TitleStyle {
    /// `material` in this style at its own font size. Its id, words and
    /// caption data are kept.
    pub fn apply(&self, material: &mut TextMaterial) {
        let size = material.font_size.max(1.0);
        let keep = (
            std::mem::take(&mut material.id),
            std::mem::take(&mut material.content),
            material.caption.take(),
        );
        *material = TextMaterial {
            font_size: size,
            ..TextMaterial::default()
        };
        (self.style)(material, size);
        material.id = keep.0;
        material.content = keep.1;
        material.caption = keep.2;
    }

    /// A new title in this style for `project`'s canvas, saying `content` or
    /// the style's sample.
    pub fn material(&self, project: &Project, content: Option<String>) -> TextMaterial {
        let content = content.unwrap_or_else(|| self.sample.to_string());
        let mut material = super::edit::default_material(project, Some(content));
        material.font_size = (material.font_size * self.scale).round().max(8.0);
        self.apply(&mut material);
        material
    }

    /// The segment transform a new title in this style starts with.
    pub fn transform(&self) -> Transform {
        Transform {
            position: self.position,
            ..Transform::default()
        }
    }
}

/// Straight sRGB from a hex colour and an alpha.
const fn hex(rgb: u32, alpha: f32) -> [f32; 4] {
    [
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        alpha,
    ]
}

const WHITE: [f32; 4] = hex(0xffffff, 1.0);
const BLACK: [f32; 4] = hex(0x000000, 1.0);

/// A shadow `offset` and `blur` font sizes away.
fn shadow(color: [f32; 4], offset: [f32; 2], blur: f32, s: f32) -> Option<TextShadow> {
    Some(TextShadow {
        color,
        offset: [(offset[0] * s).round(), (offset[1] * s).round()],
        blur: (blur * s).round(),
    })
}

/// An outline `width` font sizes wide, never under a pixel.
fn outline(m: &mut TextMaterial, color: [f32; 4], width: f32, s: f32) {
    m.stroke_color = color;
    m.stroke_width = (width * s).round().max(1.0);
}

/// A box behind the text.
fn boxed(m: &mut TextMaterial, color: [f32; 4], padding: f32, radius: f32, s: f32) {
    m.background = Some(color);
    m.background_padding = Some((padding * s).round());
    m.background_radius = (radius * s).round();
}

/// A glow: an outline and a shadow in one colour, with no offset.
fn glow(m: &mut TextMaterial, color: [f32; 4], s: f32) {
    outline(m, color, 0.035, s);
    m.shadow = shadow(color, [0.0, 0.0], 0.55, s);
}

/// How far a title placed at an edge stays from it, as a fraction of the
/// canvas: the action-safe margin short-form apps keep clear.
pub const EDGE: f32 = 0.06;

/// A new title lands at the bottom of the frame at this height.
const LOW: f32 = -0.68;
/// And at the top here.
const HIGH: f32 = 0.7;

macro_rules! style {
    ($id:literal, $name:literal, $cat:ident, $sample:literal, $scale:expr, $pos:expr, |$m:ident, $s:ident| $body:block) => {
        TitleStyle {
            id: $id,
            name: $name,
            category: StyleCategory::$cat,
            sample: $sample,
            scale: $scale,
            position: $pos,
            style: {
                #[allow(unused_variables)]
                fn style($m: &mut TextMaterial, $s: f32) $body
                style
            },
        }
    };
}

/// Every style, in the order the panel shows them.
pub fn styles() -> Vec<TitleStyle> {
    vec![
        // --- Basic --------------------------------------------------------
        style!(
            "classic",
            "Classic",
            Basic,
            "Your title",
            1.0,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                outline(m, BLACK, 0.045, s);
                m.shadow = shadow(hex(0x000000, 0.55), [0.08, 0.08], 0.12, s);
            }
        ),
        style!(
            "plain",
            "Plain",
            Basic,
            "Plain text",
            1.0,
            [0.0, 0.0],
            |m, s| {}
        ),
        style!(
            "minimal",
            "Minimal",
            Basic,
            "MINIMAL",
            0.8,
            [0.0, 0.0],
            |m, s| {
                m.letter_spacing = (0.3 * s).round();
                m.shadow = shadow(hex(0x000000, 0.35), [0.0, 0.03], 0.1, s);
            }
        ),
        style!(
            "big-impact",
            "Big impact",
            Basic,
            "BIG NEWS",
            1.6,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.letter_spacing = (-0.02 * s).round();
                m.line_height = Some(0.95);
                outline(m, BLACK, 0.07, s);
                m.shadow = shadow(BLACK, [0.06, 0.07], 0.0, s);
            }
        ),
        style!(
            "underline",
            "Underlined",
            Basic,
            "Underlined",
            1.0,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.underline = true;
                m.letter_spacing = (0.04 * s).round();
                m.shadow = shadow(hex(0x000000, 0.5), [0.0, 0.04], 0.12, s);
            }
        ),
        style!(
            "typewriter",
            "Typewriter",
            Basic,
            "Day one.",
            0.8,
            [0.0, 0.0],
            |m, s| {
                m.font_family = "monospace".into();
                m.color = hex(0xf4efe4, 1.0);
                m.letter_spacing = (0.04 * s).round();
                m.shadow = shadow(hex(0x000000, 0.6), [0.03, 0.03], 0.06, s);
            }
        ),
        style!(
            "elegant",
            "Elegant",
            Basic,
            "Elegant",
            1.0,
            [0.0, 0.0],
            |m, s| {
                m.font_family = "serif".into();
                m.italic = true;
                m.letter_spacing = (0.06 * s).round();
                m.shadow = shadow(hex(0x000000, 0.45), [0.0, 0.03], 0.25, s);
            }
        ),
        style!(
            "quote",
            "Quote",
            Basic,
            "\u{201c}Say it softly\u{201d}",
            0.8,
            [0.0, 0.0],
            |m, s| {
                m.font_family = "serif".into();
                m.italic = true;
                m.color = hex(0xe8e6e1, 1.0);
                m.line_height = Some(1.3);
            }
        ),
        // --- Outline ------------------------------------------------------
        style!(
            "bold-outline",
            "Bold outline",
            Outline,
            "Bold outline",
            1.0,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                outline(m, BLACK, 0.11, s);
            }
        ),
        style!(
            "yellow-punch",
            "Yellow punch",
            Outline,
            "WAIT FOR IT",
            1.1,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.color = hex(0xffd400, 1.0);
                outline(m, BLACK, 0.09, s);
                m.shadow = shadow(hex(0x000000, 0.6), [0.0, 0.06], 0.1, s);
            }
        ),
        style!(
            "comic",
            "Comic",
            Outline,
            "POW!",
            1.4,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.italic = true;
                m.color = hex(0xffe14d, 1.0);
                outline(m, BLACK, 0.08, s);
                m.shadow = shadow(BLACK, [0.1, 0.1], 0.0, s);
            }
        ),
        style!(
            "hollow",
            "Hollow",
            Outline,
            "HOLLOW",
            1.3,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.color = hex(0xffffff, 0.0);
                outline(m, WHITE, 0.035, s);
            }
        ),
        style!(
            "hard-shadow",
            "Hard shadow",
            Outline,
            "Hard shadow",
            1.0,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.shadow = shadow(hex(0x2fd5c8, 1.0), [0.07, 0.07], 0.0, s);
            }
        ),
        style!(
            "red-outline",
            "Red outline",
            Outline,
            "Don't miss",
            1.0,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                outline(m, hex(0xe53935, 1.0), 0.08, s);
                m.shadow = shadow(hex(0x000000, 0.5), [0.0, 0.05], 0.1, s);
            }
        ),
        // --- Box ----------------------------------------------------------
        style!(
            "subtitle-box",
            "Subtitle box",
            Box,
            "Subtitle in a box",
            0.6,
            [0.0, LOW],
            |m, s| {
                m.color = WHITE;
                boxed(m, hex(0x000000, 0.62), 0.3, 0.15, s);
            }
        ),
        style!(
            "lower-third",
            "Lower third",
            Box,
            "Alex Morgan\nDirector",
            0.55,
            [0.06, LOW + 0.12],
            |m, s| {
                m.bold = true;
                m.align = TextAlign::Left;
                m.line_height = Some(1.25);
                boxed(m, hex(0x1565c0, 0.92), 0.3, 0.08, s);
            }
        ),
        style!(
            "pill",
            "Pill",
            Box,
            "pill label",
            0.7,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.color = hex(0x111111, 1.0);
                boxed(m, WHITE, 0.35, 0.8, s);
            }
        ),
        style!(
            "highlighter",
            "Highlighter",
            Box,
            "highlight this",
            0.9,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.color = hex(0x111111, 1.0);
                boxed(m, hex(0xffe600, 1.0), 0.12, 0.06, s);
            }
        ),
        style!(
            "breaking",
            "Breaking",
            Box,
            "BREAKING NEWS",
            0.75,
            [0.0, LOW],
            |m, s| {
                m.bold = true;
                m.letter_spacing = (0.05 * s).round();
                boxed(m, hex(0xd32f2f, 1.0), 0.25, 0.0, s);
            }
        ),
        style!("tag", "Tag", Box, "NEW", 0.6, [0.0, HIGH], |m, s| {
            m.bold = true;
            m.color = hex(0x062220, 1.0);
            m.letter_spacing = (0.12 * s).round();
            boxed(m, hex(0x2fd5c8, 1.0), 0.3, 0.25, s);
        }),
        style!(
            "dark-card",
            "Dark card",
            Box,
            "A quiet card",
            0.8,
            [0.0, 0.0],
            |m, s| {
                m.color = hex(0xf2f2f2, 1.0);
                m.line_height = Some(1.2);
                boxed(m, hex(0x15161a, 0.86), 0.5, 0.3, s);
            }
        ),
        // --- Glow ---------------------------------------------------------
        style!(
            "neon-pink",
            "Neon",
            Glow,
            "NEON",
            1.3,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.color = hex(0xffe6f7, 1.0);
                glow(m, hex(0xff2bd6, 1.0), s);
            }
        ),
        style!(
            "neon-cyan",
            "Neon blue",
            Glow,
            "Night",
            1.3,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.color = hex(0xe6fdff, 1.0);
                glow(m, hex(0x19d3ff, 1.0), s);
            }
        ),
        style!(
            "soft-glow",
            "Soft glow",
            Glow,
            "Dreamy",
            1.1,
            [0.0, 0.0],
            |m, s| {
                m.font_family = "serif".into();
                m.italic = true;
                m.shadow = shadow(hex(0xffffff, 0.9), [0.0, 0.0], 0.6, s);
            }
        ),
        style!("fire", "Fire", Glow, "HOT", 1.4, [0.0, 0.0], |m, s| {
            m.bold = true;
            m.color = hex(0xffd36b, 1.0);
            outline(m, hex(0xff6a00, 1.0), 0.03, s);
            m.shadow = shadow(hex(0xff2a00, 1.0), [0.0, -0.04], 0.5, s);
        }),
        style!("ice", "Ice", Glow, "Frozen", 1.1, [0.0, 0.0], |m, s| {
            m.bold = true;
            m.color = hex(0xdff4ff, 1.0);
            outline(m, WHITE, 0.02, s);
            m.shadow = shadow(hex(0x4fb3ff, 1.0), [0.0, 0.0], 0.45, s);
        }),
        // --- Retro --------------------------------------------------------
        style!(
            "sunset",
            "Sunset",
            Retro,
            "Sunset",
            1.2,
            [0.0, 0.0],
            |m, s| {
                m.font_family = "serif".into();
                m.bold = true;
                m.italic = true;
                m.color = hex(0xffb347, 1.0);
                outline(m, hex(0x3b1053, 1.0), 0.06, s);
                m.shadow = shadow(hex(0xff4f81, 1.0), [0.07, 0.07], 0.0, s);
            }
        ),
        style!(
            "synthwave",
            "Synthwave",
            Retro,
            "RETRO",
            1.3,
            [0.0, 0.0],
            |m, s| {
                m.bold = true;
                m.italic = true;
                m.letter_spacing = (0.06 * s).round();
                m.color = hex(0x7df9ff, 1.0);
                outline(m, hex(0xff00c8, 1.0), 0.04, s);
                m.shadow = shadow(hex(0x8a2be2, 1.0), [0.0, 0.08], 0.3, s);
            }
        ),
        style!(
            "stamp",
            "Stamp",
            Retro,
            "APPROVED",
            1.0,
            [0.0, 0.0],
            |m, s| {
                m.font_family = "monospace".into();
                m.bold = true;
                m.underline = true;
                m.letter_spacing = (0.1 * s).round();
                m.color = hex(0xe23b3b, 1.0);
            }
        ),
        style!(
            "vintage",
            "Vintage",
            Retro,
            "Since 1984",
            1.0,
            [0.0, 0.0],
            |m, s| {
                m.font_family = "serif".into();
                m.color = hex(0xf3e2c0, 1.0);
                m.letter_spacing = (0.08 * s).round();
                m.shadow = shadow(hex(0x5a3a1a, 0.9), [0.04, 0.04], 0.02, s);
            }
        ),
    ]
}

/// The style called `id`.
pub fn style(id: &str) -> Option<TitleStyle> {
    styles().into_iter().find(|s| s.id == id)
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// One text template: a style and an animation.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct TextTemplate {
    pub id: &'static str,
    pub name: &'static str,
    /// The [`TitleStyle`] it draws with.
    pub style: &'static str,
    /// Its words, when they differ from the style's sample.
    pub sample: Option<&'static str>,
    /// Per letter, word or line, from the clip's first frame.
    pub text_in: Option<TextAnimator>,
    /// Clip-level: in, out and loop.
    pub intro: Option<ClipAnimation>,
    pub outro: Option<ClipAnimation>,
    pub combo: Option<ClipAnimation>,
}

impl TextTemplate {
    /// The animation a title made from this template carries, under a fresh
    /// id.
    pub fn animation(&self) -> AnimationMaterial {
        AnimationMaterial {
            intro: self.intro,
            outro: self.outro,
            combo: self.combo,
            text_in: self.text_in,
            ..AnimationMaterial::new()
        }
    }

    pub fn title_style(&self) -> TitleStyle {
        style(self.style).expect("every template names a style that exists")
    }

    /// What the template's tile and a new title from it say.
    pub fn sample(&self) -> &'static str {
        self.sample.unwrap_or_else(|| self.title_style().sample)
    }

    /// The instant its tile is drawn at: most of the way through the
    /// entrance, so the tile shows the move and still reads.
    pub fn tile_time(&self) -> Micros {
        let window = self
            .text_in
            .map(|a| a.duration)
            .into_iter()
            .chain(self.intro.map(|a| a.duration))
            .max()
            .unwrap_or(0);
        window * 7 / 10
    }
}

fn animator(
    preset: TextPreset,
    unit: TextUnit,
    duration: Micros,
    overlap: f32,
    easing: Ease,
) -> Option<TextAnimator> {
    Some(TextAnimator {
        preset,
        unit,
        order: StaggerOrder::Forward,
        seed: 0,
        duration,
        overlap,
        easing,
        strength: 1.0,
    })
}

fn clip(preset: AnimationPreset, duration: Micros, easing: Ease) -> Option<ClipAnimation> {
    Some(ClipAnimation {
        preset,
        duration,
        easing,
        strength: 1.0,
    })
}

/// Every template, in the order the panel shows them.
pub fn templates() -> Vec<TextTemplate> {
    use AnimationPreset as A;
    use TextPreset as T;
    use TextUnit as U;
    let none = TextTemplate {
        id: "",
        name: "",
        style: "classic",
        sample: None,
        text_in: None,
        intro: None,
        outro: None,
        combo: None,
    };
    vec![
        TextTemplate {
            id: "typed-note",
            name: "Typed note",
            style: "typewriter",
            sample: Some("Day one. Let's go."),
            text_in: animator(T::Typewriter, U::Letter, 1_400_000, 0.0, Ease::Linear),
            outro: clip(A::Fade, 400_000, Ease::EaseIn),
            ..none
        },
        TextTemplate {
            id: "pop-headline",
            name: "Pop headline",
            style: "big-impact",
            text_in: animator(T::Pop, U::Word, 700_000, 0.3, Ease::Back),
            outro: clip(A::ZoomOut, 300_000, Ease::EaseIn),
            ..none
        },
        TextTemplate {
            id: "neon-sign",
            name: "Neon sign",
            style: "neon-pink",
            text_in: animator(T::Fade, U::Letter, 900_000, 0.4, Ease::EaseOut),
            combo: clip(A::Flicker, 2_000_000, Ease::Linear),
            ..none
        },
        TextTemplate {
            id: "name-tag",
            name: "Name tag",
            style: "lower-third",
            intro: clip(A::WipeRight, 500_000, Ease::EaseOut),
            text_in: animator(T::SlideUp, U::Line, 700_000, 0.4, Ease::EaseOut),
            outro: clip(A::WipeLeft, 400_000, Ease::EaseIn),
            ..none
        },
        TextTemplate {
            id: "subtitle-rise",
            name: "Subtitle rise",
            style: "subtitle-box",
            text_in: animator(T::FadeUp, U::Word, 800_000, 0.5, Ease::EaseOut),
            outro: clip(A::Fade, 300_000, Ease::EaseIn),
            ..none
        },
        TextTemplate {
            id: "breaking-wipe",
            name: "Breaking",
            style: "breaking",
            intro: clip(A::WipeRight, 450_000, Ease::EaseOut),
            outro: clip(A::WipeLeft, 350_000, Ease::EaseIn),
            ..none
        },
        TextTemplate {
            id: "comic-drop",
            name: "Comic drop",
            style: "comic",
            text_in: animator(T::Drop, U::Letter, 1_000_000, 0.6, Ease::Bounce),
            combo: clip(A::Wobble, 1_600_000, Ease::Smooth),
            ..none
        },
        TextTemplate {
            id: "retro-zoom",
            name: "Retro zoom",
            style: "synthwave",
            text_in: animator(T::Zoom, U::Letter, 900_000, 0.5, Ease::EaseOut),
            combo: clip(A::Pulse, 1_200_000, Ease::Smooth),
            ..none
        },
        TextTemplate {
            id: "elegant-reveal",
            name: "Elegant reveal",
            style: "elegant",
            text_in: animator(T::Fade, U::Letter, 1_600_000, 0.75, Ease::Smooth),
            outro: clip(A::Blur, 500_000, Ease::EaseIn),
            ..none
        },
        TextTemplate {
            id: "spin-tag",
            name: "Spin tag",
            style: "tag",
            text_in: animator(T::Spin, U::Letter, 800_000, 0.5, Ease::Back),
            combo: clip(A::Float, 2_000_000, Ease::Smooth),
            ..none
        },
    ]
}

/// The template called `id`.
pub fn template(id: &str) -> Option<TextTemplate> {
    templates().into_iter().find(|t| t.id == id)
}

// ---------------------------------------------------------------------------
// Position presets
// ---------------------------------------------------------------------------

/// Where on the canvas a title sits: a 3 × 3 grid, as the Text tab's
/// position buttons offer it. Each also sets the paragraph's alignment, so
/// a title in the left column is ragged right and reads from the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextPosition {
    TopLeft,
    Top,
    TopRight,
    Left,
    Centre,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl TextPosition {
    pub const ALL: [TextPosition; 9] = [
        TextPosition::TopLeft,
        TextPosition::Top,
        TextPosition::TopRight,
        TextPosition::Left,
        TextPosition::Centre,
        TextPosition::Right,
        TextPosition::BottomLeft,
        TextPosition::Bottom,
        TextPosition::BottomRight,
    ];

    /// Column and row, each -1, 0 or 1 (y down, as the grid is drawn).
    pub fn cell(self) -> (i8, i8) {
        let i = Self::ALL.iter().position(|p| *p == self).unwrap_or(4) as i8;
        (i % 3 - 1, i / 3 - 1)
    }

    pub fn label(self) -> &'static str {
        match self {
            TextPosition::TopLeft => "Top left",
            TextPosition::Top => "Top",
            TextPosition::TopRight => "Top right",
            TextPosition::Left => "Left",
            TextPosition::Centre => "Centre",
            TextPosition::Right => "Right",
            TextPosition::BottomLeft => "Bottom left",
            TextPosition::Bottom => "Bottom",
            TextPosition::BottomRight => "Bottom right",
        }
    }

    /// The paragraph alignment of the column.
    pub fn align(self) -> TextAlign {
        match self.cell().0 {
            -1 => TextAlign::Left,
            1 => TextAlign::Right,
            _ => TextAlign::Center,
        }
    }

    /// The segment position, in half-canvas units, y up, for a paragraph
    /// `height` tall as a fraction of the canvas height (what it measures,
    /// box included, at the clip's scale).
    ///
    /// The layer is the size of the canvas with the paragraph in its middle,
    /// aligned to its edge; so a side column only insets from the edge, and
    /// a row moves the layer until the paragraph's edge is [`EDGE`] in from
    /// the canvas's. A paragraph too tall for that stays in the middle.
    pub fn position(self, height: f32) -> [f32; 2] {
        let (column, row) = self.cell();
        let travel = (1.0 - 2.0 * EDGE - height.max(0.0)).max(0.0);
        [-(column as f32) * 2.0 * EDGE, -(row as f32) * travel]
    }

    /// Which cell a title at `position` with alignment `align` is in: the
    /// column from the alignment, the row from which side of the middle it
    /// sits.
    pub fn of(position: [f32; 2], align: TextAlign) -> TextPosition {
        let column = match align {
            TextAlign::Left => 0,
            TextAlign::Center => 1,
            TextAlign::Right => 2,
        };
        let row = if position[1] > 0.02 {
            0
        } else if position[1] < -0.02 {
            2
        } else {
            1
        };
        Self::ALL[row * 3 + column]
    }
}

// ---------------------------------------------------------------------------
// Tiles
// ---------------------------------------------------------------------------

/// The tile of style `id`: its sample on a dark ground, drawn by the
/// compositor.
pub fn style_tile(id: &str, size: (u32, u32)) -> Result<PathBuf, String> {
    style_tile_in(&crate::modules::fx::tiles::tiles_dir(), id, size)
}

fn style_tile_in(dir: &std::path::Path, id: &str, size: (u32, u32)) -> Result<PathBuf, String> {
    let style = style(id).ok_or_else(|| format!("there is no title style called {id}"))?;
    tiles::draw(dir, id, &style, style.sample, None, 0, size)
}

/// The tile of template `id`: its style, caught in the middle of its
/// entrance.
pub fn template_tile(id: &str, size: (u32, u32)) -> Result<PathBuf, String> {
    template_tile_in(&crate::modules::fx::tiles::tiles_dir(), id, size)
}

fn template_tile_in(dir: &std::path::Path, id: &str, size: (u32, u32)) -> Result<PathBuf, String> {
    let template = template(id).ok_or_else(|| format!("there is no text template called {id}"))?;
    let animation = template.animation();
    tiles::draw(
        dir,
        id,
        &template.title_style(),
        template.sample(),
        Some(&animation),
        template.tile_time(),
        size,
    )
}

mod tiles {
    use std::path::PathBuf;

    use super::TitleStyle;
    use crate::modules::fx::tiles::{cached, compositor, hash, upload};
    use crate::modules::project::animation::AnimationMaterial;
    use crate::modules::project::document::{
        CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    };
    use crate::modules::render::{RenderContext, SourceFrame, SourceProvider, SourceRequest};
    use crate::modules::text::{RasterOptions, TextRenderer};

    /// A title tile's canvas: the panel's tile shape at twice the size, so a
    /// title's proportions to its canvas are a title's on a phone.
    const CANVAS: (u32, u32) = (448, 280);

    /// The ground behind the sample, in linear light: about the panel's
    /// raised grey once encoded, so a dark outline still reads.
    const GROUND: [f32; 4] = [0.016, 0.017, 0.021, 1.0];

    /// Serves the one title the tile has.
    struct Titles<'a>(&'a Project);

    impl SourceProvider for Titles<'_> {
        fn frame(
            &self,
            ctx: &RenderContext,
            request: &SourceRequest<'_>,
        ) -> anyhow::Result<Option<SourceFrame>> {
            let Some(material) = self.0.materials.text(request.material_id) else {
                return Ok(None);
            };
            let size = request.max_size;
            let size = (size.0.max(1), size.1.max(1));
            let scale = size.0 as f32 / self.0.canvas.width.max(1) as f32;
            let options = RasterOptions::canvas(size.0, size.1).with_scale(scale);
            let rastered = TextRenderer::shared()
                .rasterize(&crate::modules::text::TextRequest::from(material), &options);
            Ok(Some(upload(
                ctx,
                &rastered.pixels,
                rastered.width,
                rastered.height,
            )))
        }
    }

    pub(super) fn draw(
        dir: &std::path::Path,
        id: &str,
        style: &TitleStyle,
        sample: &str,
        animation: Option<&AnimationMaterial>,
        time: Micros,
        size: (u32, u32),
    ) -> Result<PathBuf, String> {
        let mut project = Project::new(
            "title tile",
            CanvasConfig {
                width: CANVAS.0,
                height: CANVAS.1,
                background: GROUND,
            },
            30.0,
        );
        let mut material = style.material(&project, Some(sample.to_string()));
        // The default size is for a phone's short edge; a tile is a landscape
        // card, so its text is set against the height instead and kept to a
        // size that fits the card's width.
        material.font_size = (CANVAS.1 as f32 * 0.2 * style.scale.min(1.4)).round();
        style.apply(&mut material);
        material.id = "title".into();
        let animation = animation.map(|a| AnimationMaterial {
            id: "anim".into(),
            ..a.clone()
        });

        let inset = match material.align {
            crate::modules::project::document::TextAlign::Left => 0.12,
            crate::modules::project::document::TextAlign::Right => -0.12,
            crate::modules::project::document::TextAlign::Center => 0.0,
        };
        let style_key = format!("{material:?}{animation:?}{time}{inset}");
        let key = hash(&[
            include_str!("raster.rs"),
            include_str!("../motion/text.rs"),
            &style_key,
        ]);
        let path = dir.join(format!("{key:016x}-title-{id}-{}x{}.png", size.0, size.1));
        cached(path, move || {
            let mut track = Track::new(TrackKind::Text, "Title");
            let mut segment = Segment {
                id: "s".into(),
                material_id: material.id.clone(),
                target_range: TimeRange::new(0, 4_000_000),
                source_range: TimeRange::new(0, 4_000_000),
                render_index: 0,
                speed: 1.0,
                volume: 1.0,
                // A paragraph aligned to an edge is moved in from it, as the
                // position grid does on a real canvas.
                transform: Transform {
                    position: [inset, 0.0],
                    ..Transform::default()
                },
                crop: None,
                extras: Vec::new(),
                keyframes: Vec::new(),
            };
            if let Some(animation) = &animation {
                segment.extras.push(animation.id.clone());
                project.materials.animations.push(animation.clone());
            }
            track.segments.push(segment);
            project.tracks.push(track);
            project.materials.texts.push(material);
            // Drawn at the canvas size and scaled to the tile's: the title
            // is laid out as it would be, then shrunk as a picture.
            let c = compositor()?;
            let frame = c
                .render(&project, time, CANVAS, &Titles(&project))
                .map_err(|e| e.to_string())?;
            let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.data)
                .ok_or("the frame did not fit its own size")?;
            Ok(if (frame.width, frame.height) == size {
                image
            } else {
                image::imageops::resize(&image, size.0, size.1, image::imageops::Triangle)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::CanvasConfig;

    #[test]
    fn there_are_enough_styles_and_templates_and_every_id_is_unique() {
        let styles = styles();
        assert!((20..=30).contains(&styles.len()), "{} styles", styles.len());
        for s in &styles {
            assert_eq!(
                styles.iter().filter(|o| o.id == s.id).count(),
                1,
                "{}",
                s.id
            );
        }
        for category in StyleCategory::ALL {
            assert!(
                styles.iter().any(|s| s.category == category),
                "{category:?} is empty"
            );
        }
        let templates = templates();
        assert_eq!(templates.len(), 10);
        for t in &templates {
            assert_eq!(
                templates.iter().filter(|o| o.id == t.id).count(),
                1,
                "{}",
                t.id
            );
            assert!(style(t.style).is_some(), "{} names {}", t.id, t.style);
            let animation = t.animation();
            assert!(!animation.is_empty(), "{} does not move", t.id);
            assert_eq!(animation.problem(), None, "{}", t.id);
        }
    }

    #[test]
    fn every_style_makes_a_title_the_document_accepts() {
        let project = Project::new("t", CanvasConfig::default(), 30.0);
        for s in styles() {
            let material = s.material(&project, None);
            assert_eq!(material.content, s.sample);
            crate::modules::text::edit::check_material(&material)
                .unwrap_or_else(|e| panic!("{}: {e}", s.id));
        }
    }

    /// Applying a style restyles and nothing else: the words, the id, the
    /// caption data and the size stay, and the lengths follow the size.
    #[test]
    fn a_style_applied_to_a_title_keeps_its_words_and_size() {
        let project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut title = crate::modules::text::edit::default_material(&project, Some("Mine".into()));
        title.font_size = 100.0;
        title.caption = Some(Default::default());
        let id = title.id.clone();

        let outline = style("bold-outline").unwrap();
        let mut small = title.clone();
        small.font_size = 50.0;
        outline.apply(&mut title);
        outline.apply(&mut small);

        assert_eq!(title.id, id);
        assert_eq!(title.content, "Mine");
        assert!(title.caption.is_some());
        assert_eq!(title.font_size, 100.0);
        assert!((title.stroke_width - small.stroke_width * 2.0).abs() <= 1.0);

        // Nothing from the old look survives a style that does not set it.
        style("subtitle-box").unwrap().apply(&mut title);
        assert!(title.background.is_some());
        style("plain").unwrap().apply(&mut title);
        assert_eq!(title.background, None);
        assert_eq!(title.stroke_width, 0.0);
        assert_eq!(title.shadow, None);
    }

    #[test]
    fn every_style_and_template_draws_a_tile_of_its_own() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/title-tile-test")
            .join(format!("{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        if crate::modules::fx::tiles::compositor().is_err() {
            eprintln!("skipping: no GPU adapter");
            return;
        }
        let size = (224, 140);
        let mut seen = std::collections::HashSet::new();
        for s in styles() {
            let path = style_tile_in(&dir, s.id, size).unwrap();
            let image = image::open(&path).unwrap().to_rgba8();
            assert_eq!(image.dimensions(), size, "{}", s.id);
            // Something was drawn over the ground, and no two styles look
            // the same.
            let ground = *image.get_pixel(0, 0);
            let inked = image.pixels().filter(|p| **p != ground).count();
            assert!(inked > 300, "{} drew {inked} pixels", s.id);
            assert!(
                seen.insert(image.into_raw()),
                "{} looks like another style",
                s.id
            );
        }
        for t in templates() {
            let path = template_tile_in(&dir, t.id, size).unwrap();
            assert!(path.exists(), "{}", t.id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every tile into `target/title-tiles`, for a person to look at:
    /// `cargo test -p chukcut-engine --lib -- --ignored title_tiles_for_review`.
    #[test]
    #[ignore]
    fn title_tiles_for_review() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/title-tiles");
        let _ = std::fs::remove_dir_all(&dir);
        for s in styles() {
            style_tile_in(&dir, s.id, (448, 280)).unwrap();
        }
        for t in templates() {
            template_tile_in(&dir, t.id, (448, 280)).unwrap();
        }
        eprintln!("tiles in {}", dir.display());
    }

    #[test]
    fn the_position_grid_insets_columns_and_moves_rows() {
        assert_eq!(TextPosition::Centre.position(0.1), [0.0, 0.0]);
        assert_eq!(TextPosition::TopLeft.align(), TextAlign::Left);
        assert_eq!(TextPosition::BottomRight.align(), TextAlign::Right);
        let top = TextPosition::Top.position(0.1);
        let bottom = TextPosition::Bottom.position(0.1);
        assert!(
            top[1] > 0.5 && bottom[1] < -0.5,
            "y is up: {top:?} {bottom:?}"
        );
        // A paragraph a tenth of the canvas tall, 6% in from the bottom:
        // its centre is 0.11 of the height above the bottom edge.
        assert!((bottom[1] + 0.78).abs() < 1e-5, "{bottom:?}");
        // Taller text moves less, and text taller than the safe area stays.
        assert!(TextPosition::Bottom.position(0.5)[1] > bottom[1]);
        assert_eq!(TextPosition::Bottom.position(0.95)[1], 0.0);
        assert!(
            TextPosition::Left.position(0.1)[0] > 0.0,
            "a left column is inset"
        );
        for p in TextPosition::ALL {
            assert_eq!(TextPosition::of(p.position(0.2), p.align()), p);
        }
    }
}
