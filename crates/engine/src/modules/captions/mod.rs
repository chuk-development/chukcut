//! Captions: subtitles as editable titles on their own lane.
//!
//! A caption is an ordinary text clip whose [`TextMaterial`] carries a
//! [`CaptionData`] — the words behind it, with their times, and an optional
//! karaoke highlight colour. Everything a title can do, a caption can do: the
//! same rasteriser, the same transform, the same export. What this module adds
//! is the part that is specific to subtitles:
//!
//! ```text
//!   speech::Transcript ──► group ──► Vec<Cue> ──► edit::place ──► caption lane
//!   .srt / .vtt ──► srt::parse ─────┘                 │
//!                                                     ▼
//!   .srt / .vtt ◄── srt::format ◄── edit::cues ◄── the document
//! ```
//!
//! - [`srt`] reads and writes SubRip and WebVTT.
//! - [`group`] turns timed words into cues, either a few words at a time
//!   (CapCut's word captions) or as sentences that respect a line length, a
//!   line count and a duration.
//! - [`style`] is the look shared by every caption on the lane, and the presets.
//! - [`emoji`] is the picker's list and the keyword table behind "auto emoji".
//! - [`karaoke`] decides which word is lit at an instant; the renderer asks it.
//! - [`edit`] is pure: given a document, it returns the [`EditCommand`]s that
//!   place, split, merge, retext and restyle captions. Every one of them goes
//!   through the history, so a caption edit is one Ctrl+Z like any other.
//! - [`commands`] is the shell-facing surface over all of it.
//!
//! ## Why captions are text clips and not a lane of their own kind
//!
//! A new `TrackKind` would have to be taught to every `match` over lanes in the
//! timeline, the mixer, the compositor and the export. A text clip is already
//! drawn, moved, trimmed, exported and undone correctly by all of them. So a
//! caption lane is a `TrackKind::Text` lane named [`edit::LANE_NAME`], and a
//! caption is a clip whose material has `caption: Some(..)`. Burning captions
//! into an export is therefore not a feature at all — it is what the export
//! already does with text.
//!
//! [`TextMaterial`]: crate::modules::project::TextMaterial
//! [`CaptionData`]: crate::modules::project::CaptionData
//! [`EditCommand`]: crate::modules::timeline::ops::EditCommand

pub mod commands;
pub mod edit;
pub mod emoji;
pub mod group;
pub mod karaoke;
pub mod srt;
pub mod style;
pub mod words;

use serde::{Deserialize, Serialize};

use crate::modules::project::Micros;

pub use group::CaptionMode;
pub use style::{CaptionStyle, Placement};

/// One word with the instant it is spoken.
///
/// In timeline microseconds inside this module; `silence::filler` uses the
/// same type with times in a clip's source (see [`words`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedWord {
    pub text: String,
    pub start: Micros,
    pub end: Micros,
}

/// One subtitle: what is on screen, from when to when.
///
/// `text` may hold line breaks. `words` is empty for a cue read from a file
/// that has no word timing (every `.srt`), and karaoke has nothing to light
/// there until the cue is retranscribed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub start: Micros,
    pub end: Micros,
    pub text: String,
    #[serde(default)]
    pub words: Vec<TimedWord>,
}

impl Cue {
    pub fn new(start: Micros, end: Micros, text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            text: text.into(),
            words: Vec::new(),
        }
    }
}

/// What a transcription produced, before it is grouped into captions.
///
/// Providers differ in what they return: OpenAI's Whisper API and whisper.cpp
/// give words, some OpenAI-compatible servers give only segments. Both are
/// kept, and [`Transcript::timed_words`] makes words out of segments when it
/// has to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// ISO 639-1 code, when the provider said which language it heard.
    pub language: Option<String>,
    pub words: Vec<TimedWord>,
    pub segments: Vec<Cue>,
}

impl Transcript {
    /// The words, or words estimated from the segments when the provider gave
    /// none.
    ///
    /// The estimate spreads each segment's duration over its words in
    /// proportion to their length, which is how long a word takes to say to
    /// within a syllable. Good enough to break a sentence into word captions;
    /// not good enough for karaoke, and the UI says so.
    pub fn timed_words(&self) -> Vec<TimedWord> {
        if !self.words.is_empty() {
            return self.words.clone();
        }
        self.segments
            .iter()
            .flat_map(|segment| estimate_words(&segment.text, segment.start, segment.end))
            .collect()
    }

    /// Move every time by `offset`, for stitching chunks back together.
    pub fn shifted(mut self, offset: Micros) -> Self {
        for word in &mut self.words {
            word.start += offset;
            word.end += offset;
        }
        for segment in &mut self.segments {
            segment.start += offset;
            segment.end += offset;
            for word in &mut segment.words {
                word.start += offset;
                word.end += offset;
            }
        }
        self
    }

    /// Append `other`, which must come after `self` in time.
    pub fn extend(&mut self, other: Transcript) {
        if self.language.is_none() {
            self.language = other.language;
        }
        self.words.extend(other.words);
        self.segments.extend(other.segments);
    }
}

/// Spread `start..end` over the words of `text` by their length.
pub fn estimate_words(text: &str, start: Micros, end: Micros) -> Vec<TimedWord> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let duration = (end - start).max(0);
    // One extra unit per word stands for the gap between words, so a one
    // letter word still gets a visible slice.
    let weights: Vec<i64> = tokens
        .iter()
        .map(|t| t.chars().count() as i64 + 1)
        .collect();
    let total: i64 = weights.iter().sum::<i64>().max(1);

    let mut words = Vec::with_capacity(tokens.len());
    let mut acc = 0i64;
    for (token, weight) in tokens.iter().zip(&weights) {
        let word_start = start + duration * acc / total;
        acc += weight;
        let word_end = start + duration * acc / total;
        words.push(TimedWord {
            text: (*token).to_string(),
            start: word_start,
            end: word_end,
        });
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimated_words_cover_the_segment_without_gaps() {
        let words = estimate_words("a longer sentence", 1_000_000, 2_000_000);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].start, 1_000_000);
        assert_eq!(words.last().unwrap().end, 2_000_000);
        for pair in words.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        // "sentence" is longer than "a", so it is given longer.
        assert!(words[2].end - words[2].start > words[0].end - words[0].start);
    }

    #[test]
    fn a_transcript_without_words_estimates_them_from_segments() {
        let transcript = Transcript {
            language: Some("en".into()),
            words: Vec::new(),
            segments: vec![
                Cue::new(0, 1_000_000, "hello there"),
                Cue::new(2_000_000, 3_000_000, "again"),
            ],
        };
        let words = transcript.timed_words();
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["hello", "there", "again"]
        );
        assert_eq!(words[2].start, 2_000_000);
    }

    #[test]
    fn shifting_moves_words_and_segments_alike() {
        let transcript = Transcript {
            language: None,
            words: vec![TimedWord {
                text: "x".into(),
                start: 10,
                end: 20,
            }],
            segments: vec![Cue::new(10, 20, "x")],
        }
        .shifted(1_000);
        assert_eq!(transcript.words[0].start, 1_010);
        assert_eq!(transcript.segments[0].end, 1_020);
    }
}
