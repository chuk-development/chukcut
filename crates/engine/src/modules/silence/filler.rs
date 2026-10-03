//! Filler words — "um", "uh", "äh" — found in a transcript with word times.
//!
//! This module does not transcribe. Captions do (`docs/research/ml-features.md`
//! §3.1), and they produce exactly what is needed here: words with start and
//! end times. [`WordTimings`] is the seam: the captions module registers an
//! implementation with [`register_word_timings`], and until it does, the
//! filler mode says that a transcript is needed rather than guessing.
//!
//! A filler cut is the word's own span plus a little padding, and it goes
//! through the same review list and the same [`super::cut::remove_ranges`] as
//! a pause. Whisper-style models often drop fillers from their output
//! entirely (ml-features §3.8), so this finds the ones the transcript kept; it
//! never invents any.

use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::modules::project::document::{Micros, Project, TimeRange};

/// One transcribed word, in the **source** time of the clip's material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedWord {
    pub text: String,
    pub start: Micros,
    pub end: Micros,
}

/// Where word times come from. Implemented by whatever owns transcripts.
pub trait WordTimings: Send + Sync {
    /// The words spoken in the clip `segment_id`, with times in that clip's
    /// source time, or `None` when the clip has no transcript.
    fn words(&self, project: &Project, segment_id: &str) -> Option<Vec<TimedWord>>;
}

static WORD_TIMINGS: RwLock<Option<Arc<dyn WordTimings>>> = RwLock::new(None);

/// Install the transcript provider. The last registration wins.
pub fn register_word_timings(source: Arc<dyn WordTimings>) {
    *WORD_TIMINGS.write() = Some(source);
}

/// The installed transcript provider, if any.
pub fn word_timings() -> Option<Arc<dyn WordTimings>> {
    WORD_TIMINGS.read().clone()
}

/// Languages with a filler list, as (code, name) for a picker.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("auto", "Any language"),
    ("en", "English"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("nl", "Dutch"),
    ("it", "Italian"),
];

/// Filler words per language, already in the normalised spelling
/// [`normalise`] produces (lower case, repeated letters collapsed).
///
/// Only sounds that are never a real word in that language are listed: "like"
/// and "you know" are fillers sometimes and meaning the rest of the time, and
/// cutting them blind removes words someone wanted.
pub fn filler_words(language: &str) -> &'static [&'static str] {
    match language {
        "en" => &["um", "uh", "uhm", "er", "erm", "ah", "eh", "hm", "m"],
        "de" => &["äh", "ähm", "ehm", "em", "öh", "öhm", "hm", "m", "äm"],
        "fr" => &["euh", "heu", "hum", "hm", "m", "bah"],
        "es" => &["eh", "em", "ehm", "hm", "m"],
        "nl" => &["eh", "ehm", "uh", "uhm", "hm", "m"],
        "it" => &["eh", "ehm", "em", "hm", "m", "mh"],
        _ => &[
            "um", "uh", "uhm", "er", "erm", "ah", "eh", "ehm", "em", "hm", "m", "äh", "ähm", "äm",
            "öh", "öhm", "euh", "heu", "hum", "mh",
        ],
    }
}

/// A word as the filler lists spell it: lower case, punctuation stripped,
/// runs of one letter collapsed — "Ummm," and "um" are the same filler, and
/// "ähhm" and "ähm" too.
pub fn normalise(word: &str) -> String {
    let lower = word.trim().to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last = None;
    for c in lower.chars().filter(|c| c.is_alphabetic()) {
        if Some(c) != last {
            out.push(c);
        }
        last = Some(c);
    }
    out
}

/// Whether `word` is a filler in `language` (`"auto"` checks every list).
pub fn is_filler(word: &str, language: &str) -> bool {
    let word = normalise(word);
    !word.is_empty() && filler_words(language).contains(&word.as_str())
}

/// Cuts for every filler in `words`, widened by `padding` on each side and
/// merged where they touch.
///
/// The padding is small on purpose (tens of milliseconds): word times from a
/// speech model are only as exact as its frame rate, and a filler's onset is
/// soft, so the bare span leaves a fragment of "m" behind.
pub fn filler_cuts(words: &[TimedWord], language: &str, padding: Micros) -> Vec<TimeRange> {
    let padding = padding.max(0);
    let mut spans: Vec<(Micros, Micros)> = words
        .iter()
        .filter(|w| w.end > w.start && is_filler(&w.text, language))
        .map(|w| ((w.start - padding).max(0), w.end + padding))
        .collect();
    spans.sort();
    let mut merged: Vec<(Micros, Micros)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
        .into_iter()
        .map(|(a, b)| TimeRange::new(a, b - a))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: Micros, end: Micros) -> TimedWord {
        TimedWord {
            text: text.into(),
            start,
            end,
        }
    }

    #[test]
    fn spellings_of_one_filler_normalise_to_one_word() {
        assert_eq!(normalise("Ummm,"), "um");
        assert_eq!(normalise(" ÄHHM... "), "ähm");
        assert!(is_filler("Uhh", "en"));
        assert!(is_filler("ähm", "de"));
        assert!(is_filler("ähm", "auto"));
        assert!(!is_filler("ähm", "en"));
        assert!(!is_filler("umbrella", "en"));
        assert!(!is_filler("...", "auto"));
    }

    #[test]
    fn fillers_become_padded_merged_cuts() {
        let words = [
            word("So", 0, 200_000),
            word("um,", 300_000, 500_000),
            word("uh", 520_000, 600_000),
            word("today", 700_000, 1_000_000),
            word("Äh", 2_000_000, 2_200_000),
        ];
        let cuts = filler_cuts(&words, "auto", 20_000);
        assert_eq!(
            cuts,
            vec![
                TimeRange::new(280_000, 340_000),
                TimeRange::new(1_980_000, 240_000),
            ]
        );
        // English only: the German "äh" stays.
        assert_eq!(filler_cuts(&words, "en", 20_000).len(), 1);
    }
}
