//! Every audio effect a clip can carry, with its parameters, ranges and
//! defaults.
//!
//! The same shape as `fx::catalog` for pictures: an effect is a `kind` string
//! and a map of numbers, and this table is what gives those numbers meaning.
//! A value missing from a clip's map reads its default here, so tuning a
//! default later moves every clip that never touched it, and a parameter added
//! later changes no existing file.

use serde::Serialize;

/// What the inspector groups an effect under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// Tone and dynamics: equalisers and the compressor.
    Tone,
    /// Space: reverb and echo.
    Space,
    /// Pitch and the voice changer presets.
    Voice,
}

/// One numeric parameter.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ParamSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// What the number is in, for display: "dB", "Hz", "ms", "%", "st", "x",
    /// or "" for a plain factor.
    pub unit: &'static str,
    /// Drawn on a logarithmic slider (frequencies, times).
    pub log: bool,
}

impl ParamSpec {
    pub fn clamp(&self, value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(self.min, self.max)
        } else {
            self.default
        }
    }
}

/// One effect kind.
#[derive(Debug, Clone, Serialize)]
pub struct EffectDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub description: &'static str,
    pub params: &'static [ParamSpec],
}

impl EffectDescriptor {
    pub fn param(&self, id: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.id == id)
    }
}

const fn p(
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
        min,
        max,
        default,
        unit,
        log: false,
    }
}

const fn log(
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
        min,
        max,
        default,
        unit,
        log: true,
    }
}

/// The intensity knob every voice preset has: `1` is the preset as designed,
/// `0` is the clip unchanged.
const INTENSITY: ParamSpec = p("intensity", "Intensity", 0.0, 1.0, 1.0, "%");

pub const EQ3: &str = "eq3";
pub const EQ5: &str = "eq5";
pub const COMPRESSOR: &str = "compressor";
pub const REVERB: &str = "reverb";
pub const DELAY: &str = "delay";
pub const PITCH: &str = "pitch";
pub const VOICE_DEEP: &str = "voice_deep";
pub const VOICE_CHIPMUNK: &str = "voice_chipmunk";
pub const VOICE_ROBOT: &str = "voice_robot";
pub const VOICE_TELEPHONE: &str = "voice_telephone";
pub const VOICE_MEGAPHONE: &str = "voice_megaphone";

/// The voice changer presets, in the order the inspector shows them.
pub const VOICE_PRESETS: [&str; 5] = [
    VOICE_DEEP,
    VOICE_CHIPMUNK,
    VOICE_ROBOT,
    VOICE_TELEPHONE,
    VOICE_MEGAPHONE,
];

static CATALOG: &[EffectDescriptor] = &[
    EffectDescriptor {
        id: EQ3,
        label: "Equalizer",
        group: Group::Tone,
        description: "Low, mid and high: a low shelf at 120 Hz, a bell in the middle, a high shelf at 8 kHz.",
        params: &[
            p("low", "Low", -18.0, 18.0, 0.0, "dB"),
            p("mid", "Mid", -18.0, 18.0, 0.0, "dB"),
            log("mid_freq", "Mid frequency", 200.0, 5_000.0, 1_000.0, "Hz"),
            p("high", "High", -18.0, 18.0, 0.0, "dB"),
        ],
    },
    EffectDescriptor {
        id: EQ5,
        label: "Parametric EQ",
        group: Group::Tone,
        description: "Five bands: a low shelf, three bells and a high shelf, each with its own frequency, gain and width.",
        params: &[
            log("b1_freq", "Band 1 frequency", 20.0, 500.0, 80.0, "Hz"),
            p("b1_gain", "Band 1 gain", -18.0, 18.0, 0.0, "dB"),
            p("b1_q", "Band 1 width", 0.3, 4.0, 0.707, "Q"),
            log("b2_freq", "Band 2 frequency", 60.0, 2_000.0, 250.0, "Hz"),
            p("b2_gain", "Band 2 gain", -18.0, 18.0, 0.0, "dB"),
            p("b2_q", "Band 2 width", 0.3, 8.0, 1.0, "Q"),
            log("b3_freq", "Band 3 frequency", 200.0, 6_000.0, 1_000.0, "Hz"),
            p("b3_gain", "Band 3 gain", -18.0, 18.0, 0.0, "dB"),
            p("b3_q", "Band 3 width", 0.3, 8.0, 1.0, "Q"),
            log("b4_freq", "Band 4 frequency", 800.0, 12_000.0, 4_000.0, "Hz"),
            p("b4_gain", "Band 4 gain", -18.0, 18.0, 0.0, "dB"),
            p("b4_q", "Band 4 width", 0.3, 8.0, 1.0, "Q"),
            log("b5_freq", "Band 5 frequency", 2_000.0, 20_000.0, 12_000.0, "Hz"),
            p("b5_gain", "Band 5 gain", -18.0, 18.0, 0.0, "dB"),
            p("b5_q", "Band 5 width", 0.3, 4.0, 0.707, "Q"),
        ],
    },
    EffectDescriptor {
        id: COMPRESSOR,
        label: "Compressor",
        group: Group::Tone,
        description: "Evens out the level: everything over the threshold is turned down by the ratio.",
        params: &[
            p("threshold", "Threshold", -60.0, 0.0, -18.0, "dB"),
            p("ratio", "Ratio", 1.0, 20.0, 4.0, "x"),
            log("attack", "Attack", 0.1, 200.0, 10.0, "ms"),
            log("release", "Release", 10.0, 2_000.0, 120.0, "ms"),
            p("knee", "Knee", 0.0, 24.0, 6.0, "dB"),
            p("makeup", "Makeup gain", 0.0, 24.0, 0.0, "dB"),
        ],
    },
    EffectDescriptor {
        id: REVERB,
        label: "Reverb",
        group: Group::Space,
        description: "A room around the sound (Freeverb).",
        params: &[
            p("room", "Room size", 0.0, 1.0, 0.5, "%"),
            p("damping", "Damping", 0.0, 1.0, 0.5, "%"),
            p("width", "Width", 0.0, 1.0, 1.0, "%"),
            p("mix", "Mix", 0.0, 1.0, 0.3, "%"),
        ],
    },
    EffectDescriptor {
        id: DELAY,
        label: "Echo",
        group: Group::Space,
        description: "Repeats of the sound, each quieter and darker than the last.",
        params: &[
            log("time", "Time", 20.0, 2_000.0, 300.0, "ms"),
            p("feedback", "Feedback", 0.0, 0.95, 0.35, "%"),
            p("mix", "Mix", 0.0, 1.0, 0.35, "%"),
            log("tone", "Tone", 500.0, 20_000.0, 6_000.0, "Hz"),
        ],
    },
    EffectDescriptor {
        id: PITCH,
        label: "Pitch",
        group: Group::Voice,
        description: "Moves the pitch without changing the speed.",
        params: &[
            p("semitones", "Pitch", -12.0, 12.0, 0.0, "st"),
            p("formant", "Keep voice character", 0.0, 1.0, 1.0, ""),
        ],
    },
    EffectDescriptor {
        id: VOICE_DEEP,
        label: "Deep",
        group: Group::Voice,
        description: "Five semitones down, with the voice's resonances moved down too.",
        params: &[INTENSITY],
    },
    EffectDescriptor {
        id: VOICE_CHIPMUNK,
        label: "Chipmunk",
        group: Group::Voice,
        description: "Seven semitones up, resonances and all.",
        params: &[INTENSITY],
    },
    EffectDescriptor {
        id: VOICE_ROBOT,
        label: "Robot",
        group: Group::Voice,
        description: "A monotone machine voice at 100 Hz.",
        params: &[INTENSITY],
    },
    EffectDescriptor {
        id: VOICE_TELEPHONE,
        label: "Telephone",
        group: Group::Voice,
        description: "The 300–3400 Hz band of a phone line, slightly overdriven.",
        params: &[INTENSITY],
    },
    EffectDescriptor {
        id: VOICE_MEGAPHONE,
        label: "Megaphone",
        group: Group::Voice,
        description: "A narrow, honky, distorted loudhailer.",
        params: &[INTENSITY],
    },
];

pub fn catalog() -> &'static [EffectDescriptor] {
    CATALOG
}

pub fn descriptor(kind: &str) -> Option<&'static EffectDescriptor> {
    CATALOG.iter().find(|d| d.id == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_sits_inside_its_range_and_ids_are_unique() {
        let mut kinds = std::collections::HashSet::new();
        for d in catalog() {
            assert!(kinds.insert(d.id), "{} twice", d.id);
            let mut params = std::collections::HashSet::new();
            for p in d.params {
                assert!(params.insert(p.id), "{}.{} twice", d.id, p.id);
                assert!(p.min < p.max, "{}.{}", d.id, p.id);
                assert!((p.min..=p.max).contains(&p.default), "{}.{}", d.id, p.id);
                assert!(!p.log || p.min > 0.0, "{}.{} log from zero", d.id, p.id);
            }
        }
        for preset in VOICE_PRESETS {
            assert!(descriptor(preset).is_some());
        }
    }
}
