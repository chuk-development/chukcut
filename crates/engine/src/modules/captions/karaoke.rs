//! Which word of a caption is lit, at an instant.
//!
//! The rasteriser knows nothing about time; it paints a byte range in another
//! colour when asked ([`TextHighlight`]). This is the asking: find the word
//! being spoken at the segment's source time and where it sits in the text.
//!
//! The text is the user's — they may have fixed a word, added an emoji or
//! broken a line — so word timing and text are matched by searching for each
//! word in order rather than by assuming the text is the words joined by
//! spaces. A word that cannot be found any more is simply never lit.

use std::ops::Range;

use crate::modules::project::{Micros, TextMaterial};
use crate::modules::text::{TextHighlight, TextRequest};

/// The lit word at `source_time`: its index among the caption's words and its
/// byte range in the content. `None` when karaoke is off, before the first
/// word, or when the word is no longer in the text.
pub fn lit_word(material: &TextMaterial, source_time: Micros) -> Option<(usize, Range<usize>)> {
    let caption = material.caption.as_ref()?;
    caption.highlight?;
    // The word stays lit until the next one starts, as in every karaoke
    // caption: a gap between two words is not a moment of nothing.
    let index = caption.words.iter().rposition(|w| w.start <= source_time)?;
    let ranges = word_ranges(material);
    ranges.get(index).cloned().flatten().map(|r| (index, r))
}

/// Where each word of the caption sits in its content, by searching in order.
pub fn word_ranges(material: &TextMaterial) -> Vec<Option<Range<usize>>> {
    let Some(caption) = material.caption.as_ref() else {
        return Vec::new();
    };
    let content = material.content.as_str();
    let lower = content.to_lowercase();
    // Lower-casing can change byte lengths (rarely: 'İ' grows, 'ẞ' shrinks),
    // and then a match in `lower` is not at the same offset in `content`.
    // Fall back to exact matching in that case. The check is per character:
    // comparing only the total lengths let one growing and one shrinking
    // character cancel out, and the range then split a character in
    // `content`, which panics wherever the range is used to slice it.
    let same_offsets = content
        .chars()
        .all(|c| c.to_lowercase().map(char::len_utf8).sum::<usize>() == c.len_utf8());

    let mut cursor = 0usize;
    caption
        .words
        .iter()
        .map(|word| {
            let needle = word.text.trim();
            if needle.is_empty() || cursor > content.len() {
                return None;
            }
            let (found, len) = if same_offsets {
                let needle = needle.to_lowercase();
                (lower[cursor..].find(&needle), needle.len())
            } else {
                (content[cursor..].find(needle), needle.len())
            };
            found.map(|at| {
                let start = cursor + at;
                let end = start + len;
                cursor = end;
                start..end
            })
        })
        .collect()
}

/// The text request for `material` at `source_time`, and the lit word's index
/// for a cache key. Without karaoke this is exactly `TextRequest::from`.
pub fn request_at(material: &TextMaterial, source_time: Micros) -> (TextRequest, Option<usize>) {
    let mut request = TextRequest::from(material);
    let lit = lit_word(material, source_time);
    let color = material.caption.as_ref().and_then(|c| c.highlight);
    match (lit, color) {
        (Some((index, range)), Some(color)) => {
            request.highlight = Some(TextHighlight { range, color });
            (request, Some(index))
        }
        _ => (request, None),
    }
}

/// Whether `material` changes with time at all — the renderer's cache may
/// treat any other title as a still.
pub fn is_animated(material: &TextMaterial) -> bool {
    material
        .caption
        .as_ref()
        .is_some_and(|c| c.highlight.is_some() && !c.words.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::{CaptionData, CaptionWord};

    fn caption(content: &str, words: &[(&str, Micros)], highlight: bool) -> TextMaterial {
        let mut material = crate::modules::text::edit::default_material(
            &crate::modules::project::Project::new("t", Default::default(), 30.0),
            Some(content.to_string()),
        );
        material.caption = Some(CaptionData {
            words: words
                .iter()
                .map(|(t, s)| CaptionWord {
                    text: t.to_string(),
                    start: *s,
                    end: *s + 100_000,
                })
                .collect(),
            highlight: highlight.then_some([1.0, 1.0, 0.0, 1.0]),
        });
        material
    }

    #[test]
    fn the_spoken_word_is_lit_until_the_next_starts() {
        let m = caption(
            "Hello big world",
            &[("Hello", 0), ("big", 500_000), ("world", 900_000)],
            true,
        );
        assert_eq!(lit_word(&m, 0), Some((0, 0..5)));
        assert_eq!(lit_word(&m, 450_000), Some((0, 0..5)));
        assert_eq!(lit_word(&m, 500_000), Some((1, 6..9)));
        assert_eq!(lit_word(&m, 5_000_000), Some((2, 10..15)));
    }

    #[test]
    fn nothing_is_lit_before_the_first_word_or_with_karaoke_off() {
        let on = caption("a b", &[("a", 100_000), ("b", 200_000)], true);
        assert_eq!(lit_word(&on, 0), None);
        let off = caption("a b", &[("a", 0), ("b", 200_000)], false);
        assert_eq!(lit_word(&off, 300_000), None);
        assert!(!is_animated(&off));
        assert!(is_animated(&on));
    }

    #[test]
    fn edited_text_is_searched_not_assumed() {
        // The user capitalised a word, broke the line and added an emoji; a
        // word they deleted is never lit.
        let m = caption(
            "🔥 HELLO\nworld",
            &[("hello", 0), ("brave", 200_000), ("world", 400_000)],
            true,
        );
        let ranges = word_ranges(&m);
        let start = "🔥 ".len();
        assert_eq!(ranges[0], Some(start..start + 5));
        assert_eq!(ranges[1], None);
        assert_eq!(&m.content[ranges[2].clone().unwrap()], "world");
        assert_eq!(lit_word(&m, 250_000), None);
    }

    /// The lit word is drawn in the highlight colour and the rest is not: the
    /// rasteriser half of karaoke, end to end through the real text stack.
    #[test]
    fn the_lit_word_is_painted_in_the_highlight_colour() {
        let mut m = caption("ab cd", &[("ab", 0), ("cd", 300_000)], true);
        m.stroke_width = 0.0;
        m.shadow = None;
        m.font_size = 64.0;
        m.caption.as_mut().unwrap().highlight = Some([0.0, 1.0, 0.0, 1.0]);
        let renderer = crate::modules::text::TextRenderer::new();
        let options = crate::modules::text::RasterOptions::tight();

        let green_columns = |source_time| {
            let (request, _) = request_at(&m, source_time);
            let image = renderer.rasterize_uncached(&request, &options);
            let mut columns = Vec::new();
            for x in 0..image.width {
                let green = (0..image.height).any(|y| {
                    let [r, g, b, a] = image.pixel(x, y);
                    a > 200 && g > 200 && r < 60 && b < 60
                });
                if green {
                    columns.push(x);
                }
            }
            (columns, image.width)
        };

        let (first, width) = green_columns(0);
        assert!(!first.is_empty(), "nothing was lit");
        assert!(
            first.iter().all(|x| *x < width / 2),
            "the lit word is the first one"
        );
        let (second, _) = green_columns(400_000);
        assert!(!second.is_empty());
        assert!(
            second.iter().all(|x| *x > width / 2),
            "now the second word is lit"
        );
        let (none, _) = green_columns(-1);
        assert!(none.is_empty(), "nothing is lit before the first word");
    }

    #[test]
    fn the_request_carries_the_highlight_only_when_lit() {
        let m = caption("one two", &[("one", 0), ("two", 300_000)], true);
        let (request, key) = request_at(&m, 300_000);
        assert_eq!(key, Some(1));
        assert_eq!(request.highlight.unwrap().range, 4..7);
        let (request, key) = request_at(&m, -1);
        assert_eq!(key, None);
        assert!(request.highlight.is_none());
    }

    #[test]
    fn a_growing_and_a_shrinking_capital_do_not_shift_the_ranges() {
        // 'İ' is one byte longer lower-cased and 'ẞ' one byte shorter, so the
        // whole string keeps its length while every offset after 'İ' moves.
        let content = "İx ẞword";
        let m = caption(content, &[("İx", 0), ("ẞword", 300_000)], true);
        let ranges = word_ranges(&m);
        assert_eq!(ranges, vec![Some(0..3), Some(4..11)]);
        for range in ranges.into_iter().flatten() {
            assert!(content.is_char_boundary(range.start));
            assert!(content.is_char_boundary(range.end));
        }
    }
}
