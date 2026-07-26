//! Resolving a family name to actual fonts.
//!
//! The only interesting decision here is what happens when the name is wrong,
//! and the answer is: nothing visible. A project authored on a machine with
//! "Bebas Neue" installed opens on one without it, and it must still show the
//! title. `fontique` skips a family it cannot find, so a request for one
//! missing family would otherwise leave parley with an empty family list and
//! only its script-based fallback to work with. Appending a generic family
//! guarantees there is always a real font behind the name.

use parley::style::{FontFamily, FontFamilyName, GenericFamily};

/// The family list handed to parley: what the user asked for, then a generic.
///
/// The name is passed through `FontFamilyName::parse` rather than used raw so
/// a CSS-ish value like `"Inter", sans-serif` from the UI does the expected
/// thing instead of being looked up as one long family name.
pub fn family_stack(requested: &str) -> FontFamily<'static> {
    let mut names: Vec<FontFamilyName<'static>> = Vec::new();

    for part in requested.split(',') {
        let part = part.trim().trim_matches(['"', '\''].as_slice()).trim();
        if part.is_empty() {
            continue;
        }
        match part.to_ascii_lowercase().as_str() {
            "serif" => names.push(FontFamilyName::Generic(GenericFamily::Serif)),
            "sans-serif" | "sans serif" => {
                names.push(FontFamilyName::Generic(GenericFamily::SansSerif));
            }
            "monospace" | "mono" => names.push(FontFamilyName::Generic(GenericFamily::Monospace)),
            "cursive" => names.push(FontFamilyName::Generic(GenericFamily::Cursive)),
            "fantasy" => names.push(FontFamilyName::Generic(GenericFamily::Fantasy)),
            "system-ui" | "system ui" => {
                names.push(FontFamilyName::Generic(GenericFamily::SystemUi));
            }
            "emoji" => names.push(FontFamilyName::Generic(GenericFamily::Emoji)),
            _ => names.push(FontFamilyName::Named(part.to_string().into())),
        }
    }

    // Emoji, explicitly, because the script-based fallback does not find them.
    // Emoji carry script `Common`, so a fallback keyed on script has nothing to
    // go on and the shaper produces .notdef — three tofu boxes where the user
    // typed three emoji, which is exactly the class of bug that reads as "your
    // editor does not support my language". Naming the generic family costs
    // nothing for text that has no emoji in it: a font is only consulted for the
    // characters the ones before it did not cover.
    if !names
        .iter()
        .any(|n| matches!(n, FontFamilyName::Generic(GenericFamily::Emoji)))
    {
        names.push(FontFamilyName::Generic(GenericFamily::Emoji));
    }

    // The last resort. Without it an unknown family reaches the shaper with
    // nothing selected, and what it draws then depends on the script of the
    // text rather than on anything the user chose.
    if !names
        .iter()
        .any(|n| matches!(n, FontFamilyName::Generic(GenericFamily::SansSerif)))
    {
        names.push(FontFamilyName::Generic(GenericFamily::SansSerif));
    }

    FontFamily::List(names.into())
}

/// The family list for the parts of the text that must be colour emoji.
///
/// The emoji family goes *first* here and nowhere else. Putting it first
/// globally would be a regression, not a fix: `NotoColorEmoji` maps `#`, `*`
/// and the digits `0`–`9` so it can build keycap sequences, so a font stack
/// that consults it before the user's family renders "2026" as four coloured
/// keycaps. Restricting it to the ranges [`emoji_ranges`] found is what makes
/// it safe.
pub fn emoji_stack(requested: &str) -> FontFamily<'static> {
    let mut names = vec![FontFamilyName::Generic(GenericFamily::Emoji)];
    if let FontFamily::List(rest) = family_stack(requested) {
        names.extend(rest.iter().cloned());
    }
    FontFamily::List(names.into())
}

/// Byte ranges of `text` that should be drawn in a colour emoji font.
///
/// Without this, an emoji is drawn by whichever font in the stack happens to
/// have the codepoint — and DejaVu Sans has a monochrome outline for U+1F600,
/// so "😀" comes out as a flat white line drawing on a machine that has Noto
/// Color Emoji installed. Browsers solve this with the Unicode
/// `Emoji_Presentation` property; fontique has no such notion, so the ranges
/// are recognised here.
///
/// The test is deliberately narrow — the pictographic planes, the regional
/// indicators, and anything explicitly marked with a variation selector — so
/// that `✓`, `←` and `♥`, which are text by default, keep coming from the
/// user's font.
pub fn emoji_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    /// U+FE0F: "draw the previous character as an emoji".
    const EMOJI_SELECTOR: char = '\u{FE0F}';
    /// U+200D, which joins 👨 + 💻 into one glyph.
    const ZWJ: char = '\u{200D}';
    /// U+20E3, the enclosing keycap that turns `1` into 1️⃣.
    const KEYCAP: char = '\u{20E3}';

    fn is_pictographic(c: char) -> bool {
        // Playing cards (U+1F0xx) through symbols and pictographs extended-A,
        // which includes the regional indicators that make up flags, the skin
        // tone modifiers, and the emoticon block.
        matches!(c as u32, 0x1F000..=0x1FAFF | 0x1FB00..=0x1FBFF)
    }

    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();

    for (i, (offset, c)) in chars.iter().enumerate() {
        let followed_by_selector = matches!(
            chars.get(i + 1),
            Some((_, EMOJI_SELECTOR)) | Some((_, KEYCAP))
        );
        let is_joiner = matches!(*c, EMOJI_SELECTOR | ZWJ | KEYCAP);
        // A joiner only counts when it is holding emoji together, which it is
        // exactly when the character before it was already claimed.
        let joins_emoji = is_joiner && ranges.last().is_some_and(|r| r.end == *offset);

        if !(is_pictographic(*c) || followed_by_selector || joins_emoji) {
            continue;
        }

        let end = offset + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == *offset => last.end = end,
            _ => ranges.push(*offset..end),
        }
    }

    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(family: &FontFamily<'_>) -> Vec<String> {
        match family {
            FontFamily::List(list) => list
                .iter()
                .map(|n| match n {
                    FontFamilyName::Named(name) => name.to_string(),
                    FontFamilyName::Generic(g) => format!("<{g:?}>"),
                })
                .collect(),
            other => vec![format!("{other:?}")],
        }
    }

    #[test]
    fn an_unknown_family_still_ends_in_a_generic() {
        let stack = family_stack("Definitely Not Installed 9000");
        let names = names(&stack);
        assert_eq!(names.len(), 3, "{names:?}");
        assert_eq!(names[0], "Definitely Not Installed 9000");
        assert!(names[1].contains("Emoji"), "{names:?}");
        assert!(names[2].contains("SansSerif"), "{names:?}");
    }

    #[test]
    fn a_css_list_is_split_and_unquoted() {
        let names = names(&family_stack("\"Inter\", 'Noto Sans', monospace"));
        assert_eq!(names[0], "Inter");
        assert_eq!(names[1], "Noto Sans");
        assert!(names[2].contains("Monospace"), "{names:?}");
    }

    #[test]
    fn an_empty_family_is_just_the_generics() {
        assert_eq!(names(&family_stack("")).len(), 2);
        assert_eq!(names(&family_stack("  ,  ")).len(), 2);
    }

    #[test]
    fn the_emoji_stack_puts_the_emoji_family_first() {
        let names = names(&emoji_stack("Inter"));
        assert!(names[0].contains("Emoji"), "{names:?}");
        assert_eq!(names[1], "Inter");
    }

    #[test]
    fn only_emoji_are_claimed_for_the_emoji_font() {
        assert!(emoji_ranges("plain text 2026 ✓ ← ♥").is_empty());
        assert_eq!(emoji_ranges("😀"), vec![0..4]);
        assert_eq!(emoji_ranges("a😀b"), vec![1..5]);
        // A flag is two regional indicators and one glyph.
        assert_eq!(emoji_ranges("🇩🇪"), vec![0..8]);
        // A keycap: the digit is claimed only because of what follows it.
        assert_eq!(emoji_ranges("1\u{FE0F}\u{20E3}"), vec![0..7]);
        // A joined sequence stays one range so it can shape into one glyph.
        assert_eq!(emoji_ranges("👨\u{200D}💻"), vec![0..11]);
        // A zero-width joiner on its own is not an excuse to switch fonts.
        assert!(emoji_ranges("a\u{200D}b").is_empty());
    }
}
