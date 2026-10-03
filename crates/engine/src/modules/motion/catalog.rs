//! The presets a UI offers, with their defaults.
//!
//! In Rust rather than in the app, for the reason `transitions::catalog`
//! gives: two copies of a list drift. A preset added here appears in the
//! Animation tab without the app changing.

use serde::Serialize;

use crate::modules::project::animation::{
    AnimationPreset, AnimationSlot, ClipAnimation, Ease, StaggerOrder, TextAnimator, TextPreset,
    TextUnit, DEFAULT_ANIMATION_DURATION, DEFAULT_COMBO_PERIOD,
};
use crate::modules::project::document::Micros;

#[derive(Debug, Clone, Serialize)]
pub struct PresetDescriptor {
    pub preset: AnimationPreset,
    /// Sentence case, no period.
    pub label: &'static str,
    /// Offered in the In and Out tabs.
    pub in_out: bool,
    /// Offered in the Combo tab.
    pub combo: bool,
    pub easing: Ease,
    /// Default length in In/Out; default period in Combo.
    pub duration: Micros,
}

impl PresetDescriptor {
    /// Whether this preset belongs in `slot`'s tab.
    pub fn fits(&self, slot: AnimationSlot) -> bool {
        match slot {
            AnimationSlot::In | AnimationSlot::Out => self.in_out,
            AnimationSlot::Combo => self.combo,
        }
    }

    /// The animation this preset makes with its defaults.
    pub fn animation(&self) -> ClipAnimation {
        ClipAnimation {
            preset: self.preset,
            duration: self.duration,
            easing: self.easing,
            strength: 1.0,
        }
    }
}

pub fn clip_presets() -> Vec<PresetDescriptor> {
    use AnimationPreset as P;
    let io = |preset, label, easing| PresetDescriptor {
        preset,
        label,
        in_out: true,
        combo: false,
        easing,
        duration: DEFAULT_ANIMATION_DURATION,
    };
    let lp = |preset, label, easing, duration| PresetDescriptor {
        preset,
        label,
        in_out: false,
        combo: true,
        easing,
        duration,
    };
    vec![
        io(P::Fade, "Fade", Ease::EaseOut),
        io(P::SlideLeft, "Slide left", Ease::EaseOut),
        io(P::SlideRight, "Slide right", Ease::EaseOut),
        io(P::SlideUp, "Slide up", Ease::EaseOut),
        io(P::SlideDown, "Slide down", Ease::EaseOut),
        io(P::ZoomIn, "Zoom in", Ease::EaseOut),
        io(P::ZoomOut, "Zoom out", Ease::EaseOut),
        io(P::Pop, "Pop", Ease::Back),
        io(P::Bounce, "Bounce", Ease::Bounce),
        io(P::Spin, "Spin", Ease::EaseOut),
        io(P::Blur, "Blur", Ease::EaseOut),
        io(P::WipeRight, "Wipe right", Ease::EaseInOut),
        io(P::WipeLeft, "Wipe left", Ease::EaseInOut),
        io(P::WipeUp, "Wipe up", Ease::EaseInOut),
        io(P::WipeDown, "Wipe down", Ease::EaseInOut),
        io(P::Swing, "Swing", Ease::Elastic),
        io(P::Shake, "Shake", Ease::EaseOut),
        io(P::Rise, "Rise", Ease::EaseOut),
        io(P::Flip, "Flip", Ease::Back),
        io(P::Whip, "Whip", Ease::Snap),
        lp(P::Pulse, "Pulse", Ease::Smooth, DEFAULT_COMBO_PERIOD),
        lp(P::Heartbeat, "Heartbeat", Ease::Linear, 1_200_000),
        lp(P::Wobble, "Wobble", Ease::Linear, 800_000),
        lp(P::Rock, "Rock", Ease::Linear, 1_500_000),
        lp(P::Float, "Float", Ease::Linear, 2_000_000),
        lp(P::Jitter, "Jitter", Ease::Linear, DEFAULT_COMBO_PERIOD),
        lp(P::Rotate, "Rotate", Ease::Linear, 3_000_000),
        lp(P::Flicker, "Flicker", Ease::Linear, DEFAULT_COMBO_PERIOD),
    ]
}

/// The descriptor of one preset.
pub fn clip_preset(preset: AnimationPreset) -> PresetDescriptor {
    clip_presets()
        .into_iter()
        .find(|d| d.preset == preset)
        .expect("every preset is in the catalog")
}

#[derive(Debug, Clone, Serialize)]
pub struct TextPresetDescriptor {
    pub preset: TextPreset,
    pub label: &'static str,
    /// The animator this preset makes with its defaults.
    pub animator: TextAnimator,
}

pub fn text_presets() -> Vec<TextPresetDescriptor> {
    use TextPreset as T;
    let make = |preset, label, unit, overlap, easing, duration| TextPresetDescriptor {
        preset,
        label,
        animator: TextAnimator {
            preset,
            unit,
            order: StaggerOrder::Forward,
            seed: 0,
            duration,
            overlap,
            easing,
            strength: 1.0,
        },
    };
    vec![
        make(
            T::Typewriter,
            "Typewriter",
            TextUnit::Letter,
            0.0,
            Ease::Linear,
            1_200_000,
        ),
        make(
            T::FadeUp,
            "Fade up by word",
            TextUnit::Word,
            0.5,
            Ease::EaseOut,
            900_000,
        ),
        make(
            T::Pop,
            "Pop by word",
            TextUnit::Word,
            0.3,
            Ease::Back,
            900_000,
        ),
        make(
            T::SlideUp,
            "Slide by line",
            TextUnit::Line,
            0.4,
            Ease::EaseOut,
            800_000,
        ),
        make(
            T::Fade,
            "Fade by letter",
            TextUnit::Letter,
            0.7,
            Ease::EaseOut,
            1_000_000,
        ),
        make(
            T::Drop,
            "Drop by letter",
            TextUnit::Letter,
            0.6,
            Ease::Bounce,
            1_200_000,
        ),
        make(
            T::Zoom,
            "Zoom by letter",
            TextUnit::Letter,
            0.5,
            Ease::EaseOut,
            900_000,
        ),
        make(
            T::Spin,
            "Spin by letter",
            TextUnit::Letter,
            0.5,
            Ease::Back,
            1_000_000,
        ),
    ]
}

pub fn text_preset(preset: TextPreset) -> TextPresetDescriptor {
    text_presets()
        .into_iter()
        .find(|d| d.preset == preset)
        .expect("every text preset is in the catalog")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_listed_once_and_in_at_least_one_tab() {
        let all = clip_presets();
        for d in &all {
            assert_eq!(all.iter().filter(|o| o.preset == d.preset).count(), 1);
            assert!(d.in_out || d.combo, "{:?} is in no tab", d.preset);
        }
        let in_out = all.iter().filter(|d| d.in_out).count();
        let combo = all.iter().filter(|d| d.combo).count();
        assert!(in_out >= 15, "{in_out} in/out presets");
        assert!(combo >= 6, "{combo} combo presets");
    }

    #[test]
    fn every_text_preset_is_listed_once() {
        let all = text_presets();
        for d in &all {
            assert_eq!(all.iter().filter(|o| o.preset == d.preset).count(), 1);
            assert_eq!(d.animator.preset, d.preset);
        }
    }
}
