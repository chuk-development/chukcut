//! Keyframe-free animation, as the document stores it.
//!
//! A clip's In, Out and Combo animation, its per-letter text animation and
//! its punch-in zoom are **parameters**, not keyframes. The document says
//! "slide up, 0.5 s, back easing" and the compositor works out every frame
//! from the clip's *current* start and end. That is the whole design, and it
//! is the lesson `docs/research/resolve-plugins.md` §6.3 draws from Magic
//! Animate and Neo Zoom: baked keyframes break the moment a clip is trimmed,
//! and a parameter does not. Trim the tail of a clip and its Out animation
//! moves with the new end; split it and the In stays on the left half while
//! the Out goes with the right (see `motion::edit::split_commands`).
//!
//! ## Where it lives
//!
//! One [`AnimationMaterial`] per animated segment, in
//! `MaterialPool::animations`, referenced from `Segment::extras` — the same
//! typed-pool-category arrangement as transitions and colour adjustments, for
//! the reasons `TransitionMaterial` writes down: no `Segment` field (every
//! struct literal in the tree keeps compiling), `RemoveSegment` restores the
//! reference for free, and the compositor reads typed fields on every frame
//! instead of parsing JSON.
//!
//! A material is treated as **immutable**: every change mints a new id and
//! swaps the reference through `EditCommand::SetAnimation`. So a material
//! that two segments share — a duplicated clip — is never changed under the
//! other one's feet.
//!
//! ## Defaults
//!
//! Every field has a serde default, so a project written before a field
//! existed opens unchanged, and a project written before animations existed
//! has no `animations` key at all and opens with the pool empty.

use serde::{Deserialize, Serialize};

use super::document::{Id, Micros};

/// The length an In or Out animation gets when the user has not said.
pub const DEFAULT_ANIMATION_DURATION: Micros = 500_000;
/// The period a Combo animation loops with when the user has not said.
pub const DEFAULT_COMBO_PERIOD: Micros = 1_000_000;
/// The shortest In, Out, period or text window the engine accepts. Shorter is
/// a division by a number that is nearly zero and looks like a glitch.
pub const MIN_ANIMATION_DURATION: Micros = 33_000;
/// The longest. Long enough for a slow title; short enough that a typo in a
/// number box (50 instead of 5.0 seconds) is refused rather than obeyed.
pub const MAX_ANIMATION_DURATION: Micros = 60_000_000;

/// Everything that animates one segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimationMaterial {
    pub id: Id,
    /// Plays from the clip's first frame. Serialized as `in`, CapCut's word.
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub intro: Option<ClipAnimation>,
    /// Plays into the clip's last frame.
    #[serde(default, rename = "out", skip_serializing_if = "Option::is_none")]
    pub outro: Option<ClipAnimation>,
    /// Loops for the clip's whole length; `duration` is the period.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combo: Option<ClipAnimation>,
    /// Per-letter, word or line entrance of a text clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_in: Option<TextAnimator>,
    /// Per-letter, word or line exit of a text clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_out: Option<TextAnimator>,
    /// A punch-in zoom about a pivot, from the clip's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<PunchZoom>,
}

impl AnimationMaterial {
    /// An empty material with a fresh id.
    pub fn new() -> Self {
        Self {
            id: super::document::new_id(),
            intro: None,
            outro: None,
            combo: None,
            text_in: None,
            text_out: None,
            zoom: None,
        }
    }

    /// Whether nothing in it animates anything. An empty material is never
    /// stored: the edit that empties one removes the reference instead.
    pub fn is_empty(&self) -> bool {
        self.intro.is_none()
            && self.outro.is_none()
            && self.combo.is_none()
            && self.text_in.is_none()
            && self.text_out.is_none()
            && self.zoom.is_none()
    }

    /// The same parameters, ignoring the id.
    pub fn same_parameters(&self, other: &AnimationMaterial) -> bool {
        self.intro == other.intro
            && self.outro == other.outro
            && self.combo == other.combo
            && self.text_in == other.text_in
            && self.text_out == other.text_out
            && self.zoom == other.zoom
    }

    /// The first value that is not a finite number or is out of range, as a
    /// user-facing sentence. Checked at the edit boundary for the reason every
    /// other float is: a NaN saves as `null` and the project never reopens.
    pub fn problem(&self) -> Option<String> {
        for (name, slot) in [
            ("In", &self.intro),
            ("Out", &self.outro),
            ("Combo", &self.combo),
        ] {
            if let Some(a) = slot {
                if !a.strength.is_finite() {
                    return Some(format!("the {name} animation's strength must be a number"));
                }
                if !(MIN_ANIMATION_DURATION..=MAX_ANIMATION_DURATION).contains(&a.duration) {
                    return Some(format!(
                        "the {name} animation must last between 0.033 and 60 seconds"
                    ));
                }
            }
        }
        for (name, slot) in [("text In", &self.text_in), ("text Out", &self.text_out)] {
            if let Some(a) = slot {
                if !a.strength.is_finite() || !a.overlap.is_finite() {
                    return Some(format!(
                        "the {name} animation has a value that is not a number"
                    ));
                }
                if !(MIN_ANIMATION_DURATION..=MAX_ANIMATION_DURATION).contains(&a.duration) {
                    return Some(format!(
                        "the {name} animation must last between 0.033 and 60 seconds"
                    ));
                }
            }
        }
        if let Some(zoom) = &self.zoom {
            if !zoom.amount.is_finite() || !zoom.pivot.iter().all(|v| v.is_finite()) {
                return Some("the zoom has a value that is not a number".into());
            }
            if !(0.1..=10.0).contains(&zoom.amount) {
                return Some("the zoom must be between 10% and 1000%".into());
            }
            if !(0..=MAX_ANIMATION_DURATION).contains(&zoom.duration) {
                return Some("the zoom ramp must last between 0 and 60 seconds".into());
            }
        }
        None
    }
}

impl Default for AnimationMaterial {
    fn default() -> Self {
        Self::new()
    }
}

/// Which of a clip's three animation slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationSlot {
    In,
    Out,
    Combo,
}

/// One preset in one slot, with its timing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClipAnimation {
    pub preset: AnimationPreset,
    /// In and Out: how long the animation takes. Combo: one period of the loop.
    #[serde(default = "default_duration")]
    pub duration: Micros,
    #[serde(default)]
    pub easing: Ease,
    /// Scales how far the preset moves: distance, angle, zoom, blur. 1 is
    /// the preset as designed.
    #[serde(default = "one")]
    pub strength: f32,
}

/// A clip animation preset. One enum for all three slots; which slots a
/// preset belongs to is in `motion::catalog`. Serialized by name, so the
/// order here is free to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationPreset {
    // --- In and Out ---------------------------------------------------------
    Fade,
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    ZoomIn,
    ZoomOut,
    Pop,
    Bounce,
    Spin,
    Blur,
    WipeLeft,
    WipeRight,
    WipeUp,
    WipeDown,
    Swing,
    Shake,
    Rise,
    Flip,
    Whip,
    // --- Combo --------------------------------------------------------------
    Pulse,
    Heartbeat,
    Wobble,
    Rock,
    Float,
    Jitter,
    Rotate,
    Flicker,
}

/// The shared easing library.
///
/// A curve maps progress `0..1` to `0..1` at the ends; the overshooting ones
/// (back, elastic) leave that range in between, which is the point of them.
/// Separate from the keyframe [`super::document::Easing`] on purpose: that
/// enum is matched exhaustively by the keyframe editor, and adding curves to
/// it is a decision for the keyframe UI. [`Ease::from`] maps every keyframe
/// easing onto this library, so the two never disagree about a shared name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    Linear,
    /// Cubic, accelerating.
    EaseIn,
    /// Cubic, decelerating. The default: things arrive and settle.
    #[default]
    EaseOut,
    /// Cubic, both.
    EaseInOut,
    /// Sine in-out: gentler than the cubic.
    Smooth,
    /// Exponential out: a fast start and a long settle. Whips and snaps.
    Snap,
    /// Pulls back before it goes (anticipation).
    BackIn,
    /// Overshoots and comes back.
    Back,
    /// Both of the above.
    BackInOut,
    /// Springs past the end and rings down.
    Elastic,
    /// Lands and bounces.
    Bounce,
}

impl Ease {
    pub const ALL: [Ease; 11] = [
        Ease::Linear,
        Ease::EaseIn,
        Ease::EaseOut,
        Ease::EaseInOut,
        Ease::Smooth,
        Ease::Snap,
        Ease::BackIn,
        Ease::Back,
        Ease::BackInOut,
        Ease::Elastic,
        Ease::Bounce,
    ];

    /// The curve at `t`, which is clamped into `0..1`. `0` maps to `0` and
    /// `1` to `1` exactly for every curve.
    pub fn apply(self, t: f32) -> f32 {
        use std::f32::consts::PI;
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        // Penner's constants: 1.70158 overshoots by 10 %.
        const C1: f32 = 1.70158;
        const C2: f32 = C1 * 1.525;
        const C3: f32 = C1 + 1.0;
        match self {
            Ease::Linear => t,
            Ease::EaseIn => t * t * t,
            Ease::EaseOut => 1.0 - (1.0 - t).powi(3),
            Ease::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Ease::Smooth => -((PI * t).cos() - 1.0) / 2.0,
            Ease::Snap => 1.0 - 2f32.powf(-10.0 * t),
            Ease::BackIn => C3 * t * t * t - C1 * t * t,
            Ease::Back => 1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2),
            Ease::BackInOut => {
                if t < 0.5 {
                    ((2.0 * t).powi(2) * ((C2 + 1.0) * 2.0 * t - C2)) / 2.0
                } else {
                    ((2.0 * t - 2.0).powi(2) * ((C2 + 1.0) * (t * 2.0 - 2.0) + C2) + 2.0) / 2.0
                }
            }
            Ease::Elastic => {
                let c4 = (2.0 * PI) / 3.0;
                2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin() + 1.0
            }
            Ease::Bounce => bounce_out(t),
        }
    }

    /// The name a picker shows.
    pub fn label(self) -> &'static str {
        match self {
            Ease::Linear => "Linear",
            Ease::EaseIn => "Ease in",
            Ease::EaseOut => "Ease out",
            Ease::EaseInOut => "Ease in-out",
            Ease::Smooth => "Smooth",
            Ease::Snap => "Snap",
            Ease::BackIn => "Anticipate",
            Ease::Back => "Overshoot",
            Ease::BackInOut => "Overshoot both",
            Ease::Elastic => "Elastic",
            Ease::Bounce => "Bounce",
        }
    }
}

impl From<super::document::Easing> for Ease {
    fn from(easing: super::document::Easing) -> Self {
        use super::document::Easing;
        match easing {
            // A hold has no curve; linear is what a preset can do with it.
            Easing::Hold | Easing::Linear => Ease::Linear,
            Easing::EaseIn => Ease::EaseIn,
            Easing::EaseOut => Ease::EaseOut,
            Easing::EaseInOut => Ease::EaseInOut,
            Easing::Curve(ease) => ease,
            // A drawn curve has no library name; the nearest is linear.
            Easing::Bezier { .. } => Ease::Linear,
        }
    }
}

fn bounce_out(t: f32) -> f32 {
    const N1: f32 = 7.5625;
    const D1: f32 = 2.75;
    if t < 1.0 / D1 {
        N1 * t * t
    } else if t < 2.0 / D1 {
        let t = t - 1.5 / D1;
        N1 * t * t + 0.75
    } else if t < 2.5 / D1 {
        let t = t - 2.25 / D1;
        N1 * t * t + 0.9375
    } else {
        let t = t - 2.625 / D1;
        N1 * t * t + 0.984375
    }
}

// ---------------------------------------------------------------------------
// Text animator
// ---------------------------------------------------------------------------

/// Which slot of a text clip's per-unit animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSlot {
    In,
    Out,
}

/// What a text animation staggers over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextUnit {
    /// One shaped cluster: a letter, a ligature, an emoji.
    #[default]
    Letter,
    Word,
    Line,
}

/// In which order the units start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaggerOrder {
    /// Reading order.
    #[default]
    Forward,
    Backward,
    /// From the middle out to both ends.
    Centre,
    /// Shuffled, by `TextAnimator::seed`, so a render is repeatable.
    Random,
}

/// A per-unit text animation preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextPreset {
    /// Each unit appears at once, in turn.
    Typewriter,
    Fade,
    /// Fades in while rising half a line.
    FadeUp,
    /// Grows from nothing, overshooting.
    Pop,
    /// Slides up a whole line, fading.
    SlideUp,
    /// Falls in from above and lands.
    Drop,
    /// Shrinks in from twice its size.
    Zoom,
    /// Turns in from half a turn, growing.
    Spin,
}

/// A per-letter, word or line animation of a text clip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TextAnimator {
    pub preset: TextPreset,
    #[serde(default)]
    pub unit: TextUnit,
    #[serde(default)]
    pub order: StaggerOrder,
    /// The shuffle for [`StaggerOrder::Random`]; ignored by the others.
    #[serde(default)]
    pub seed: u32,
    /// The whole window, first unit's start to last unit's end.
    #[serde(default = "default_duration")]
    pub duration: Micros,
    /// How much neighbouring units overlap: 0 is strictly one after another,
    /// 1 is all together.
    #[serde(default = "half")]
    pub overlap: f32,
    #[serde(default)]
    pub easing: Ease,
    /// Scales the distance, size and angle a unit travels.
    #[serde(default = "one")]
    pub strength: f32,
}

// ---------------------------------------------------------------------------
// Punch-in zoom
// ---------------------------------------------------------------------------

/// A zoom about a pivot: the short-form "energy" move, and what auto zoom
/// writes on alternate jump cuts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PunchZoom {
    /// Scale reached, 1.1 is 110 %.
    #[serde(default = "default_zoom_amount")]
    pub amount: f32,
    /// The point that stays still, in the same normalized canvas units as
    /// `Transform::position`: `0,0` is the centre, `1` half the canvas, +y up.
    #[serde(default)]
    pub pivot: [f32; 2],
    /// How long the push takes from the clip's start; 0 is a hard punch.
    #[serde(default)]
    pub duration: Micros,
    #[serde(default)]
    pub easing: Ease,
}

impl Default for PunchZoom {
    fn default() -> Self {
        Self {
            amount: default_zoom_amount(),
            pivot: [0.0, 0.0],
            duration: 0,
            easing: Ease::EaseOut,
        }
    }
}

fn default_duration() -> Micros {
    DEFAULT_ANIMATION_DURATION
}

fn default_zoom_amount() -> f32 {
    1.1
}

fn one() -> f32 {
    1.0
}

fn half() -> f32 {
    0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_curve_starts_at_zero_and_ends_at_one() {
        for ease in Ease::ALL {
            assert_eq!(ease.apply(0.0), 0.0, "{ease:?}");
            assert_eq!(ease.apply(1.0), 1.0, "{ease:?}");
            assert_eq!(ease.apply(-3.0), 0.0, "{ease:?}");
            assert_eq!(ease.apply(f32::NAN), 0.0, "{ease:?}");
            // Continuous at the ends: no curve jumps on its first or last step.
            assert!(ease.apply(0.001).abs() < 0.05, "{ease:?}");
            assert!((ease.apply(0.999) - 1.0).abs() < 0.05, "{ease:?}");
        }
    }

    #[test]
    fn overshooting_curves_overshoot_and_the_plain_ones_do_not() {
        let peak = |e: Ease| {
            (1..100)
                .map(|i| e.apply(i as f32 / 100.0))
                .fold(0.0, f32::max)
        };
        assert!(peak(Ease::Back) > 1.05);
        assert!(peak(Ease::Elastic) > 1.05);
        for e in [Ease::Linear, Ease::EaseOut, Ease::EaseInOut, Ease::Bounce] {
            assert!(peak(e) <= 1.0 + 1e-5, "{e:?}");
        }
        // Anticipation dips below zero before it goes.
        let low = (1..100)
            .map(|i| Ease::BackIn.apply(i as f32 / 100.0))
            .fold(1.0, f32::min);
        assert!(low < -0.05);
    }

    #[test]
    fn an_old_project_without_the_new_fields_still_reads() {
        let json = r#"{"id":"a","in":{"preset":"fade"}}"#;
        let material: AnimationMaterial = serde_json::from_str(json).unwrap();
        let intro = material.intro.unwrap();
        assert_eq!(intro.duration, DEFAULT_ANIMATION_DURATION);
        assert_eq!(intro.easing, Ease::EaseOut);
        assert_eq!(intro.strength, 1.0);
        assert!(material.outro.is_none() && material.zoom.is_none());

        let zoom: PunchZoom = serde_json::from_str("{}").unwrap();
        assert_eq!(zoom, PunchZoom::default());
        let text: TextAnimator = serde_json::from_str(r#"{"preset":"pop"}"#).unwrap();
        assert_eq!(text.unit, TextUnit::Letter);
        assert_eq!(text.overlap, 0.5);
    }

    #[test]
    fn an_empty_slot_is_not_written() {
        let mut material = AnimationMaterial::new();
        material.intro = Some(ClipAnimation {
            preset: AnimationPreset::Pop,
            duration: 400_000,
            easing: Ease::Back,
            strength: 1.0,
        });
        let json = serde_json::to_string(&material).unwrap();
        assert!(json.contains(r#""in":"#));
        assert!(!json.contains("out"));
        assert!(!json.contains("zoom"));
        let back: AnimationMaterial = serde_json::from_str(&json).unwrap();
        assert_eq!(back, material);
    }

    #[test]
    fn out_of_range_values_are_named() {
        let mut material = AnimationMaterial::new();
        material.zoom = Some(PunchZoom {
            amount: f32::NAN,
            ..PunchZoom::default()
        });
        assert!(material.problem().unwrap().contains("zoom"));
        material.zoom = None;
        material.combo = Some(ClipAnimation {
            preset: AnimationPreset::Pulse,
            duration: 0,
            easing: Ease::Linear,
            strength: 1.0,
        });
        assert!(material.problem().unwrap().contains("Combo"));
    }
}
