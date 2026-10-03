//! Every built-in effect, with its parameters, defaults and ranges.
//!
//! This table is the one place an effect's controls are described. The
//! inspector builds its rows from it, the asset panel its tiles, the edit
//! builders validate against it and the renderer reads defaults from it. A
//! value in the document is in the units written here — mostly `0..100`
//! sliders, degrees for angles, seconds for periods — and the renderer turns
//! those into pixels of the frame it is drawing, so the preview (small) and
//! the export (large) look the same.

use serde::Serialize;

/// Where a tile is listed in the asset panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Blur,
    Light,
    Motion,
    Retro,
    Distort,
    Film,
    Layout,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Blur,
        Category::Light,
        Category::Motion,
        Category::Retro,
        Category::Distort,
        Category::Film,
        Category::Layout,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Blur => "Blur",
            Category::Light => "Light",
            Category::Motion => "Motion",
            Category::Retro => "Retro",
            Category::Distort => "Distort",
            Category::Film => "Film",
            Category::Layout => "Layout",
        }
    }
}

/// What kind of control a parameter is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamKind {
    /// A number with a slider. Keyframable.
    Number {
        min: f32,
        max: f32,
        step: f32,
        default: f32,
        /// Shown after the value: `""`, `"°"`, `"s"`, `"%"`.
        unit: &'static str,
    },
    /// An RGBA colour, linear, straight alpha. Not keyframable.
    Color { default: [f32; 4] },
    /// One of a few named options, stored as its index.
    Choice {
        options: &'static [&'static str],
        default: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ParamSpec {
    /// The key in `EffectMaterial::params`. Never renamed once shipped.
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
}

impl ParamSpec {
    pub fn keyframable(&self) -> bool {
        matches!(self.kind, ParamKind::Number { .. })
    }

    /// The default as a number: a number's default, a choice's index.
    pub fn default_number(&self) -> f32 {
        match self.kind {
            ParamKind::Number { default, .. } => default,
            ParamKind::Choice { default, .. } => default as f32,
            ParamKind::Color { .. } => 0.0,
        }
    }

    pub fn default_color(&self) -> [f32; 4] {
        match self.kind {
            ParamKind::Color { default } => default,
            _ => [0.0, 0.0, 0.0, 0.0],
        }
    }

    /// `value` brought into this parameter's range. Choices round to an
    /// index. Colours are clamped to `0..1` per channel.
    pub fn clamp(&self, value: f32) -> f32 {
        match self.kind {
            ParamKind::Number { min, max, .. } => value.clamp(min, max),
            ParamKind::Choice { options, .. } => value
                .round()
                .clamp(0.0, options.len().saturating_sub(1) as f32),
            ParamKind::Color { .. } => value.clamp(0.0, 1.0),
        }
    }
}

/// One effect the renderer implements.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EffectDescriptor {
    /// `EffectMaterial::kind`. Never renamed once shipped.
    pub id: &'static str,
    /// Sentence case, no trailing period.
    pub label: &'static str,
    pub category: Category,
    /// One line for a tooltip.
    pub description: &'static str,
    pub params: &'static [ParamSpec],
}

impl EffectDescriptor {
    pub fn param(&self, id: &str) -> Option<&'static ParamSpec> {
        self.params.iter().find(|p| p.id == id)
    }
}

const fn number(
    id: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
) -> ParamSpec {
    ParamSpec {
        id,
        label,
        kind: ParamKind::Number {
            min,
            max,
            step: 1.0,
            default,
            unit,
        },
    }
}

const fn slider(id: &'static str, label: &'static str, default: f32) -> ParamSpec {
    number(id, label, 0.0, 100.0, default, "")
}

const fn color(id: &'static str, label: &'static str, default: [f32; 4]) -> ParamSpec {
    ParamSpec {
        id,
        label,
        kind: ParamKind::Color { default },
    }
}

const fn choice(
    id: &'static str,
    label: &'static str,
    options: &'static [&'static str],
    default: u32,
) -> ParamSpec {
    ParamSpec {
        id,
        label,
        kind: ParamKind::Choice { options, default },
    }
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

pub const GAUSSIAN_BLUR: &str = "gaussian_blur";
pub const ZOOM_BLUR: &str = "zoom_blur";
pub const GLOW: &str = "glow";
pub const LIGHT_SWEEP: &str = "light_sweep";
pub const SHAKE: &str = "shake";
pub const RGB_SPLIT: &str = "rgb_split";
pub const GLITCH: &str = "glitch";
pub const VHS: &str = "vhs";
pub const PIXELATE: &str = "pixelate";
pub const MIRROR: &str = "mirror";
pub const KALEIDOSCOPE: &str = "kaleidoscope";
pub const GRAIN: &str = "film_grain";
pub const HALATION: &str = "halation";
pub const BLOOM: &str = "bloom";
pub const GATE_WEAVE: &str = "gate_weave";
pub const LETTERBOX: &str = "letterbox";
pub const FRAME: &str = "frame";

/// Mirror modes, in `choice` index order.
pub const MIRROR_MODES: &[&str] = &[
    "Left to right",
    "Right to left",
    "Top to bottom",
    "Bottom to top",
    "Four ways",
];

/// What the glow measures to decide what glows.
pub const GLOW_SOURCES: &[&str] = &["Brightness", "Red", "Green", "Blue"];

static EFFECTS: &[EffectDescriptor] = &[
    EffectDescriptor {
        id: GAUSSIAN_BLUR,
        label: "Blur",
        category: Category::Blur,
        description: "Softens the whole picture evenly.",
        params: &[slider("radius", "Radius", 20.0)],
    },
    EffectDescriptor {
        id: ZOOM_BLUR,
        label: "Zoom blur",
        category: Category::Blur,
        description: "Streaks the picture outwards from a centre, like a fast zoom.",
        params: &[
            slider("strength", "Strength", 30.0),
            number("center_x", "Centre X", -100.0, 100.0, 0.0, ""),
            number("center_y", "Centre Y", -100.0, 100.0, 0.0, ""),
        ],
    },
    EffectDescriptor {
        id: GLOW,
        label: "Glow",
        category: Category::Light,
        description: "Bright parts bleed light into their surroundings.",
        params: &[
            slider("intensity", "Intensity", 60.0),
            slider("threshold", "Threshold", 65.0),
            slider("radius", "Radius", 40.0),
            choice("source", "Glow from", GLOW_SOURCES, 0),
            color("tint", "Colour", WHITE),
        ],
    },
    EffectDescriptor {
        id: LIGHT_SWEEP,
        label: "Light sweep",
        category: Category::Light,
        description: "A band of light sweeps across the picture.",
        params: &[
            slider("intensity", "Intensity", 60.0),
            slider("width", "Width", 25.0),
            number("angle", "Angle", -90.0, 90.0, 30.0, "°"),
            number("period", "Period", 0.2, 6.0, 1.5, "s"),
            color("color", "Colour", WHITE),
        ],
    },
    EffectDescriptor {
        id: SHAKE,
        label: "Shake",
        category: Category::Motion,
        description: "Camera shake: the picture jolts about, the same way every time.",
        params: &[
            slider("amplitude", "Amplitude", 30.0),
            number("frequency", "Frequency", 1.0, 30.0, 10.0, "Hz"),
            slider("rotation", "Rotation", 20.0),
            slider("zoom", "Zoom", 10.0),
        ],
    },
    EffectDescriptor {
        id: RGB_SPLIT,
        label: "RGB split",
        category: Category::Retro,
        description: "Red and blue drift apart: chromatic aberration.",
        params: &[
            slider("amount", "Amount", 30.0),
            number("angle", "Angle", -180.0, 180.0, 0.0, "°"),
        ],
    },
    EffectDescriptor {
        id: GLITCH,
        label: "Glitch",
        category: Category::Retro,
        description: "Blocks of the picture jump sideways and the colours tear.",
        params: &[
            slider("intensity", "Intensity", 50.0),
            slider("block_size", "Block size", 35.0),
            number("speed", "Speed", 1.0, 30.0, 12.0, "/s"),
            slider("color_shift", "Colour shift", 40.0),
        ],
    },
    EffectDescriptor {
        id: VHS,
        label: "VHS",
        category: Category::Retro,
        description: "Tape: wobbling lines, bleeding colour, scanlines and noise.",
        params: &[
            slider("intensity", "Intensity", 60.0),
            slider("noise", "Noise", 35.0),
            slider("jitter", "Jitter", 30.0),
            slider("scanlines", "Scanlines", 45.0),
            slider("bleed", "Colour bleed", 50.0),
        ],
    },
    EffectDescriptor {
        id: PIXELATE,
        label: "Pixelate",
        category: Category::Retro,
        description: "Big square pixels.",
        params: &[slider("size", "Size", 30.0)],
    },
    EffectDescriptor {
        id: MIRROR,
        label: "Mirror",
        category: Category::Distort,
        description: "One half of the picture reflected onto the other.",
        params: &[choice("mode", "Mode", MIRROR_MODES, 0)],
    },
    EffectDescriptor {
        id: KALEIDOSCOPE,
        label: "Kaleidoscope",
        category: Category::Distort,
        description: "The picture folded into a ring of mirrored wedges.",
        params: &[
            number("segments", "Segments", 2.0, 16.0, 6.0, ""),
            number("rotation", "Rotation", 0.0, 360.0, 0.0, "°"),
            number("zoom", "Zoom", 25.0, 400.0, 100.0, "%"),
        ],
    },
    EffectDescriptor {
        id: GRAIN,
        label: "Grain",
        category: Category::Film,
        description: "Film grain, different on every frame.",
        params: &[
            slider("amount", "Amount", 35.0),
            number("size", "Size", 1.0, 4.0, 1.0, "px"),
        ],
    },
    EffectDescriptor {
        id: HALATION,
        label: "Halation",
        category: Category::Film,
        description: "The red-orange halo film draws around bright highlights.",
        params: &[
            slider("amount", "Amount", 50.0),
            slider("threshold", "Threshold", 70.0),
            slider("radius", "Radius", 45.0),
            color("color", "Colour", [1.0, 0.25, 0.08, 1.0]),
        ],
    },
    EffectDescriptor {
        id: BLOOM,
        label: "Bloom",
        category: Category::Film,
        description: "A soft, wide haze of light around the bright areas.",
        params: &[
            slider("amount", "Amount", 40.0),
            slider("threshold", "Threshold", 55.0),
            slider("radius", "Radius", 70.0),
        ],
    },
    EffectDescriptor {
        id: GATE_WEAVE,
        label: "Gate weave",
        category: Category::Film,
        description: "The slow drift of film moving through a projector gate.",
        params: &[
            slider("amount", "Amount", 30.0),
            slider("speed", "Speed", 30.0),
        ],
    },
    EffectDescriptor {
        id: LETTERBOX,
        label: "Letterbox",
        category: Category::Film,
        description: "Bars that crop the frame to a cinema shape.",
        params: &[
            number("aspect", "Aspect", 1.0, 3.0, 2.39, ":1"),
            slider("opacity", "Opacity", 100.0),
            color("color", "Colour", BLACK),
        ],
    },
    EffectDescriptor {
        id: FRAME,
        label: "Frame",
        category: Category::Layout,
        description: "Rounded corners, a border and a drop shadow, for picture in picture.",
        params: &[
            slider("radius", "Corner radius", 12.0),
            slider("border", "Border", 0.0),
            color("border_color", "Border colour", WHITE),
            slider("shadow", "Shadow", 0.0),
            slider("shadow_blur", "Shadow blur", 30.0),
            slider("shadow_distance", "Shadow distance", 20.0),
            number("shadow_angle", "Shadow angle", -180.0, 180.0, 135.0, "°"),
            color("shadow_color", "Shadow colour", BLACK),
        ],
    },
];

/// Every effect, in the order the asset panel lists them.
pub fn catalog() -> &'static [EffectDescriptor] {
    EFFECTS
}

/// The descriptor of `kind`, or `None` for a kind this build does not know.
pub fn descriptor(kind: &str) -> Option<&'static EffectDescriptor> {
    EFFECTS.iter().find(|d| d.id == kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_every_default_is_in_range() {
        let mut ids = HashSet::new();
        for effect in catalog() {
            assert!(ids.insert(effect.id), "{} listed twice", effect.id);
            let mut params = HashSet::new();
            for param in effect.params {
                assert!(params.insert(param.id), "{}.{} twice", effect.id, param.id);
                let d = param.default_number();
                assert_eq!(
                    param.clamp(d),
                    d,
                    "{}.{} default out of range",
                    effect.id,
                    param.id
                );
            }
        }
    }

    #[test]
    fn every_category_has_something_in_it() {
        for category in Category::ALL {
            assert!(
                catalog().iter().any(|e| e.category == category),
                "{category:?} is empty"
            );
        }
    }
}
