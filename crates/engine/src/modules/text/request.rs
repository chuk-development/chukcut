//! What the caller asks for.
//!
//! [`TextRequest`] is everything about *the text*, [`RasterOptions`] is
//! everything about *the image it lands in*. They are separate because the
//! first is document state and the second is a property of the render — the
//! same title is rasterised at 540x960 for the preview and 1080x1920 for the
//! export without the document knowing.

use crate::modules::project::document::{TextAlign, TextMaterial, TextShadow};

/// A text layer to lay out and paint.
///
/// This mirrors [`TextMaterial`] and adds the typographic controls the document
/// does not carry yet — line height, letter spacing and the background box's
/// geometry. Those have defaults that reproduce what a `TextMaterial` alone
/// implies, so `TextRequest::from(&material)` is lossless in both directions
/// until the document grows the fields.
///
/// Not `PartialEq`: comparing two of these is always a question about whether
/// they *draw* the same, and the answer to that is the cache key, not field
/// equality.
#[derive(Debug, Clone)]
pub struct TextRequest {
    pub content: String,
    /// CSS-style family name. Unknown names fall back rather than failing; see
    /// [`crate::modules::text::font`].
    pub font_family: String,
    /// In pixels at scale 1.
    pub font_size: f32,
    /// Straight (non-premultiplied) sRGB, 0..1.
    pub color: [f32; 4],
    pub bold: bool,
    pub italic: bool,
    pub align: TextAlign,
    /// Visible outline width in pixels, outside the glyph. 0 disables it.
    pub stroke_width: f32,
    pub stroke_color: [f32; 4],
    pub shadow: Option<TextShadow>,
    /// Fill of the box drawn behind the text, if any.
    pub background: Option<[f32; 4]>,

    // --- not in `TextMaterial` yet ---
    /// Line advance as a multiple of the font size. `None` uses the font's own
    /// ascent + descent + line gap, which is what a word processor does and
    /// what looks right for a paragraph. A title usually wants a number.
    pub line_height: Option<f32>,
    /// Extra space between glyphs, in pixels at scale 1. Negative tightens.
    pub letter_spacing: f32,
    /// How far the background box extends past the text, in pixels at scale 1.
    /// Defaults to a fifth of the font size, which is roughly what CapCut draws.
    pub background_padding: Option<f32>,
    /// Corner radius of the background box, in pixels at scale 1.
    pub background_radius: f32,
    /// Paint one byte range of `content` in another colour — the word being
    /// spoken in a karaoke caption. Time is not a property of a text layer, so
    /// the caller decides which range is lit at which instant; see
    /// `captions::karaoke`.
    pub highlight: Option<TextHighlight>,
}

/// A byte range of the content painted in its own fill colour.
#[derive(Debug, Clone, PartialEq)]
pub struct TextHighlight {
    pub range: std::ops::Range<usize>,
    /// Straight sRGB, 0..1, like [`TextRequest::color`].
    pub color: [f32; 4],
}

impl TextRequest {
    pub(crate) fn background_padding_px(&self) -> f32 {
        self.background_padding
            .unwrap_or(self.font_size * 0.2)
            .max(0.0)
    }

    /// How far paint can spill past the glyph outlines, in pixels at scale 1.
    ///
    /// This is what sizes the margin of a tight image and what stops a shadow
    /// being clipped at the edge of a canvas-sized one.
    pub(crate) fn bleed(&self) -> f32 {
        let stroke = self.stroke_width.max(0.0);
        let shadow = self.shadow.map_or(0.0, |s| {
            let offset = s.offset[0].abs().max(s.offset[1].abs());
            offset + s.blur.max(0.0) * 2.0
        });
        let background = if self.background.is_some() {
            self.background_padding_px()
        } else {
            0.0
        };
        stroke + shadow.max(background) + 1.0
    }
}

impl Default for TextRequest {
    fn default() -> Self {
        Self {
            content: String::new(),
            font_family: "sans-serif".to_string(),
            font_size: 48.0,
            color: [1.0, 1.0, 1.0, 1.0],
            bold: false,
            italic: false,
            align: TextAlign::default(),
            stroke_width: 0.0,
            stroke_color: [0.0, 0.0, 0.0, 1.0],
            shadow: None,
            background: None,
            line_height: None,
            letter_spacing: 0.0,
            background_padding: None,
            background_radius: 0.0,
            highlight: None,
        }
    }
}

impl From<&TextMaterial> for TextRequest {
    fn from(material: &TextMaterial) -> Self {
        Self {
            content: material.content.clone(),
            font_family: material.font_family.clone(),
            font_size: material.font_size,
            color: material.color,
            bold: material.bold,
            italic: material.italic,
            align: material.align,
            stroke_width: material.stroke_width,
            stroke_color: material.stroke_color,
            shadow: material.shadow,
            background: material.background,
            ..Self::default()
        }
    }
}

/// The shape of the image the text is painted into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RasterTarget {
    /// One image the size of the canvas, with the text placed inside it.
    ///
    /// This is what the compositor wants. `render::layout::fit_size` scales a
    /// source to fit the canvas, so a canvas-sized text layer is drawn 1:1 and
    /// the segment's own transform positions it — exactly as it does for a
    /// video clip. A tightly cropped image would instead be *stretched* to fill
    /// the frame, which is the bug this variant exists to avoid.
    Canvas { width: u32, height: u32 },
    /// An image just big enough for the text and its effects.
    ///
    /// For thumbnails, measurement and tests. `max_width` is the wrap width in
    /// pixels at scale 1; `None` means never wrap except at explicit newlines.
    Tight { max_width: Option<f32> },
}

/// Where the text block sits vertically inside a [`RasterTarget::Canvas`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerticalAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterOptions {
    pub target: RasterTarget,
    /// Device pixels per logical pixel. The export renders the same title at a
    /// larger scale than the preview and gets sharper glyphs, not bigger ones.
    pub scale: f32,
    pub vertical_align: VerticalAlign,
    /// Space kept clear at the edges of a canvas target, in pixels at scale 1.
    /// Text wraps inside it.
    pub margin: f32,
}

impl RasterOptions {
    pub fn canvas(width: u32, height: u32) -> Self {
        Self {
            target: RasterTarget::Canvas { width, height },
            ..Self::default()
        }
    }

    pub fn tight() -> Self {
        Self {
            target: RasterTarget::Tight { max_width: None },
            ..Self::default()
        }
    }

    pub fn with_scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    pub fn with_max_width(mut self, max_width: f32) -> Self {
        self.target = RasterTarget::Tight {
            max_width: Some(max_width),
        };
        self
    }

    pub fn with_vertical_align(mut self, vertical_align: VerticalAlign) -> Self {
        self.vertical_align = vertical_align;
        self
    }

    pub fn with_margin(mut self, margin: f32) -> Self {
        self.margin = margin;
        self
    }
}

impl Default for RasterOptions {
    fn default() -> Self {
        Self {
            target: RasterTarget::Tight { max_width: None },
            scale: 1.0,
            vertical_align: VerticalAlign::default(),
            margin: 0.0,
        }
    }
}
