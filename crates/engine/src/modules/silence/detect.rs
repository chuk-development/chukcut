//! Which stretches of an envelope are pauses worth cutting.
//!
//! A window is *quiet* when its level is under the threshold (and, in voice
//! mode, when RNNoise also hears no speech in it). Runs of quiet windows at
//! least `min_silence` long are pauses; each pause is cut minus `padding` on
//! the side that touches speech, because a cut flush against a word clips its
//! attack or its decay and every jump cut then sounds chopped. The side that
//! touches the clip's own edge needs no padding — there is no word there to
//! protect.

use serde::{Deserialize, Serialize};

use super::analyse::Envelope;
use crate::modules::project::document::{Micros, TimeRange};

/// How a window is judged quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Level under the threshold. Needs no model, and is what every
    /// "remove silences" tool starts from.
    #[default]
    Energy,
    /// Level under the threshold *or* no voice according to RNNoise. Catches
    /// breaths, keyboard noise and room tone that are loud but not speech.
    Voice,
}

/// The knobs on the review panel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SilenceParams {
    #[serde(default)]
    pub method: Method,
    /// Windows quieter than this are pauses, in dBFS.
    pub threshold_db: f32,
    /// Shorter pauses are kept: natural rhythm between words.
    pub min_silence: Micros,
    /// Kept on the speech side of every cut.
    pub padding: Micros,
}

impl Default for SilenceParams {
    fn default() -> Self {
        Self {
            method: Method::Energy,
            threshold_db: -40.0,
            min_silence: 500_000,
            padding: 120_000,
        }
    }
}

/// RNNoise's probability below which a window counts as "no voice".
const VOICE_THRESHOLD: f32 = 0.5;

/// Every pause in `envelope`, as source-time ranges to cut, in order.
pub fn detect(envelope: &Envelope, params: &SilenceParams) -> Vec<TimeRange> {
    let threshold = if params.threshold_db.is_finite() {
        params.threshold_db
    } else {
        SilenceParams::default().threshold_db
    };
    let min_silence = params.min_silence.max(envelope.window);
    let padding = params.padding.max(0);
    let voice = match params.method {
        Method::Voice => envelope.voice.as_deref(),
        Method::Energy => None,
    };

    let quiet = |i: usize| {
        envelope.db[i] < threshold
            || voice.is_some_and(|v| v.get(i).copied() < Some(VOICE_THRESHOLD))
    };

    let mut cuts = Vec::new();
    let windows = envelope.db.len();
    let mut i = 0;
    while i < windows {
        if !quiet(i) {
            i += 1;
            continue;
        }
        let first = i;
        while i < windows && quiet(i) {
            i += 1;
        }
        let run_start = envelope.start + first as Micros * envelope.window;
        let run_end = envelope.start + i as Micros * envelope.window;
        if run_end - run_start < min_silence {
            continue;
        }
        let at_head = first == 0;
        let at_tail = i == windows;
        let start = if at_head {
            run_start
        } else {
            run_start + padding
        };
        let end = if at_tail { run_end } else { run_end - padding };
        if end > start {
            cuts.push(TimeRange::new(start, end - start));
        }
    }
    cuts
}

/// A threshold that fits this recording: a little above its noise floor.
///
/// The floor is the 10th percentile of the window levels — in a talking-head
/// take at least a tenth of the time is pauses — and the threshold sits 8 dB
/// over it, but never closer than 15 dB under the median (speech), and inside
/// −60..−20 dBFS so a pathological file still gets a usable slider start.
pub fn suggest_threshold(envelope: &Envelope) -> f32 {
    if envelope.db.is_empty() {
        return SilenceParams::default().threshold_db;
    }
    let mut sorted = envelope.db.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
    let floor = at(0.10);
    let median = at(0.50);
    (floor + 8.0).min(median - 15.0).clamp(-60.0, -20.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::silence::analyse::{FLOOR_DB, WINDOW};

    /// An envelope from a pattern: `#` is speech (−12 dB), `.` is a pause.
    /// One character per 10 ms window.
    fn env(pattern: &str) -> Envelope {
        Envelope {
            start: 0,
            window: WINDOW,
            db: pattern
                .chars()
                .map(|c| if c == '#' { -12.0 } else { FLOOR_DB })
                .collect(),
            voice: None,
        }
    }

    fn params(min_silence: Micros, padding: Micros) -> SilenceParams {
        SilenceParams {
            method: Method::Energy,
            threshold_db: -40.0,
            min_silence,
            padding,
        }
    }

    #[test]
    fn a_long_pause_between_words_is_cut_minus_padding_on_both_sides() {
        // 10 windows speech, 60 pause, 10 speech.
        let pattern = format!("{}{}{}", "#".repeat(10), ".".repeat(60), "#".repeat(10));
        let cuts = detect(&env(&pattern), &params(500_000, 100_000));
        assert_eq!(cuts, vec![TimeRange::new(200_000, 400_000)]);
    }

    #[test]
    fn a_pause_shorter_than_the_minimum_is_kept() {
        let pattern = format!("{}{}{}", "#".repeat(10), ".".repeat(40), "#".repeat(10));
        assert!(detect(&env(&pattern), &params(500_000, 100_000)).is_empty());
    }

    #[test]
    fn edges_of_the_clip_need_no_padding() {
        let pattern = format!("{}{}{}", ".".repeat(60), "#".repeat(10), ".".repeat(60));
        let cuts = detect(&env(&pattern), &params(500_000, 100_000));
        assert_eq!(
            cuts,
            vec![
                // Head: from the very start up to the padding before speech.
                TimeRange::new(0, 500_000),
                // Tail: from the padding after speech to the very end.
                TimeRange::new(800_000, 500_000),
            ]
        );
    }

    #[test]
    fn padding_larger_than_the_pause_leaves_nothing_to_cut() {
        let pattern = format!("{}{}{}", "#".repeat(10), ".".repeat(60), "#".repeat(10));
        assert!(detect(&env(&pattern), &params(500_000, 300_000)).is_empty());
    }

    #[test]
    fn cuts_are_in_source_time_from_the_envelope_start() {
        let mut e = env(&format!(
            "{}{}{}",
            "#".repeat(5),
            ".".repeat(60),
            "#".repeat(5)
        ));
        e.start = 2_000_000;
        let cuts = detect(&e, &params(500_000, 0));
        assert_eq!(cuts, vec![TimeRange::new(2_050_000, 600_000)]);
    }

    #[test]
    fn voice_mode_also_cuts_loud_windows_without_speech() {
        let mut e = env(&"#".repeat(100));
        let mut voice = vec![0.9f32; 100];
        for p in &mut voice[20..80] {
            *p = 0.05;
        }
        e.voice = Some(voice);
        let mut p = params(500_000, 0);
        assert!(detect(&e, &p).is_empty(), "energy alone hears no pause");
        p.method = Method::Voice;
        assert_eq!(detect(&e, &p), vec![TimeRange::new(200_000, 600_000)]);
    }

    #[test]
    fn the_suggested_threshold_sits_between_floor_and_speech() {
        let mut e = env(&format!("{}{}", "#".repeat(70), ".".repeat(30)));
        for (i, db) in e.db.iter_mut().enumerate() {
            if *db == FLOOR_DB {
                *db = -62.0 + (i % 3) as f32;
            }
        }
        let t = suggest_threshold(&e);
        assert!(t > -60.0 && t < -27.0, "got {t}");
        assert!(!detect(
            &e,
            &SilenceParams {
                threshold_db: t,
                ..params(200_000, 0)
            }
        )
        .is_empty());
    }
}
