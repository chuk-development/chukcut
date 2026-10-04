//! Stickers: emoji from Fluent Emoji (MIT) and Noto Emoji (Apache-2.0), and
//! icons from the Iconify sets whose licence a creator can use.
//!
//! **The emoji index** joins two public files: Unicode's `emoji-test.txt`
//! (every emoji with its code points, name and group — the category column)
//! and the file tree of `microsoft/fluentui-emoji`, whose folders are named
//! after the same CLDR names. The joined index is stored as one small JSON
//! file and kept for a month; offline, the stored copy answers.
//!
//! **Three looks.** Fluent 3D is the closest free match to CapCut's sticker
//! look but is 256 px; Fluent Flat is an SVG we rasterise at 1024 px; Noto is
//! Google's 512 px PNG. Skin-tone variants are left out: the default is what
//! a sticker panel shows.
//!
//! **Icons** come from the Iconify API: a search, then one SVG rasterised
//! here with `resvg`. Sets are filtered by their SPDX licence through the
//! policy layer, and sets that are logos, programming marks or unmaintained
//! are left out — a brand mark on a video is a trademark question no licence
//! answers.
//!
//! **Animated stickers** (Noto Animated Emoji, Lottie) are in
//! `animated_emoji`; they are drawn by `modules::animated`.
//!
//! A sticker on the timeline is an image clip on an overlay lane, centred and
//! scaled to 40% of the short side, so it moves, keyframes and follows a
//! motion track like any other overlay ([`place`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::licence::{self, Policy, Use};
use super::net;
use crate::modules::cloud::provenance::{self, Licence, Origin, OriginKind};
use crate::modules::project::{
    new_id, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
};
use crate::modules::timeline::ops::EditCommand;

/// How long the joined emoji index is trusted.
pub const INDEX_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// How long the list of icon sets is trusted.
pub const SETS_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// How long a sticker lasts on the timeline when it is added.
pub const STICKER_DURATION: Micros = 3_000_000;
/// A new sticker's size: this fraction of the canvas's short side.
pub const STICKER_SCALE: f32 = 0.4;

/// The addresses the stickers come from. Tests point them at a local server.
#[derive(Debug, Clone)]
pub struct StickerSource {
    pub emoji_test: String,
    /// The GitHub tree API of the Fluent repository.
    pub fluent_tree: String,
    /// Where Fluent files are served, ending in the repository root.
    pub fluent_files: String,
    /// Where Noto's PNG directories are served (`…/2D/png`).
    pub noto_files: String,
    pub iconify: String,
}

impl Default for StickerSource {
    fn default() -> Self {
        Self {
            emoji_test: "https://unicode.org/Public/emoji/latest/emoji-test.txt".into(),
            fluent_tree:
                "https://api.github.com/repos/microsoft/fluentui-emoji/git/trees/main?recursive=1"
                    .into(),
            fluent_files: "https://cdn.jsdelivr.net/gh/microsoft/fluentui-emoji@main".into(),
            noto_files: "https://cdn.jsdelivr.net/gh/googlefonts/noto-emoji@main/2D/png".into(),
            iconify: "https://api.iconify.design".into(),
        }
    }
}

/// Which drawing of an emoji.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StickerStyle {
    Fluent3d,
    FluentFlat,
    Noto,
}

impl StickerStyle {
    pub const ALL: [StickerStyle; 3] = [
        StickerStyle::Fluent3d,
        StickerStyle::FluentFlat,
        StickerStyle::Noto,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StickerStyle::Fluent3d => "3D",
            StickerStyle::FluentFlat => "Flat",
            StickerStyle::Noto => "Noto",
        }
    }

    fn provider(self) -> &'static str {
        match self {
            StickerStyle::Fluent3d | StickerStyle::FluentFlat => "fluent-emoji",
            StickerStyle::Noto => "noto-emoji",
        }
    }

    /// The licence record a sticker in this style carries.
    pub fn origin(self, sticker: &Sticker) -> Origin {
        let (licence, creator, url, credit) = match self {
            StickerStyle::Fluent3d | StickerStyle::FluentFlat => (
                licence::spdx("MIT"),
                "Microsoft",
                "https://github.com/microsoft/fluentui-emoji",
                "Fluent Emoji by Microsoft, MIT License",
            ),
            StickerStyle::Noto => (
                licence::spdx("Apache-2.0"),
                "Google",
                "https://github.com/googlefonts/noto-emoji",
                "Noto Emoji by Google, Apache License 2.0",
            ),
        };
        Origin {
            title: sticker.name.clone(),
            source_id: sticker.id.clone(),
            creator: creator.into(),
            source_url: url.into(),
            licence,
            credit: format!("\"{}\" from {credit}", sticker.name),
            ..Origin::new(OriginKind::Library, self.provider())
        }
    }
}

/// One emoji.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sticker {
    /// Code points, lower-case hex joined by `-`: `1f44d`, `1f468-200d-1f4bb`.
    pub id: String,
    /// The CLDR name: "thumbs up".
    pub name: String,
    /// The Unicode group: "Smileys & Emotion".
    pub group: String,
    /// The character itself, for search and a fallback tile.
    pub glyph: String,
    /// Paths in the Fluent repository, when Fluent draws it.
    #[serde(default)]
    pub fluent_3d: Option<String>,
    #[serde(default)]
    pub fluent_flat: Option<String>,
}

impl Sticker {
    /// Whether this emoji exists in `style`.
    pub fn has(&self, style: StickerStyle) -> bool {
        match style {
            StickerStyle::Fluent3d => self.fluent_3d.is_some(),
            StickerStyle::FluentFlat => self.fluent_flat.is_some(),
            StickerStyle::Noto => true,
        }
    }

    /// Noto's file name: `emoji_u1f44d.png`, variation selectors dropped.
    pub fn noto_file(&self) -> String {
        let points: Vec<&str> = self.id.split('-').filter(|p| *p != "fe0f").collect();
        format!("emoji_u{}.png", points.join("_"))
    }
}

/// The emoji index.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StickerIndex {
    pub stickers: Vec<Sticker>,
    /// The groups in Unicode's order, for the category column.
    pub groups: Vec<String>,
    /// Set when the index came from an old copy because the network did not
    /// answer.
    #[serde(skip)]
    pub stale: bool,
}

impl StickerIndex {
    /// Stickers of `group` (all groups when `None`) whose name or glyph
    /// matches `query`, that exist in `style`.
    pub fn search(&self, query: &str, group: Option<&str>, style: StickerStyle) -> Vec<Sticker> {
        let query = query.trim().to_lowercase();
        self.stickers
            .iter()
            .filter(|s| s.has(style))
            .filter(|s| group.is_none_or(|g| s.group == g))
            .filter(|s| query.is_empty() || s.name.contains(&query) || s.glyph == query)
            .cloned()
            .collect()
    }
}

/// A name as a join key: lower case, letters and digits only, apostrophes
/// dropped ("o’clock" and "oclock" agree), runs of anything else one space.
fn key(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if c == '\'' || c == '\u{2019}' {
            continue;
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// Read `emoji-test.txt`: fully-qualified emoji without skin tones, in file
/// order, with their group.
pub fn parse_emoji_test(text: &str) -> (Vec<Sticker>, Vec<String>) {
    const TONES: [&str; 5] = ["1f3fb", "1f3fc", "1f3fd", "1f3fe", "1f3ff"];
    let mut stickers = Vec::new();
    let mut groups: Vec<String> = Vec::new();
    let mut group = String::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("# group:") {
            group = name.trim().to_string();
            if group != "Component" {
                groups.push(group.clone());
            }
            continue;
        }
        if line.starts_with('#') || line.trim().is_empty() || group == "Component" {
            continue;
        }
        let Some((points, rest)) = line.split_once(';') else {
            continue;
        };
        let Some((status, comment)) = rest.split_once('#') else {
            continue;
        };
        if status.trim() != "fully-qualified" {
            continue;
        }
        let id: Vec<String> = points
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        if id.iter().any(|p| TONES.contains(&p.as_str())) {
            continue;
        }
        // "😀 E1.0 grinning face"
        let mut words = comment.split_whitespace();
        let glyph = words.next().unwrap_or_default().to_string();
        let _version = words.next();
        let name = words.collect::<Vec<_>>().join(" ").to_lowercase();
        stickers.push(Sticker {
            id: id.join("-"),
            name,
            group: group.clone(),
            glyph,
            fluent_3d: None,
            fluent_flat: None,
        });
    }
    (stickers, groups)
}

/// Attach Fluent's file paths from its repository tree (the GitHub tree API's
/// JSON) to the stickers whose names match a Fluent folder.
pub fn attach_fluent(stickers: &mut [Sticker], tree: &serde_json::Value) {
    let mut three_d: HashMap<String, String> = HashMap::new();
    let mut flat: HashMap<String, String> = HashMap::new();
    let entries = tree
        .get("tree")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    for entry in entries {
        let Some(path) = entry.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        let parts: Vec<&str> = path.split('/').collect();
        // assets/<Folder>/3D/x.png, or assets/<Folder>/Default/3D/x.png for
        // emoji with skin tones.
        let (folder, kind) = match parts.as_slice() {
            ["assets", folder, kind, _file] => (*folder, *kind),
            ["assets", folder, "Default", kind, _file] => (*folder, *kind),
            _ => continue,
        };
        match kind {
            "3D" if path.ends_with(".png") => {
                three_d.insert(key(folder), path.to_string());
            }
            "Flat" if path.ends_with(".svg") => {
                flat.insert(key(folder), path.to_string());
            }
            _ => {}
        }
    }
    for sticker in stickers {
        let k = key(&sticker.name);
        sticker.fluent_3d = three_d.get(&k).cloned();
        sticker.fluent_flat = flat.get(&k).cloned();
    }
}

/// The emoji index: the stored copy while it is younger than [`INDEX_TTL`],
/// otherwise rebuilt from the two sources, and the stored copy again when
/// they cannot be reached.
pub fn index(source: &StickerSource, dir: &Path) -> Result<StickerIndex, String> {
    let path = dir.join("stickers.json");
    let read = |stale: bool| -> Option<StickerIndex> {
        let mut index: StickerIndex = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        index.stale = stale;
        Some(index)
    };
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < INDEX_TTL);
    if fresh {
        if let Some(index) = read(false) {
            return Ok(index);
        }
    }
    let built = (|| -> Result<StickerIndex, String> {
        let text = net::get(&source.emoji_test)?;
        let (mut stickers, groups) = parse_emoji_test(&String::from_utf8_lossy(&text));
        if stickers.is_empty() {
            return Err("the emoji list was empty".into());
        }
        // Fluent is a bonus: without its tree the Noto look still works.
        match net::get(&source.fluent_tree)
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        {
            Ok(tree) => attach_fluent(&mut stickers, &tree),
            Err(error) => tracing::warn!(%error, "no Fluent Emoji index"),
        }
        Ok(StickerIndex {
            stickers,
            groups,
            stale: false,
        })
    })();
    match built {
        Ok(index) => {
            let _ = std::fs::create_dir_all(dir);
            if let Ok(text) = serde_json::to_vec(&index) {
                let _ = std::fs::write(&path, text);
            }
            Ok(index)
        }
        Err(error) => read(true).ok_or_else(|| net::unreachable("the emoji list", &error)),
    }
}

// --- files ---------------------------------------------------------------------

/// Rasterise an SVG to a square PNG `size` pixels on a side, keeping its
/// aspect ratio inside the square.
pub fn svg_to_png(svg: &[u8], size: u32, dest: &Path) -> Result<(), String> {
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default())
        .map_err(|e| format!("the drawing did not read ({e})"))?;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(size, size).ok_or("the picture size is not usable")?;
    let view = tree.size();
    let scale = (size as f32 / view.width()).min(size as f32 / view.height());
    let dx = (size as f32 - view.width() * scale) / 2.0;
    let dy = (size as f32 - view.height() * scale) / 2.0;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy),
        &mut pixmap.as_mut(),
    );
    // tiny-skia holds premultiplied RGBA; PNG wants it straight.
    let mut pixels = pixmap.take();
    for px in pixels.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let image = image::RgbaImage::from_raw(size, size, pixels).ok_or("bad picture size")?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let part = dest.with_extension("part.png");
    image
        .save(&part)
        .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    std::fs::rename(&part, dest).map_err(|e| format!("cannot write {}: {e}", dest.display()))
}

/// Where a sticker's file is fetched from.
fn sticker_url(
    source: &StickerSource,
    sticker: &Sticker,
    style: StickerStyle,
    thumb: bool,
) -> Option<String> {
    let fluent = |path: &str| {
        format!(
            "{}/{}",
            source.fluent_files,
            path.split('/')
                .map(crate::modules::cloud::http::encode)
                .collect::<Vec<_>>()
                .join("/")
        )
    };
    match style {
        StickerStyle::Fluent3d => sticker.fluent_3d.as_deref().map(fluent),
        StickerStyle::FluentFlat => sticker.fluent_flat.as_deref().map(fluent),
        StickerStyle::Noto => Some(format!(
            "{}/{}/{}",
            source.noto_files,
            if thumb { 128 } else { 512 },
            sticker.noto_file()
        )),
    }
}

/// A small picture of a sticker for the panel, fetched once into `dir`.
pub fn thumb(
    source: &StickerSource,
    sticker: &Sticker,
    style: StickerStyle,
    dir: &Path,
) -> Result<PathBuf, String> {
    let url = sticker_url(source, sticker, style, true)
        .ok_or_else(|| format!("{} has no {} drawing", sticker.name, style.label()))?;
    let name = format!("{}-{}", style.provider(), sticker.id);
    match style {
        StickerStyle::FluentFlat => {
            let png = dir.join(format!("{name}-flat.png"));
            if !png.exists() {
                let svg = dir.join(format!("{name}-flat.svg"));
                net::once(&url, &svg, "a sticker preview")?;
                svg_to_png(&std::fs::read(&svg).map_err(|e| e.to_string())?, 128, &png)?;
            }
            Ok(png)
        }
        _ => {
            let path = dir.join(format!("{name}-{}.png", style.label().to_lowercase()));
            net::once(&url, &path, "a sticker preview")?;
            Ok(path)
        }
    }
}

/// The sticker's full-size file with its licence record, under `root`
/// (normally the library cache): what goes on the timeline.
pub fn fetch(
    source: &StickerSource,
    sticker: &Sticker,
    style: StickerStyle,
    root: &Path,
) -> Result<PathBuf, String> {
    let url = sticker_url(source, sticker, style, false)
        .ok_or_else(|| format!("{} has no {} drawing", sticker.name, style.label()))?;
    let stem = provenance::slug(&format!("{} {}", sticker.name, style.label()));
    let dir = root.join(style.provider()).join(&stem);
    let path = dir.join(format!("{stem}.png"));
    if path.exists() && provenance::read_sidecar(&path).is_some() {
        return Ok(path);
    }
    let what = format!("the sticker \"{}\"", sticker.name);
    match style {
        StickerStyle::FluentFlat => {
            let svg = net::get(&url).map_err(|e| net::unreachable(&what, &e))?;
            svg_to_png(&svg, 1024, &path)?;
        }
        _ => {
            net::download(&url, &path).map_err(|e| net::unreachable(&what, &e))?;
        }
    }
    provenance::write_sidecar(&path, &style.origin(sticker))?;
    Ok(path)
}

// --- icons ---------------------------------------------------------------------

/// One Iconify set a sticker may come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IconSet {
    pub prefix: String,
    pub name: String,
    pub author: String,
    #[serde(default)]
    pub author_url: String,
    pub licence: Licence,
    /// Multicoloured: drawn as is. Otherwise the icon is one colour and is
    /// drawn white, which reads on most footage.
    pub palette: bool,
    pub policy: Policy,
}

/// Categories of sets that are never offered as stickers.
const EXCLUDED_CATEGORIES: [&str; 3] = ["Logos", "Programming", "Archive / Unmaintained"];

/// The icon sets the policy allows, by prefix.
pub fn icon_sets(source: &StickerSource, dir: &Path) -> Result<Vec<IconSet>, String> {
    let fetched = net::cached(
        &format!("{}/collections", source.iconify),
        &dir.join("iconify-collections.json"),
        SETS_TTL,
        "the icon sets from iconify.design",
    )?;
    let value: serde_json::Value = serde_json::from_slice(&fetched.bytes)
        .map_err(|e| format!("the icon sets did not read ({e})"))?;
    let Some(map) = value.as_object() else {
        return Err("the icon sets did not read".into());
    };
    let mut sets: Vec<IconSet> = map
        .iter()
        .filter_map(|(prefix, set)| {
            let category = set.get("category").and_then(|c| c.as_str()).unwrap_or("");
            if EXCLUDED_CATEGORIES.contains(&category) || set.get("hidden").is_some() {
                return None;
            }
            let spdx = set
                .pointer("/license/spdx")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let mut licence = licence::spdx(spdx);
            if let Some(url) = set.pointer("/license/url").and_then(|s| s.as_str()) {
                licence.url = url.to_string();
            }
            let policy = licence::policy(&licence, Use::Picture, false);
            (policy != Policy::Hide).then(|| IconSet {
                prefix: prefix.clone(),
                name: set
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or(prefix)
                    .to_string(),
                author: set
                    .pointer("/author/name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                author_url: set
                    .pointer("/author/url")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                licence,
                palette: set
                    .get("palette")
                    .and_then(|p| p.as_bool())
                    .unwrap_or(false),
                policy,
            })
        })
        .collect();
    sets.sort_by(|a, b| a.prefix.cmp(&b.prefix));
    Ok(sets)
}

/// One icon from a search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IconHit {
    pub prefix: String,
    pub name: String,
    pub set: IconSet,
}

impl IconHit {
    pub fn id(&self) -> String {
        format!("{}:{}", self.prefix, self.name)
    }

    fn svg_url(&self, source: &StickerSource, size: u32) -> String {
        let colour = if self.set.palette {
            String::new()
        } else {
            "&color=%23ffffff".to_string()
        };
        format!(
            "{}/{}/{}.svg?height={size}{colour}",
            source.iconify, self.prefix, self.name
        )
    }

    pub fn origin(&self) -> Origin {
        let by = if self.set.author.is_empty() {
            String::new()
        } else {
            format!(" by {}", self.set.author)
        };
        Origin {
            title: self.name.replace('-', " "),
            source_id: self.id(),
            creator: self.set.author.clone(),
            creator_url: self.set.author_url.clone(),
            source_url: format!(
                "https://icon-sets.iconify.design/{}/{}/",
                self.prefix, self.name
            ),
            licence: self.set.licence.clone(),
            credit: format!(
                "\"{}\" from {}{by}, {}",
                self.name, self.set.name, self.set.licence.name
            ),
            ..Origin::new(OriginKind::Library, "iconify")
        }
    }
}

/// Search the allowed icon sets.
pub fn icon_search(
    source: &StickerSource,
    sets: &[IconSet],
    query: &str,
    limit: u32,
) -> Result<Vec<IconHit>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let by_prefix: HashMap<&str, &IconSet> = sets.iter().map(|s| (s.prefix.as_str(), s)).collect();
    let url = format!(
        "{}/search?query={}&limit={}",
        source.iconify,
        crate::modules::cloud::http::encode(query),
        limit.clamp(32, 999)
    );
    let bytes = net::get(&url).map_err(|e| net::unreachable("icons from iconify.design", &e))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("the icon search did not read ({e})"))?;
    Ok(value
        .get("icons")
        .and_then(|i| i.as_array())
        .into_iter()
        .flatten()
        .filter_map(|icon| icon.as_str())
        .filter_map(|id| {
            let (prefix, name) = id.split_once(':')?;
            let set = by_prefix.get(prefix)?;
            Some(IconHit {
                prefix: prefix.to_string(),
                name: name.to_string(),
                set: (*set).clone(),
            })
        })
        .collect())
}

/// An icon's tile, rasterised once into `dir`.
pub fn icon_thumb(source: &StickerSource, icon: &IconHit, dir: &Path) -> Result<PathBuf, String> {
    let png = dir.join(format!(
        "icon-{}-{}.png",
        icon.prefix,
        provenance::slug(&icon.name)
    ));
    if png.exists() {
        return Ok(png);
    }
    let svg = net::get(&icon.svg_url(source, 128))
        .map_err(|e| net::unreachable("an icon preview", &e))?;
    svg_to_png(&svg, 128, &png)?;
    Ok(png)
}

/// The icon as a 1024 px sticker with its licence record, under `root`.
pub fn icon_fetch(source: &StickerSource, icon: &IconHit, root: &Path) -> Result<PathBuf, String> {
    let stem = provenance::slug(&format!("{} {}", icon.prefix, icon.name));
    let path = root.join("iconify").join(&stem).join(format!("{stem}.png"));
    if path.exists() && provenance::read_sidecar(&path).is_some() {
        return Ok(path);
    }
    let svg = net::get(&icon.svg_url(source, 1024))
        .map_err(|e| net::unreachable(&format!("the icon \"{}\"", icon.name), &e))?;
    svg_to_png(&svg, 1024, &path)?;
    provenance::write_sidecar(&path, &icon.origin())?;
    Ok(path)
}

// --- the timeline --------------------------------------------------------------

/// Put the image `material_id` on the timeline as a sticker at `at`: on the
/// first unlocked overlay lane (a video lane above the first one) that is
/// free there, or on a new lane on top. Centred, at [`STICKER_SCALE`].
pub fn place(project: &Project, material_id: &str, at: Micros) -> Result<EditCommand, String> {
    if project.materials.image(material_id).is_none() {
        return Err("the sticker is not in the project".into());
    }
    let start = at.max(0);
    let range = TimeRange::new(start, STICKER_DURATION);
    let segment = Segment {
        id: new_id(),
        material_id: material_id.to_string(),
        target_range: range,
        source_range: TimeRange::new(0, STICKER_DURATION),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform {
            scale: [STICKER_SCALE, STICKER_SCALE],
            ..Transform::default()
        },
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    };
    let first_video = project
        .tracks
        .iter()
        .position(|t| t.kind == TrackKind::Video);
    let lane = project.tracks.iter().enumerate().find(|(i, t)| {
        t.kind == TrackKind::Video
            && first_video.is_some_and(|first| *i > first)
            && !t.locked
            && t.is_range_free(&range, None)
    });
    Ok(match lane {
        Some((_, track)) => EditCommand::InsertSegment {
            track_id: track.id.clone(),
            index: track
                .segments
                .iter()
                .filter(|s| s.target_range.start < start)
                .count(),
            segment,
        },
        None => {
            let track = Track::new(TrackKind::Video, "Stickers");
            let track_id = track.id.clone();
            EditCommand::Composite {
                label: "Add sticker".to_string(),
                commands: vec![
                    EditCommand::AddTrack {
                        track,
                        index: project.tracks.len(),
                    },
                    EditCommand::InsertSegment {
                        track_id,
                        index: 0,
                        segment,
                    },
                ],
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::cloud::http::test_server;
    use crate::modules::project::{CanvasConfig, ImageMaterial, VideoMaterial};
    use crate::modules::timeline::history::History;

    const EMOJI_TEST: &str = "\
# group: Smileys & Emotion
# subgroup: face-smiling
1F600                                                  ; fully-qualified     # 😀 E1.0 grinning face
263A FE0F                                              ; fully-qualified     # ☺️ E0.6 smiling face
263A                                                   ; unqualified         # ☺ E0.6 smiling face
# group: People & Body
1F44D                                                  ; fully-qualified     # 👍 E0.6 thumbs up
1F44D 1F3FB                                            ; fully-qualified     # 👍🏻 E1.0 thumbs up: light skin tone
1F570 FE0F                                             ; fully-qualified     # 🕰️ E0.7 mantelpiece clock
1F55B                                                  ; fully-qualified     # 🕛 E0.6 twelve o’clock
# group: Component
1F3FB                                                  ; fully-qualified     # 🏻 E1.0 light skin tone
";

    #[test]
    fn the_unicode_list_gives_names_groups_and_no_skin_tones() {
        let (stickers, groups) = parse_emoji_test(EMOJI_TEST);
        assert_eq!(groups, ["Smileys & Emotion", "People & Body"]);
        let ids: Vec<&str> = stickers.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["1f600", "263a-fe0f", "1f44d", "1f570-fe0f", "1f55b"]);
        assert_eq!(stickers[2].name, "thumbs up");
        assert_eq!(stickers[2].glyph, "👍");
        assert_eq!(stickers[1].noto_file(), "emoji_u263a.png");
    }

    #[test]
    fn fluent_folders_join_by_name() {
        let (mut stickers, _) = parse_emoji_test(EMOJI_TEST);
        let tree = serde_json::json!({"tree": [
            {"path": "assets/Grinning face/3D/grinning_face_3d.png"},
            {"path": "assets/Grinning face/Flat/grinning_face_flat.svg"},
            {"path": "assets/Thumbs up/Default/3D/thumbs_up_3d_default.png"},
            {"path": "assets/Thumbs up/Dark/3D/thumbs_up_3d_dark.png"},
            {"path": "assets/Twelve oclock/3D/twelve_oclock_3d.png"},
        ]});
        attach_fluent(&mut stickers, &tree);
        assert_eq!(
            stickers[0].fluent_3d.as_deref(),
            Some("assets/Grinning face/3D/grinning_face_3d.png")
        );
        assert!(stickers[0].fluent_flat.is_some());
        assert_eq!(
            stickers[2].fluent_3d.as_deref(),
            Some("assets/Thumbs up/Default/3D/thumbs_up_3d_default.png")
        );
        assert!(stickers[4].fluent_3d.is_some(), "o’clock matches oclock");
        assert!(stickers[1].fluent_3d.is_none());
        let index = StickerIndex {
            stickers,
            groups: vec![],
            stale: false,
        };
        assert_eq!(index.search("", None, StickerStyle::Fluent3d).len(), 3);
        assert_eq!(index.search("thumbs", None, StickerStyle::Noto).len(), 1);
        assert_eq!(
            index
                .search("", Some("People & Body"), StickerStyle::Noto)
                .len(),
            3
        );
    }

    #[test]
    fn the_index_is_kept_and_survives_going_offline() {
        let dir = super::super::scratch("stickers-index");
        let server = test_server::serve(vec![
            (200, "text/plain", EMOJI_TEST.as_bytes().to_vec()),
            (500, "text/plain", b"down".to_vec()),
        ]);
        let source = StickerSource {
            emoji_test: format!("{}/emoji-test.txt", server.url),
            fluent_tree: format!("{}/tree", server.url),
            ..StickerSource::default()
        };
        let index = index(&source, &dir).unwrap();
        assert_eq!(index.stickers.len(), 5);
        assert!(!index.stale);
        // The server is gone; the stored copy answers.
        let again = super::index(&source, &dir).unwrap();
        assert_eq!(again.stickers.len(), 5);
    }

    #[test]
    fn a_sticker_lands_with_its_licence_record() {
        let root = super::super::scratch("stickers-fetch");
        let mut png = Vec::new();
        image::RgbaImage::new(4, 4)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let server = test_server::serve(vec![(200, "image/png", png.clone())]);
        let source = StickerSource {
            noto_files: server.url.clone(),
            ..StickerSource::default()
        };
        let (stickers, _) = parse_emoji_test(EMOJI_TEST);
        let path = fetch(&source, &stickers[2], StickerStyle::Noto, &root).unwrap();
        assert!(path.ends_with("noto-emoji/thumbs-up-noto/thumbs-up-noto.png"));
        let origin = provenance::read_sidecar(&path).unwrap();
        assert_eq!(origin.kind, OriginKind::Library);
        assert_eq!(origin.licence.id, "Apache-2.0");
        assert!(origin.credit.contains("Noto Emoji by Google"));
        assert_eq!(
            server.requests.lock().unwrap()[0].request_line,
            "GET /512/emoji_u1f44d.png HTTP/1.1"
        );
        // Cached: no second request.
        assert_eq!(
            fetch(&source, &stickers[2], StickerStyle::Noto, &root).unwrap(),
            path
        );
    }

    #[test]
    fn svg_icons_rasterise_to_square_pngs() {
        let dir = super::super::scratch("stickers-svg");
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 12"><rect width="24" height="12" fill="#fff"/></svg>"##;
        let path = dir.join("wide.png");
        svg_to_png(svg, 64, &path).unwrap();
        let image = image::open(&path).unwrap().to_rgba8();
        assert_eq!((image.width(), image.height()), (64, 64));
        assert_eq!(image.get_pixel(32, 32).0, [255, 255, 255, 255]);
        assert_eq!(image.get_pixel(32, 2).0[3], 0, "letterboxed, not stretched");
    }

    #[test]
    fn icon_sets_are_filtered_by_licence_and_kind() {
        let dir = super::super::scratch("stickers-sets");
        let body = serde_json::json!({
            "mdi": {"name": "Material Design Icons", "author": {"name": "Pictogrammers"}, "license": {"spdx": "Apache-2.0"}, "category": "UI 24px"},
            "logos": {"name": "SVG Logos", "license": {"spdx": "CC0-1.0"}, "category": "Logos"},
            "nc": {"name": "NC set", "license": {"spdx": "CC-BY-NC-4.0"}, "category": "UI 24px"},
            "gpl": {"name": "GPL set", "license": {"spdx": "GPL-3.0"}, "category": "UI 24px"},
            "openmoji": {"name": "OpenMoji", "license": {"spdx": "CC-BY-SA-4.0"}, "category": "Emoji", "palette": true}
        });
        let server = test_server::serve(vec![(
            200,
            "application/json",
            body.to_string().into_bytes(),
        )]);
        let source = StickerSource {
            iconify: server.url.clone(),
            ..StickerSource::default()
        };
        let sets = icon_sets(&source, &dir).unwrap();
        let prefixes: Vec<&str> = sets.iter().map(|s| s.prefix.as_str()).collect();
        assert_eq!(prefixes, ["mdi", "openmoji"]);
        assert_eq!(sets[1].policy, Policy::Warn);
        assert!(sets[1].palette);
    }

    fn project() -> Project {
        let mut project = Project::new("p", CanvasConfig::default(), 30.0);
        project.materials.videos.push(VideoMaterial {
            id: "v".into(),
            path: "/x/v.mp4".into(),
            width: 1080,
            height: 1920,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        project.materials.images.push(ImageMaterial {
            id: "s".into(),
            path: "/x/s.png".into(),
            width: 512,
            height: 512,
        });
        let mut main = Track::new(TrackKind::Video, "Main");
        main.segments.push(Segment {
            id: "clip".into(),
            material_id: "v".into(),
            target_range: TimeRange::new(0, 10_000_000),
            source_range: TimeRange::new(0, 10_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        project.tracks.push(main);
        project
    }

    #[test]
    fn a_sticker_goes_on_an_overlay_lane_never_the_main_one() {
        let mut project = project();
        // The main lane is free at 12 s, but a sticker still goes above it.
        let command = place(&project, "s", 12_000_000).unwrap();
        History::default().apply(&mut project, command).unwrap();
        assert_eq!(project.tracks.len(), 2);
        let sticker = &project.tracks[1].segments[0];
        assert_eq!(sticker.material_id, "s");
        assert_eq!(sticker.transform.scale, [STICKER_SCALE, STICKER_SCALE]);
        assert_eq!(sticker.transform.position, [0.0, 0.0]);
        // A second one at another time shares the overlay lane.
        let command = place(&project, "s", 1_000_000).unwrap();
        History::default().apply(&mut project, command).unwrap();
        assert_eq!(project.tracks.len(), 2);
        assert_eq!(project.tracks[1].segments.len(), 2);
        assert!(project
            .validate()
            .iter()
            .all(|i| i.severity != crate::modules::project::Severity::Error));
        assert!(place(&project, "v", 0).is_err(), "only images are stickers");
    }
}
