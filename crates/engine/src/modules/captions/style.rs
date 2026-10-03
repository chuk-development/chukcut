//! How captions look: one style for the lane, presets, and placement.
//!
//! A [`CaptionStyle`] is the subset of a title's parameters a caption user
//! changes, plus where on the canvas the caption sits and the karaoke colour.
//! It is applied either to every caption at once or to one, and it is read
//! back from a caption so the panel can show what is there.

use serde::{Deserialize, Serialize};

use crate::modules::project::{CanvasConfig, Segment, TextAlign, TextMaterial, TextShadow};

/// The look of a caption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionStyle {
    /// A system family name; see `text::font` for the fallback rules. The
    /// online font library hooks in here: once a downloaded family is
    /// registered with the text renderer its name is valid like any other.
    pub font_family: String,
    /// Document pixels.
    pub font_size: f32,
    pub color: [f32; 4],
    pub bold: bool,
    pub italic: bool,
    pub align: TextAlign,
    /// 0 for no outline.
    pub stroke_width: f32,
    pub stroke_color: [f32; 4],
    pub shadow: Option<TextShadow>,
    /// The box behind the text, if any.
    pub background: Option<[f32; 4]>,
    /// Centre of the caption, in the segment transform's normalised canvas
    /// units: `[0, 0]` is the middle, `+y` is up, `1` is the edge.
    pub position: [f32; 2],
    /// Karaoke: the colour of the word being spoken. `None` is off.
    pub highlight: Option<[f32; 4]>,
}

/// The three quick placements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Top,
    Middle,
    Bottom,
}

impl Placement {
    pub const ALL: [Placement; 3] = [Placement::Top, Placement::Middle, Placement::Bottom];

    /// The vertical position. Not the very edge: platforms draw their own
    /// buttons and captions over the bottom fifth of a vertical video, and a
    /// caption under them cannot be read.
    pub fn y(self) -> f32 {
        match self {
            Placement::Top => 0.62,
            Placement::Middle => 0.0,
            Placement::Bottom => -0.55,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Placement::Top => "Top",
            Placement::Middle => "Middle",
            Placement::Bottom => "Bottom",
        }
    }

    /// Which placement `y` is, if it is one.
    pub fn of(y: f32) -> Option<Self> {
        Self::ALL.into_iter().find(|p| (p.y() - y).abs() < 0.01)
    }
}

/// Karaoke yellow, the colour every short-form app uses for the spoken word.
pub const HIGHLIGHT_YELLOW: [f32; 4] = [1.0, 0.85, 0.1, 1.0];

impl CaptionStyle {
    /// The default for a canvas: bold white with a black outline, low on the
    /// screen, sized from the short edge like a title.
    pub fn default_for(canvas: &CanvasConfig) -> Self {
        let short_edge = canvas.width.min(canvas.height).max(1) as f32;
        let font_size = (short_edge * 0.065).round().max(8.0);
        Self {
            font_family: crate::modules::text::edit::DEFAULT_FAMILY.to_string(),
            font_size,
            color: [1.0, 1.0, 1.0, 1.0],
            bold: true,
            italic: false,
            align: TextAlign::Center,
            stroke_width: (font_size * 0.07).round().max(1.0),
            stroke_color: [0.0, 0.0, 0.0, 1.0],
            shadow: Some(TextShadow {
                color: [0.0, 0.0, 0.0, 0.5],
                offset: [(font_size / 14.0).round(), (font_size / 14.0).round()],
                blur: (font_size / 10.0).round(),
            }),
            background: None,
            position: [0.0, Placement::Bottom.y()],
            highlight: None,
        }
    }

    /// Named starting points for the panel.
    pub fn presets(canvas: &CanvasConfig) -> Vec<(&'static str, CaptionStyle)> {
        let base = Self::default_for(canvas);
        let size = base.font_size;
        vec![
            ("Classic", base.clone()),
            (
                "Karaoke",
                CaptionStyle {
                    highlight: Some(HIGHLIGHT_YELLOW),
                    ..base.clone()
                },
            ),
            (
                "Yellow",
                CaptionStyle {
                    color: [1.0, 0.86, 0.0, 1.0],
                    ..base.clone()
                },
            ),
            (
                "Boxed",
                CaptionStyle {
                    stroke_width: 0.0,
                    shadow: None,
                    background: Some([0.0, 0.0, 0.0, 0.7]),
                    ..base.clone()
                },
            ),
            (
                "Big",
                CaptionStyle {
                    font_size: (size * 1.5).round(),
                    stroke_width: (size * 1.5 * 0.08).round(),
                    position: [0.0, Placement::Middle.y()],
                    highlight: Some([0.2, 0.9, 0.4, 1.0]),
                    ..base.clone()
                },
            ),
            (
                "Neon",
                CaptionStyle {
                    color: [0.6, 1.0, 1.0, 1.0],
                    stroke_width: 0.0,
                    shadow: Some(TextShadow {
                        color: [0.0, 0.9, 1.0, 0.9],
                        offset: [0.0, 0.0],
                        blur: (size / 3.0).round(),
                    }),
                    ..base.clone()
                },
            ),
            (
                "Minimal",
                CaptionStyle {
                    bold: false,
                    stroke_width: 0.0,
                    shadow: Some(TextShadow {
                        color: [0.0, 0.0, 0.0, 0.8],
                        offset: [0.0, (size / 16.0).round()],
                        blur: (size / 6.0).round(),
                    }),
                    ..base
                },
            ),
        ]
    }

    /// The style a caption has now.
    pub fn of(material: &TextMaterial, segment: &Segment) -> Self {
        Self {
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
            position: segment.transform.position,
            highlight: material.caption.as_ref().and_then(|c| c.highlight),
        }
    }

    /// Write everything but the position — which lives on the segment — into
    /// `material`.
    pub fn apply_to(&self, material: &mut TextMaterial) {
        material.font_family = self.font_family.clone();
        material.font_size = self.font_size;
        material.color = self.color;
        material.bold = self.bold;
        material.italic = self.italic;
        material.align = self.align;
        material.stroke_width = self.stroke_width;
        material.stroke_color = self.stroke_color;
        material.shadow = self.shadow;
        material.background = self.background;
        material
            .caption
            .get_or_insert_with(Default::default)
            .highlight = self.highlight;
    }

    pub fn with_placement(mut self, placement: Placement) -> Self {
        self.position = [self.position[0], placement.y()];
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_style_survives_being_applied_and_read_back() {
        let canvas = CanvasConfig::default();
        let style = CaptionStyle::presets(&canvas)
            .into_iter()
            .find(|(name, _)| *name == "Karaoke")
            .unwrap()
            .1
            .with_placement(Placement::Top);

        let mut material = crate::modules::text::edit::default_material(
            &crate::modules::project::Project::new("t", canvas, 30.0),
            Some("hi".into()),
        );
        style.apply_to(&mut material);
        let mut segment = crate::modules::captions::edit::caption_segment("m", 0, 1_000_000);
        segment.transform.position = style.position;

        let back = CaptionStyle::of(&material, &segment);
        assert_eq!(back.highlight, Some(HIGHLIGHT_YELLOW));
        assert_eq!(back.position, [0.0, Placement::Top.y()]);
        assert_eq!(back.font_size, style.font_size);
        assert!(
            material.caption.is_some(),
            "applying a style makes a caption"
        );
        assert_eq!(Placement::of(back.position[1]), Some(Placement::Top));
    }

    #[test]
    fn the_default_scales_with_the_canvas() {
        let small = CaptionStyle::default_for(&CanvasConfig {
            width: 1080,
            height: 1920,
            background: [0.0; 4],
        });
        let large = CaptionStyle::default_for(&CanvasConfig {
            width: 3840,
            height: 2160,
            background: [0.0; 4],
        });
        assert!(large.font_size > small.font_size * 1.9);
        assert!(small.position[1] < 0.0, "captions start low on the screen");
    }
}
