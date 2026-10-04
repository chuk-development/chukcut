//! Noto Animated Emoji: Google's animated emoji, as Lottie, for the Stickers
//! tab's "Animated" section.
//!
//! **Licence: CC BY 4.0**, read on the project's own page (Documentation,
//! "Can I use these animated assets commercially": "Animated Noto Emoji is
//! licensed under CC BY 4.0"), 2026-10-04. A video using one must credit
//! Google; the licence record every fetched file carries puts "Animated
//! emoji by Google, CC BY 4.0" into the export's credits file.
//!
//! **The index** is the site's own `data/api.json` (881 emoji with code
//! point, tags, category and popularity), cached for a month like the static
//! emoji index. **A file** is `…/notoemoji/latest/<code point>/lottie.json`
//! on Google's font CDN, 37 KB for a typical face, fetched once into the
//! library cache with its licence record. **A tile** is a small GIF rendered
//! from that Lottie by our own renderer (`animated::commands::animated_preview`),
//! which GPUI plays in an `img`; the CDN's own 512-px GIFs are 20 times the
//! size of the Lottie.
//!
//! Requests carry `User-Agent: chukcut/<version>` and nothing else.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::licence;
use super::net;
use crate::modules::cloud::provenance::{self, Origin, OriginKind};

/// How long the index is trusted.
pub const INDEX_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

/// The addresses. Tests point them at a local server.
#[derive(Debug, Clone)]
pub struct AnimatedSource {
    pub index: String,
    /// Where `<code point>/lottie.json` is served.
    pub files: String,
}

impl Default for AnimatedSource {
    fn default() -> Self {
        Self {
            index: "https://googlefonts.github.io/noto-emoji-animation/data/api.json".into(),
            files: "https://fonts.gstatic.com/s/e/notoemoji/latest".into(),
        }
    }
}

/// One animated emoji.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimatedEmoji {
    /// Code points, lower-case hex joined by `_` as the CDN names them:
    /// `1f600`, `1f44d_1f3fb`.
    pub codepoint: String,
    /// A readable name from its first tag: "smile with big eyes".
    pub name: String,
    pub category: String,
    /// Every tag, without colons, for search.
    pub tags: Vec<String>,
    /// Higher is more used; the index's own order.
    pub popularity: i64,
    /// The character itself, for a fallback tile.
    pub glyph: String,
}

#[derive(Deserialize)]
struct Index {
    icons: Vec<Icon>,
}

#[derive(Deserialize)]
struct Icon {
    codepoint: String,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    popularity: i64,
}

/// Parse the site's `api.json`, most popular first.
pub fn parse_index(bytes: &[u8]) -> Result<Vec<AnimatedEmoji>, String> {
    let index: Index = serde_json::from_slice(bytes)
        .map_err(|e| format!("the animated emoji list could not be read: {e}"))?;
    let mut list: Vec<AnimatedEmoji> = index
        .icons
        .into_iter()
        .filter(|icon| {
            !icon.codepoint.is_empty()
                && icon
                    .codepoint
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == '_')
        })
        .map(|icon| {
            let tags: Vec<String> = icon
                .tags
                .iter()
                .map(|t| t.trim_matches(':').replace('-', " "))
                .filter(|t| !t.is_empty())
                .collect();
            let glyph = icon
                .codepoint
                .split('_')
                .filter_map(|cp| u32::from_str_radix(cp, 16).ok())
                .filter_map(char::from_u32)
                .collect();
            AnimatedEmoji {
                name: tags
                    .first()
                    .cloned()
                    .unwrap_or_else(|| icon.codepoint.clone()),
                codepoint: icon.codepoint.to_ascii_lowercase(),
                category: icon.categories.first().cloned().unwrap_or_default(),
                tags,
                popularity: icon.popularity,
                glyph,
            }
        })
        .collect();
    list.sort_by_key(|icon| std::cmp::Reverse(icon.popularity));
    Ok(list)
}

/// The index, through the copy in `dir`. The flag is "an old copy, because
/// the network did not answer".
pub fn index(source: &AnimatedSource, dir: &Path) -> Result<(Vec<AnimatedEmoji>, bool), String> {
    let fetched = net::cached(
        &source.index,
        &dir.join("noto-animated.json"),
        INDEX_TTL,
        "the animated emoji list",
    )?;
    Ok((parse_index(&fetched.bytes)?, fetched.stale))
}

/// The emoji matching `query` (name, tag, glyph or code point) in
/// `category` (any when `None`), in index order.
pub fn search<'a>(
    list: &'a [AnimatedEmoji],
    query: &str,
    category: Option<&str>,
) -> Vec<&'a AnimatedEmoji> {
    let query = query.trim().to_lowercase();
    list.iter()
        .filter(|e| category.is_none_or(|c| e.category.eq_ignore_ascii_case(c)))
        .filter(|e| {
            query.is_empty()
                || e.name.to_lowercase().contains(&query)
                || e.tags.iter().any(|t| t.to_lowercase().contains(&query))
                || e.glyph == query
                || e.codepoint == query
        })
        .collect()
}

/// The categories, in the order the index first uses them.
pub fn categories(list: &[AnimatedEmoji]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in list {
        if !e.category.is_empty() && !out.contains(&e.category) {
            out.push(e.category.clone());
        }
    }
    out
}

/// The licence record an animated emoji carries.
pub fn origin(emoji: &AnimatedEmoji) -> Origin {
    Origin {
        title: emoji.name.clone(),
        source_id: emoji.codepoint.clone(),
        creator: "Google".into(),
        source_url: "https://googlefonts.github.io/noto-emoji-animation/".into(),
        licence: licence::spdx("CC-BY-4.0"),
        credit: format!(
            "\"{}\" from Animated emoji by Google, CC BY 4.0",
            emoji.name
        ),
        ..Origin::new(OriginKind::Library, "noto-animated-emoji")
    }
}

/// The emoji's Lottie with its licence record, under `root` (normally the
/// library cache): what goes on the timeline.
pub fn fetch(
    source: &AnimatedSource,
    emoji: &AnimatedEmoji,
    root: &Path,
) -> Result<PathBuf, String> {
    let stem = provenance::slug(&format!("{} animated", emoji.name));
    let path = root
        .join("noto-animated-emoji")
        .join(&emoji.codepoint)
        .join(format!("{stem}.json"));
    if path.exists() && provenance::read_sidecar(&path).is_some() {
        return Ok(path);
    }
    let url = format!("{}/{}/lottie.json", source.files, emoji.codepoint);
    let what = format!("the animated sticker \"{}\"", emoji.name);
    net::download(&url, &path).map_err(|e| net::unreachable(&what, &e))?;
    // A body that is not a Lottie (an error page) must not stay as if it were.
    if let Err(error) = crate::modules::animated::lottie::LottieAnimation::open(&path) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{what} did not arrive as an animation: {error}"));
    }
    provenance::write_sidecar(&path, &origin(emoji))?;
    Ok(path)
}

/// A moving tile for the panel: the Lottie fetched (into `root`) and
/// rendered as a small looping GIF into `thumbs`.
pub fn preview(
    source: &AnimatedSource,
    emoji: &AnimatedEmoji,
    root: &Path,
    thumbs: &Path,
    side: u32,
) -> Result<PathBuf, String> {
    let out = thumbs.join(format!("noto-animated-{}-{side}.gif", emoji.codepoint));
    if out.exists() {
        return Ok(out);
    }
    let lottie = fetch(source, emoji, root)?;
    crate::modules::animated::commands::animated_preview(&lottie, side, &out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"host":"","families":["Animated Emoji"],"icons":[
      {"name":"emoji_u1f603","codepoint":"1f603","categories":["Smileys and emotions"],"tags":[":smile-with-big-eyes:"],"popularity":880},
      {"name":"emoji_u1f600","codepoint":"1f600","categories":["Smileys and emotions"],"tags":[":smile:",":grin:"],"popularity":881},
      {"name":"emoji_u1f44d_1f3fb","codepoint":"1f44d_1f3fb","categories":["People"],"tags":[":thumbs-up:"],"popularity":5},
      {"name":"broken","codepoint":"<script>","tags":[]}
    ]}"#;

    #[test]
    fn the_index_reads_names_tags_and_glyphs_and_sorts_by_popularity() {
        let list = parse_index(SAMPLE.as_bytes()).unwrap();
        assert_eq!(list.len(), 3, "the malformed code point is dropped");
        assert_eq!(list[0].codepoint, "1f600");
        assert_eq!(list[0].name, "smile");
        assert_eq!(list[0].glyph, "😀");
        assert_eq!(list[1].name, "smile with big eyes");
        assert_eq!(list[2].glyph, "👍🏻");
        assert_eq!(categories(&list), vec!["Smileys and emotions", "People"]);
    }

    #[test]
    fn search_finds_by_name_tag_glyph_and_category() {
        let list = parse_index(SAMPLE.as_bytes()).unwrap();
        assert_eq!(search(&list, "grin", None).len(), 1);
        assert_eq!(search(&list, "smile", None).len(), 2);
        assert_eq!(search(&list, "😀", None)[0].codepoint, "1f600");
        assert_eq!(search(&list, "", Some("people")).len(), 1);
    }

    #[test]
    fn the_record_credits_google_under_cc_by() {
        let list = parse_index(SAMPLE.as_bytes()).unwrap();
        let origin = origin(&list[0]);
        assert_eq!(origin.licence.id, "CC-BY-4.0");
        assert!(origin.licence.attribution_required);
        assert!(origin.credit.contains("Google"));
    }
}
