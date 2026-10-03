//! Timed words into captions.
//!
//! Two modes, the two every short-form editor offers:
//!
//! - **Words**: one to a few words at a time, the punchy CapCut look. A cue
//!   stays up until the next one starts, unless the speaker pauses — then it
//!   lingers briefly and the screen goes empty rather than holding a word over
//!   silence.
//! - **Sentences**: classic subtitles. Words fill lines up to a character
//!   limit, lines fill a cue up to a line limit, and a cue never runs longer
//!   than a maximum duration. A sentence end or a long pause always starts a
//!   new cue, because a subtitle that spans two sentences reads wrong.

use serde::{Deserialize, Serialize};

use super::{Cue, TimedWord};
use crate::modules::project::{Micros, MICROS_PER_SECOND};

/// How words are grouped into captions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CaptionMode {
    Words {
        /// At most this many words on screen at once. 1 is the classic
        /// one-word-at-a-time look.
        max_words: usize,
    },
    Sentences {
        max_chars_per_line: usize,
        max_lines: usize,
        max_duration: Micros,
    },
}

impl Default for CaptionMode {
    fn default() -> Self {
        Self::Sentences {
            // Netflix's style guide: 42 characters, two lines, seven seconds.
            // The duration is shorter here because short-form video moves
            // faster than television.
            max_chars_per_line: 42,
            max_lines: 2,
            max_duration: 5 * MICROS_PER_SECOND,
        }
    }
}

impl CaptionMode {
    pub fn words(max_words: usize) -> Self {
        Self::Words {
            max_words: max_words.max(1),
        }
    }
}

/// A pause this long ends a cue in either mode.
const PAUSE: Micros = 700_000;
/// How long a word caption stays up after its last word when nothing follows.
const LINGER: Micros = 400_000;
/// The shortest cue worth showing. A shorter one is a flicker.
const MIN_CUE: Micros = 120_000;

/// Group `words` into cues.
pub fn group(words: &[TimedWord], mode: CaptionMode) -> Vec<Cue> {
    let words: Vec<TimedWord> = words
        .iter()
        .filter_map(|w| {
            let text = w.text.trim();
            (!text.is_empty()).then(|| TimedWord {
                text: text.to_string(),
                start: w.start,
                end: w.end.max(w.start),
            })
        })
        .collect();

    let groups = match mode {
        CaptionMode::Words { max_words } => by_count(&words, max_words.max(1)),
        CaptionMode::Sentences {
            max_chars_per_line,
            max_lines,
            max_duration,
        } => by_sentence(
            &words,
            max_chars_per_line.max(4),
            max_lines.max(1),
            max_duration.max(MIN_CUE),
        ),
    };

    let mut cues: Vec<Cue> = groups
        .into_iter()
        .filter(|g| !g.is_empty())
        .map(|g| {
            let text = match mode {
                CaptionMode::Words { .. } => join_words(&g),
                CaptionMode::Sentences {
                    max_chars_per_line, ..
                } => wrap(&g, max_chars_per_line.max(4)).join("\n"),
            };
            Cue {
                start: g[0].start,
                end: g[g.len() - 1].end,
                text,
                words: g,
            }
        })
        .collect();

    // Timing: hold each cue until the next one when they are close, linger a
    // little when they are not, and never overlap.
    let hold_gap = match mode {
        CaptionMode::Words { .. } => PAUSE,
        CaptionMode::Sentences { .. } => PAUSE / 2,
    };
    for i in 0..cues.len() {
        let next_start = cues.get(i + 1).map(|c| c.start);
        let cue = &mut cues[i];
        let mut end = cue.end;
        match next_start {
            Some(next) if next - end <= hold_gap => end = next,
            Some(next) => end = (end + LINGER).min(next),
            None => end += LINGER,
        }
        if end - cue.start < MIN_CUE {
            end = match next_start {
                Some(next) => (cue.start + MIN_CUE).min(next),
                None => cue.start + MIN_CUE,
            };
        }
        cue.end = end.max(cue.start + 1);
    }
    cues
}

fn by_count(words: &[TimedWord], max_words: usize) -> Vec<Vec<TimedWord>> {
    let mut groups: Vec<Vec<TimedWord>> = Vec::new();
    let mut current: Vec<TimedWord> = Vec::new();
    for word in words {
        let paused = current.last().is_some_and(|l| word.start - l.end > PAUSE);
        if paused || current.len() >= max_words {
            groups.push(std::mem::take(&mut current));
        }
        let ends_sentence = ends_sentence(&word.text);
        current.push(word.clone());
        if ends_sentence {
            groups.push(std::mem::take(&mut current));
        }
    }
    groups.push(current);
    groups
}

fn by_sentence(
    words: &[TimedWord],
    max_chars: usize,
    max_lines: usize,
    max_duration: Micros,
) -> Vec<Vec<TimedWord>> {
    let mut groups: Vec<Vec<TimedWord>> = Vec::new();
    let mut current: Vec<TimedWord> = Vec::new();
    for word in words {
        if let Some(first) = current.first() {
            let paused = word.start - current[current.len() - 1].end > PAUSE;
            let too_long = word.end - first.start > max_duration;
            let mut candidate = current.clone();
            candidate.push(word.clone());
            let too_many_lines = wrap(&candidate, max_chars).len() > max_lines;
            if paused || too_long || too_many_lines {
                groups.push(std::mem::take(&mut current));
            }
        }
        current.push(word.clone());
        if ends_sentence(&word.text) {
            groups.push(std::mem::take(&mut current));
        } else if ends_clause(&word.text) {
            // A comma is a good place to break once the cue is mostly full: it
            // keeps a clause on one screen instead of splitting it mid-phrase.
            let used: usize = join_words(&current).chars().count();
            if used * 10 >= max_chars * max_lines * 6 {
                groups.push(std::mem::take(&mut current));
            }
        }
    }
    groups.push(current);
    groups
}

/// Words joined by spaces, with punctuation-only tokens glued to the word
/// before them — some transcribers return "," as a word of its own.
pub fn join_words(words: &[TimedWord]) -> String {
    let mut out = String::new();
    for word in words {
        let text = word.text.trim();
        if !out.is_empty() && !is_punctuation(text) {
            out.push(' ');
        }
        out.push_str(text);
    }
    out
}

/// Greedy line breaking by character count.
fn wrap(words: &[TimedWord], max_chars: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in words {
        let text = word.text.trim();
        let glue = !line.is_empty() && !is_punctuation(text);
        let wanted = line.chars().count() + usize::from(glue) + text.chars().count();
        if !line.is_empty() && wanted > max_chars && !is_punctuation(text) {
            lines.push(std::mem::take(&mut line));
        } else if glue {
            line.push(' ');
        }
        line.push_str(text);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn is_punctuation(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_punctuation() || "…。、，！？".contains(c))
}

fn ends_sentence(text: &str) -> bool {
    let text = text.trim_end_matches(['"', '\'', ')', '»', '”']);
    text.ends_with(['.', '!', '?', '…', '。', '！', '？'])
}

fn ends_clause(text: &str) -> bool {
    text.ends_with([',', ';', ':', '，', '、'])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words a quarter second apart, each 200 ms long.
    fn words(text: &str) -> Vec<TimedWord> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, w)| TimedWord {
                text: w.to_string(),
                start: i as i64 * 250_000,
                end: i as i64 * 250_000 + 200_000,
            })
            .collect()
    }

    fn texts(cues: &[Cue]) -> Vec<&str> {
        cues.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn one_word_at_a_time_holds_until_the_next_word() {
        let cues = group(&words("one two three"), CaptionMode::words(1));
        assert_eq!(texts(&cues), ["one", "two", "three"]);
        assert_eq!(cues[0].end, cues[1].start);
        assert_eq!(cues[1].end, cues[2].start);
        // The last one lingers past its word.
        assert_eq!(cues[2].end, 500_000 + 200_000 + LINGER);
    }

    #[test]
    fn word_groups_break_at_sentence_ends_and_pauses() {
        let mut w = words("this is fine. and then more words here");
        // A long pause before "here".
        let last = w.len() - 1;
        w[last].start += 2_000_000;
        w[last].end += 2_000_000;
        let cues = group(&w, CaptionMode::words(3));
        assert_eq!(
            texts(&cues),
            ["this is fine.", "and then more", "words", "here"]
        );
        // "words" lingers and does not run into the pause.
        assert!(cues[2].end < cues[3].start);
    }

    #[test]
    fn sentences_wrap_to_the_line_limit() {
        let cues = group(
            &words("the quick brown fox jumps over the lazy dog and keeps running far away"),
            CaptionMode::Sentences {
                max_chars_per_line: 16,
                max_lines: 2,
                max_duration: 60 * MICROS_PER_SECOND,
            },
        );
        for cue in &cues {
            let lines: Vec<&str> = cue.text.lines().collect();
            assert!(lines.len() <= 2, "{:?}", cue.text);
            for line in lines {
                assert!(line.chars().count() <= 16, "{line:?}");
            }
        }
        assert_eq!(cues[0].text, "the quick brown\nfox jumps over");
        // Nothing lost, nothing duplicated.
        let joined: Vec<String> = cues.iter().map(|c| c.text.replace('\n', " ")).collect();
        assert_eq!(
            joined.join(" "),
            "the quick brown fox jumps over the lazy dog and keeps running far away"
        );
    }

    #[test]
    fn sentences_respect_the_duration_and_the_full_stop() {
        let cues = group(
            &words("one two three four five six. seven"),
            CaptionMode::Sentences {
                max_chars_per_line: 100,
                max_lines: 2,
                max_duration: 800_000,
            },
        );
        assert_eq!(texts(&cues), ["one two three", "four five six.", "seven"]);
        for pair in cues.windows(2) {
            assert!(pair[0].end <= pair[1].start);
        }
    }

    #[test]
    fn punctuation_tokens_glue_to_the_word_before() {
        let cues = group(&words("hello , world !"), CaptionMode::default());
        assert_eq!(texts(&cues), ["hello, world!"]);
    }

    #[test]
    fn empty_input_is_no_captions() {
        assert!(group(&[], CaptionMode::default()).is_empty());
        assert!(group(&words("   "), CaptionMode::words(2)).is_empty());
    }
}
