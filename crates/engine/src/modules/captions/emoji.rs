//! Emoji for captions: the picker's list and "auto emoji".
//!
//! Colour emoji are drawn by the text rasteriser itself — it paints `CBDT`
//! and `sbix` bitmap strikes, which is what Noto Color Emoji ships — so this
//! module only has to choose characters. See `text::font::emoji_ranges` for
//! how an emoji is routed to the colour font.
//!
//! Every entry that is a single character from the older symbol blocks
//! (☕, ⭐, ⚡) carries U+FE0F. Those characters are *text* by default, and
//! without the selector the rasteriser draws them from the caption's own font
//! as a flat outline; the selector is what asks for the colour glyph.
//!
//! "Auto emoji" is a keyword table, not a model. It picks one fitting emoji per
//! caption from the words in it, which is what the feature is in every editor
//! that has it, and a table can be read, fixed and extended by anyone.

/// The picker, by category. Order is display order.
pub const PICKER: &[(&str, &[&str])] = &[
    (
        "Smileys",
        &[
            "😀", "😃", "😄", "😁", "😆", "😅", "🤣", "😂", "🙂", "😉", "😊", "😇", "🥰", "😍",
            "🤩", "😘", "😋", "😛", "😜", "🤪", "🤗", "🤭", "🤫", "🤔", "😐", "😏", "😒", "🙄",
            "😬", "😌", "😴", "😷", "🤯", "🥳", "😎", "🤓", "😕", "😮", "😲", "😳", "🥺", "😢",
            "😭", "😱", "😤", "😡", "🤬", "💀", "🤡", "👻", "👽", "🤖", "💩",
        ],
    ),
    (
        "Gestures",
        &[
            "👍",
            "👎",
            "👌",
            "✌\u{fe0f}",
            "🤞",
            "🤟",
            "🤘",
            "🤙",
            "👈",
            "👉",
            "👆",
            "👇",
            "☝\u{fe0f}",
            "✋\u{fe0f}",
            "👋",
            "👏",
            "🙌",
            "🙏",
            "💪",
            "🫶",
            "👀",
            "🧠",
            "🫡",
            "🤝",
        ],
    ),
    (
        "Hearts",
        &[
            "❤\u{fe0f}",
            "🧡",
            "💛",
            "💚",
            "💙",
            "💜",
            "🖤",
            "🤍",
            "🤎",
            "💔",
            "💕",
            "💖",
            "💘",
            "💯",
            "💥",
            "💫",
            "💦",
            "💨",
        ],
    ),
    (
        "Nature",
        &[
            "🔥",
            "✨\u{fe0f}",
            "⭐\u{fe0f}",
            "🌟",
            "☀\u{fe0f}",
            "🌙",
            "🌈",
            "⚡\u{fe0f}",
            "❄\u{fe0f}",
            "🌊",
            "🌍",
            "🌸",
            "🌹",
            "🌴",
            "🍀",
            "🐶",
            "🐱",
            "🦁",
            "🐻",
            "🐼",
            "🦄",
            "🐝",
            "🦋",
        ],
    ),
    (
        "Food",
        &[
            "🍎",
            "🍌",
            "🍓",
            "🍑",
            "🥑",
            "🍕",
            "🍔",
            "🍟",
            "🌮",
            "🍣",
            "🍜",
            "🍩",
            "🍪",
            "🎂",
            "🍫",
            "🍿",
            "☕\u{fe0f}",
            "🍺",
            "🍷",
            "🥤",
            "🥗",
            "🥩",
        ],
    ),
    (
        "Activity",
        &[
            "⚽\u{fe0f}",
            "🏀",
            "🏈",
            "🎾",
            "🏋\u{fe0f}",
            "🏃",
            "🚴",
            "🧘",
            "🎯",
            "🎮",
            "🎲",
            "🎵",
            "🎶",
            "🎤",
            "🎧",
            "🎸",
            "🎬",
            "📸",
            "🎉",
            "🎊",
            "🎁",
            "🏆",
            "🥇",
        ],
    ),
    (
        "Objects",
        &[
            "📱",
            "💻",
            "⌚\u{fe0f}",
            "💡",
            "💰",
            "💵",
            "💎",
            "📈",
            "📉",
            "📌",
            "📚",
            "✏\u{fe0f}",
            "🔑",
            "🔒",
            "🛒",
            "🚗",
            "✈\u{fe0f}",
            "🚀",
            "🏠",
            "⏰\u{fe0f}",
            "📅",
            "🧪",
            "💊",
        ],
    ),
    (
        "Symbols",
        &[
            "✅\u{fe0f}",
            "❌\u{fe0f}",
            "⚠\u{fe0f}",
            "❓\u{fe0f}",
            "❗\u{fe0f}",
            "‼\u{fe0f}",
            "💬",
            "💭",
            "🔔",
            "🆕",
            "🆗",
            "🆒",
            "🔝",
            "➡\u{fe0f}",
            "⬅\u{fe0f}",
            "⬆\u{fe0f}",
            "⬇\u{fe0f}",
            "🔄",
            "♻\u{fe0f}",
            "🚫",
        ],
    ),
];

/// Keyword → emoji. Lower case; matched against whole words and simple
/// inflections of them (see [`matches_keyword`]). English first, then German,
/// because those are the languages this editor's first users speak.
const KEYWORDS: &[(&str, &str)] = &[
    // feelings
    ("love", "❤\u{fe0f}"),
    ("heart", "❤\u{fe0f}"),
    ("happy", "😊"),
    ("smile", "😊"),
    ("laugh", "😂"),
    ("funny", "😂"),
    ("lol", "😂"),
    ("sad", "😢"),
    ("cry", "😭"),
    ("angry", "😡"),
    ("mad", "😡"),
    ("scared", "😱"),
    ("scary", "😱"),
    ("shock", "😱"),
    ("crazy", "🤯"),
    ("mind", "🤯"),
    ("wow", "🤩"),
    ("amazing", "🤩"),
    ("awesome", "🤩"),
    ("cool", "😎"),
    ("think", "🤔"),
    ("wonder", "🤔"),
    ("secret", "🤫"),
    ("kiss", "😘"),
    ("tired", "😴"),
    ("sleep", "😴"),
    ("dead", "💀"),
    ("sick", "🤒"),
    // people and gestures
    ("hello", "👋"),
    ("hi", "👋"),
    ("bye", "👋"),
    ("thanks", "🙏"),
    ("thank", "🙏"),
    ("please", "🙏"),
    ("pray", "🙏"),
    ("good", "👍"),
    ("great", "👍"),
    ("yes", "✅\u{fe0f}"),
    ("correct", "✅\u{fe0f}"),
    ("wrong", "❌\u{fe0f}"),
    ("bad", "👎"),
    ("look", "👀"),
    ("see", "👀"),
    ("watch", "👀"),
    ("clap", "👏"),
    ("strong", "💪"),
    ("muscle", "💪"),
    ("gym", "💪"),
    ("workout", "🏋\u{fe0f}"),
    ("train", "🏋\u{fe0f}"),
    ("run", "🏃"),
    ("smart", "🧠"),
    ("brain", "🧠"),
    ("learn", "📚"),
    ("baby", "👶"),
    // things
    ("fire", "🔥"),
    ("hot", "🔥"),
    ("lit", "🔥"),
    ("money", "💰"),
    ("cash", "💵"),
    ("rich", "🤑"),
    ("pay", "💸"),
    ("dollar", "💵"),
    ("euro", "💶"),
    ("idea", "💡"),
    ("time", "⏰\u{fe0f}"),
    ("clock", "⏰\u{fe0f}"),
    ("today", "📅"),
    ("phone", "📱"),
    ("computer", "💻"),
    ("laptop", "💻"),
    ("code", "💻"),
    ("book", "📚"),
    ("read", "📚"),
    ("school", "🏫"),
    ("work", "💼"),
    ("job", "💼"),
    ("business", "💼"),
    ("home", "🏠"),
    ("house", "🏠"),
    ("car", "🚗"),
    ("drive", "🚗"),
    ("travel", "✈\u{fe0f}"),
    ("flight", "✈\u{fe0f}"),
    ("rocket", "🚀"),
    ("launch", "🚀"),
    ("growth", "📈"),
    ("grow", "📈"),
    ("camera", "📸"),
    ("photo", "📸"),
    ("video", "🎬"),
    ("movie", "🎬"),
    ("film", "🎬"),
    ("music", "🎵"),
    ("song", "🎶"),
    ("sing", "🎤"),
    ("game", "🎮"),
    ("play", "🎮"),
    ("party", "🎉"),
    ("celebrate", "🎉"),
    ("gift", "🎁"),
    ("birthday", "🎂"),
    ("win", "🏆"),
    ("winner", "🏆"),
    ("champion", "🏆"),
    ("goal", "🎯"),
    ("target", "🎯"),
    ("key", "🔑"),
    ("shop", "🛒"),
    ("buy", "🛒"),
    ("new", "🆕"),
    ("star", "⭐\u{fe0f}"),
    ("magic", "✨\u{fe0f}"),
    ("sparkle", "✨\u{fe0f}"),
    ("fast", "⚡\u{fe0f}"),
    ("quick", "⚡\u{fe0f}"),
    ("energy", "⚡\u{fe0f}"),
    ("power", "⚡\u{fe0f}"),
    ("warning", "⚠\u{fe0f}"),
    ("danger", "⚠\u{fe0f}"),
    ("stop", "🛑"),
    ("question", "❓\u{fe0f}"),
    // nature and food
    ("sun", "☀\u{fe0f}"),
    ("summer", "☀\u{fe0f}"),
    ("rain", "🌧\u{fe0f}"),
    ("snow", "❄\u{fe0f}"),
    ("cold", "🥶"),
    ("winter", "❄\u{fe0f}"),
    ("world", "🌍"),
    ("earth", "🌍"),
    ("beach", "🏖\u{fe0f}"),
    ("ocean", "🌊"),
    ("sea", "🌊"),
    ("flower", "🌸"),
    ("dog", "🐶"),
    ("cat", "🐱"),
    ("food", "🍽\u{fe0f}"),
    ("eat", "🍽\u{fe0f}"),
    ("hungry", "😋"),
    ("delicious", "😋"),
    ("tasty", "😋"),
    ("pizza", "🍕"),
    ("burger", "🍔"),
    ("coffee", "☕\u{fe0f}"),
    ("beer", "🍺"),
    ("wine", "🍷"),
    ("water", "💧"),
    ("apple", "🍎"),
    ("cake", "🎂"),
    ("healthy", "🥗"),
    ("salad", "🥗"),
    ("protein", "🥩"),
    // German
    ("liebe", "❤\u{fe0f}"),
    ("herz", "❤\u{fe0f}"),
    ("lachen", "😂"),
    ("lustig", "😂"),
    ("traurig", "😢"),
    ("krass", "🤯"),
    ("geil", "🔥"),
    ("feuer", "🔥"),
    ("geld", "💰"),
    ("idee", "💡"),
    ("zeit", "⏰\u{fe0f}"),
    ("heute", "📅"),
    ("handy", "📱"),
    ("arbeit", "💼"),
    ("haus", "🏠"),
    ("auto", "🚗"),
    ("reise", "✈\u{fe0f}"),
    ("musik", "🎵"),
    ("spiel", "🎮"),
    ("gewinnen", "🏆"),
    ("ziel", "🎯"),
    ("schnell", "⚡\u{fe0f}"),
    ("stark", "💪"),
    ("training", "🏋\u{fe0f}"),
    ("sport", "💪"),
    ("schlafen", "😴"),
    ("müde", "😴"),
    ("hallo", "👋"),
    ("danke", "🙏"),
    ("bitte", "🙏"),
    ("gut", "👍"),
    ("super", "🤩"),
    ("sonne", "☀\u{fe0f}"),
    ("welt", "🌍"),
    ("hund", "🐶"),
    ("katze", "🐱"),
    ("essen", "🍽\u{fe0f}"),
    ("lecker", "😋"),
    ("kaffee", "☕\u{fe0f}"),
    ("bier", "🍺"),
    ("wasser", "💧"),
    ("gesund", "🥗"),
];

/// Whether `word` is `keyword` or an inflection of it.
///
/// Exact for short keywords — "hi" must not fire on "his" — and a prefix with
/// at most three more letters for longer ones, which covers plurals, "-ing",
/// "-ed" and German endings without a stemmer.
fn matches_keyword(word: &str, keyword: &str) -> bool {
    if word == keyword {
        return true;
    }
    let (wl, kl) = (word.chars().count(), keyword.chars().count());
    kl >= 4 && wl > kl && wl - kl <= 3 && word.starts_with(keyword)
}

/// One emoji that fits `text`, from the first word that has one.
pub fn suggest(text: &str) -> Option<&'static str> {
    for raw in text.split_whitespace() {
        let word: String = raw
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        if word.is_empty() {
            continue;
        }
        if let Some((_, emoji)) = KEYWORDS.iter().find(|(k, _)| matches_keyword(&word, k)) {
            return Some(emoji);
        }
    }
    None
}

/// `text` with a fitting emoji appended, unless it already has one or none
/// fits.
pub fn with_auto_emoji(text: &str) -> String {
    if has_emoji(text) {
        return text.to_string();
    }
    match suggest(text) {
        Some(emoji) => format!("{} {emoji}", text.trim_end()),
        None => text.to_string(),
    }
}

/// Whether `text` holds anything the rasteriser will draw as a colour emoji.
pub fn has_emoji(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c as u32, 0x1F000..=0x1FBFF) || c == '\u{FE0F}')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every emoji either sits in the pictographic planes or carries U+FE0F,
    /// which are the two things `text::font::emoji_ranges` recognises. One that
    /// does neither renders as a monochrome outline from the caption's font.
    #[test]
    fn every_emoji_is_routed_to_the_colour_font() {
        let all = PICKER
            .iter()
            .flat_map(|(_, list)| list.iter().copied())
            .chain(KEYWORDS.iter().map(|(_, e)| *e));
        for emoji in all {
            assert!(has_emoji(emoji), "{emoji:?} would be drawn as text");
        }
    }

    #[test]
    fn keywords_match_whole_words_and_inflections() {
        assert_eq!(suggest("I love this"), Some("❤\u{fe0f}"));
        assert_eq!(suggest("Loving it!"), None, "a changed stem is not matched");
        assert_eq!(suggest("so much money."), Some("💰"));
        assert_eq!(suggest("those rockets"), Some("🚀"));
        assert_eq!(suggest("this is his"), None, "short keywords are exact");
        assert_eq!(suggest("Hi!"), Some("👋"));
        assert_eq!(suggest("Das ist Geld"), Some("💰"));
        assert_eq!(suggest("nothing to see"), Some("👀"));
        assert_eq!(suggest("zzz qqq"), None);
    }

    #[test]
    fn auto_emoji_appends_once() {
        assert_eq!(with_auto_emoji("time to eat"), "time to eat ⏰\u{fe0f}");
        assert_eq!(with_auto_emoji("already 🔥"), "already 🔥");
        assert_eq!(with_auto_emoji("plain words"), "plain words");
    }

    #[test]
    fn keywords_are_lower_case_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (keyword, _) in KEYWORDS {
            assert_eq!(*keyword, keyword.to_lowercase());
            assert!(seen.insert(*keyword), "{keyword} is listed twice");
        }
    }
}
