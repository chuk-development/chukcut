//! Fonts from Fontsource: the catalogue, preview tiles, download on use.
//!
//! - **Catalogue**: `api.fontsource.org/v1/fonts`, no key, about 2,100
//!   families with their SPDX licence (OFL-1.1 for almost all). Cached for a
//!   week; offline, the stored copy answers.
//! - **Preview tiles**: each family's name drawn in its own face, as a PNG.
//!   The face comes from Google's CSS2 API with `text=<name>`, which returns
//!   a subset with only those letters (about 9 KB) — or, when the user turned
//!   Google off or it does not answer, from the Fontsource latin subset. The
//!   subset is registered under a private family name, so it can never stand
//!   in for the real font in a title.
//! - **Install on use**: regular and bold (and italic when there is one) as
//!   TTF from jsDelivr, into `text::user_fonts_dir()/<id>/`, with the
//!   licence text beside the files (OFL asks for it when the files travel)
//!   and a `font.json` that records where they came from. Then registered
//!   with the shared text renderer, so the next frame draws with it.
//!
//! The downloads are the `latin` subset, or the family's own default subset
//! when it has no latin one. Text in another script falls back to a system
//! face; Fontsource publishes no unsubsetted files, and parley picks one
//! face per family, so several subsets of one family cannot be combined.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::licence::{self, Policy, Use};
use super::net;
use crate::modules::cloud::provenance::{self, Licence};
use crate::modules::text::{RasterOptions, TextRenderer, TextRequest};

/// How long the font list is trusted before it is asked for again.
pub const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// The addresses the provider talks to. Tests point them at a local server.
#[derive(Debug, Clone)]
pub struct FontSource {
    /// `https://api.fontsource.org/v1`
    pub api: String,
    /// `https://cdn.jsdelivr.net/fontsource/fonts`
    pub cdn: String,
    /// `https://fonts.googleapis.com/css2`
    pub google_css: String,
    /// `https://raw.githubusercontent.com/google/fonts/main`, for the licence
    /// text of a Google family.
    pub google_repo: String,
}

impl Default for FontSource {
    fn default() -> Self {
        Self {
            api: "https://api.fontsource.org/v1".into(),
            cdn: "https://cdn.jsdelivr.net/fontsource/fonts".into(),
            google_css: "https://fonts.googleapis.com/css2".into(),
            google_repo: "https://raw.githubusercontent.com/google/fonts/main".into(),
        }
    }
}

/// One family in the catalogue, as Fontsource lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontEntry {
    pub id: String,
    pub family: String,
    #[serde(default)]
    pub subsets: Vec<String>,
    #[serde(default)]
    pub weights: Vec<u16>,
    #[serde(default)]
    pub styles: Vec<String>,
    #[serde(default, rename = "defSubset")]
    pub def_subset: String,
    #[serde(default)]
    pub variable: bool,
    #[serde(default)]
    pub category: String,
    /// SPDX id.
    #[serde(default)]
    pub license: String,
    /// `google` or `other`.
    #[serde(default, rename = "type")]
    pub source: String,
}

impl FontEntry {
    /// The page a user can read about the family on.
    pub fn page_url(&self) -> String {
        format!("https://fontsource.org/fonts/{}", self.id)
    }

    /// The subset the files are fetched in.
    pub fn subset(&self) -> &str {
        if self.subsets.iter().any(|s| s == "latin") {
            "latin"
        } else if !self.def_subset.is_empty() {
            &self.def_subset
        } else {
            self.subsets.first().map(String::as_str).unwrap_or("latin")
        }
    }

    /// The weight nearest to `want` that the family has.
    pub fn nearest_weight(&self, want: u16) -> u16 {
        self.weights
            .iter()
            .copied()
            .min_by_key(|w| (i32::from(*w) - i32::from(want)).abs())
            .unwrap_or(400)
    }

    /// The faces an install fetches: (weight, style).
    pub fn faces(&self) -> Vec<(u16, &'static str)> {
        let regular = self.nearest_weight(400);
        let has_normal = self.styles.is_empty() || self.styles.iter().any(|s| s == "normal");
        let has_italic = self.styles.iter().any(|s| s == "italic");
        let base = if has_normal { "normal" } else { "italic" };
        let mut faces = vec![(regular, base)];
        let bold = self.nearest_weight(700);
        if bold >= 600 && bold != regular {
            faces.push((bold, base));
        }
        if has_normal && has_italic {
            faces.push((regular, "italic"));
        }
        faces
    }
}

/// The picker's category column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontCategory {
    Popular,
    All,
    SansSerif,
    Serif,
    Display,
    Handwriting,
    Monospace,
}

impl FontCategory {
    pub const ALL: [FontCategory; 7] = [
        FontCategory::Popular,
        FontCategory::All,
        FontCategory::SansSerif,
        FontCategory::Serif,
        FontCategory::Display,
        FontCategory::Handwriting,
        FontCategory::Monospace,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FontCategory::Popular => "Popular",
            FontCategory::All => "All",
            FontCategory::SansSerif => "Sans",
            FontCategory::Serif => "Serif",
            FontCategory::Display => "Display",
            FontCategory::Handwriting => "Handwriting",
            FontCategory::Monospace => "Mono",
        }
    }

    fn admits(self, entry: &FontEntry) -> bool {
        match self {
            FontCategory::Popular => POPULAR.contains(&entry.id.as_str()),
            FontCategory::All => true,
            FontCategory::SansSerif => entry.category == "sans-serif",
            FontCategory::Serif => entry.category == "serif",
            FontCategory::Display => entry.category == "display",
            FontCategory::Handwriting => entry.category == "handwriting",
            FontCategory::Monospace => entry.category == "monospace",
        }
    }
}

/// Families short-form creators reach for first, in the order the picker
/// shows them. Fontsource has no popularity figure; this list is ours.
pub const POPULAR: &[&str] = &[
    "montserrat",
    "poppins",
    "bebas-neue",
    "anton",
    "oswald",
    "inter",
    "roboto",
    "archivo-black",
    "bangers",
    "luckiest-guy",
    "permanent-marker",
    "lobster",
    "pacifico",
    "dancing-script",
    "playfair-display",
    "abril-fatface",
    "dm-serif-display",
    "righteous",
    "fredoka",
    "rubik",
    "nunito",
    "raleway",
    "lato",
    "kanit",
    "barlow",
    "space-grotesk",
    "caveat",
    "shadows-into-light",
    "comfortaa",
    "bungee",
];

/// The catalogue, without icon fonts and without any family whose licence
/// the policy hides. `stale` is true when it came from an old copy because
/// the network did not answer.
pub fn catalogue(source: &FontSource, dir: &Path) -> Result<(Vec<FontEntry>, bool), String> {
    let fetched = net::cached(
        &format!("{}/fonts", source.api),
        &dir.join("fontsource.json"),
        CATALOGUE_TTL,
        "the font list from fontsource.org",
    )?;
    let entries: Vec<FontEntry> = serde_json::from_slice(&fetched.bytes)
        .map_err(|e| format!("the font list did not read ({e})"))?;
    let entries = entries
        .into_iter()
        .filter(|e| e.category != "icons")
        .filter(|e| licence::policy(&licence::spdx(&e.license), Use::Font, false) != Policy::Hide)
        .collect();
    Ok((entries, fetched.stale))
}

/// Families matching `query` (any part of the name, case ignored) in
/// `category`. Popular keeps its own order; the rest are by name.
pub fn search(entries: &[FontEntry], query: &str, category: FontCategory) -> Vec<FontEntry> {
    let query = query.trim().to_lowercase();
    let mut out: Vec<FontEntry> = entries
        .iter()
        .filter(|e| category.admits(e))
        .filter(|e| query.is_empty() || e.family.to_lowercase().contains(&query))
        .cloned()
        .collect();
    if category == FontCategory::Popular {
        out.sort_by_key(|e| POPULAR.iter().position(|p| *p == e.id));
    } else {
        out.sort_by_key(|e| e.family.to_lowercase());
    }
    out
}

// --- installed families --------------------------------------------------------

/// What `font.json` records about one installed family.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledFont {
    pub id: String,
    pub family: String,
    pub licence: Licence,
    /// File names in the family's directory.
    pub files: Vec<String>,
    pub source_url: String,
    pub installed_at: String,
}

/// The manifest's name in each family's directory.
pub const MANIFEST: &str = "font.json";

/// Every family installed under `dir`, by family name.
pub fn installed(dir: &Path) -> Vec<InstalledFont> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<InstalledFont> = entries
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join(MANIFEST)).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    out.sort_by_key(|f| f.family.to_lowercase());
    out
}

/// The installed family with this id, if its files are all there.
pub fn installed_one(dir: &Path, id: &str) -> Option<InstalledFont> {
    let family_dir = dir.join(provenance::slug(id));
    let font: InstalledFont =
        serde_json::from_slice(&std::fs::read(family_dir.join(MANIFEST)).ok()?).ok()?;
    font.files
        .iter()
        .all(|f| family_dir.join(f).exists())
        .then_some(font)
}

/// The directory an install of `id` uses.
pub fn family_dir(dir: &Path, id: &str) -> PathBuf {
    dir.join(provenance::slug(id))
}

/// The google/fonts directory of a family: `Open Sans` → `opensans`.
fn google_dir_name(family: &str) -> String {
    family
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The licence text to keep beside the files: the family's own (it carries
/// the copyright line) when google/fonts has it, else a pointer to the
/// licence.
fn licence_text(source: &FontSource, entry: &FontEntry, licence: &Licence) -> String {
    if entry.source == "google" {
        let (folder, file) = match licence.id.as_str() {
            "OFL-1.1" => ("ofl", "OFL.txt"),
            "Apache-2.0" => ("apache", "LICENSE.txt"),
            "UFL-1.0" => ("ufl", "UFL.txt"),
            _ => ("", ""),
        };
        if !folder.is_empty() {
            let url = format!(
                "{}/{folder}/{}/{file}",
                source.google_repo,
                google_dir_name(&entry.family)
            );
            if let Ok(bytes) = net::get(&url) {
                if let Ok(text) = String::from_utf8(bytes) {
                    if text.len() > 200 {
                        return text;
                    }
                }
            }
        }
    }
    format!(
        "{family} is licensed under the {name} ({id}).\nThe full text: {url}\nSource: {page}\n",
        family = entry.family,
        name = licence.name,
        id = licence.id,
        url = licence.url,
        page = entry.page_url(),
    )
}

/// Download a family into `dir/<id>/` and record it. Returns the record and
/// the paths of the font files, for the caller to register. A family that
/// is already complete is not fetched again.
pub fn install(
    source: &FontSource,
    entry: &FontEntry,
    dir: &Path,
) -> Result<(InstalledFont, Vec<PathBuf>), String> {
    let family_dir = family_dir(dir, &entry.id);
    if let Some(done) = installed_one(dir, &entry.id) {
        let paths = done.files.iter().map(|f| family_dir.join(f)).collect();
        return Ok((done, paths));
    }
    let licence = licence::spdx(&entry.license);
    if licence::policy(&licence, Use::Font, false) == Policy::Hide {
        return Err(format!(
            "{} has a licence chukcut cannot check ({})",
            entry.family, entry.license
        ));
    }
    let subset = entry.subset().to_string();
    let mut files = Vec::new();
    let mut paths = Vec::new();
    for (weight, style) in entry.faces() {
        let name = format!("{}-{subset}-{weight}-{style}.ttf", entry.id);
        let url = format!(
            "{}/{}@latest/{subset}-{weight}-{style}.ttf",
            source.cdn, entry.id
        );
        let path = family_dir.join(&name);
        match net::once(&url, &path, &format!("the font {}", entry.family)) {
            Ok(()) => {
                files.push(name);
                paths.push(path);
            }
            // The regular face is the family; without it there is nothing.
            Err(error) if files.is_empty() => return Err(error),
            Err(error) => tracing::warn!(%error, "a font face was not downloaded"),
        }
    }
    std::fs::write(
        family_dir.join("LICENSE.txt"),
        licence_text(source, entry, &licence),
    )
    .map_err(|e| format!("cannot write the font licence: {e}"))?;
    let record = InstalledFont {
        id: entry.id.clone(),
        family: entry.family.clone(),
        licence,
        files,
        source_url: entry.page_url(),
        installed_at: provenance::now_rfc3339(),
    };
    let text = serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?;
    std::fs::write(family_dir.join(MANIFEST), text)
        .map_err(|e| format!("cannot write the font record: {e}"))?;
    Ok((record, paths))
}

/// Register an installed family's files with `renderer`. Returns the family
/// names the renderer now knows them by.
pub fn register(renderer: &TextRenderer, paths: &[PathBuf]) -> Vec<String> {
    let mut names = Vec::new();
    for path in paths {
        match std::fs::read(path) {
            Ok(bytes) => names.extend(renderer.register_font(bytes)),
            Err(error) => tracing::warn!(%error, path = %path.display(), "font not loaded"),
        }
    }
    names.sort_unstable();
    names.dedup();
    names
}

// --- preview tiles -------------------------------------------------------------

/// Where a preview's face comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewHost {
    /// Google's CSS2 API with `text=`: a 9 KB subset per family. Sends the
    /// user's IP address to Google.
    #[default]
    Google,
    /// The Fontsource latin subset from jsDelivr: about ten times larger.
    Fontsource,
}

/// The renderer preview subsets are registered with: its own, so a subset
/// never reaches the shared one titles are drawn with.
fn preview_renderer() -> &'static TextRenderer {
    static RENDERER: OnceLock<TextRenderer> = OnceLock::new();
    RENDERER.get_or_init(|| TextRenderer::with_budget(8 * 1024 * 1024))
}

fn registered_previews() -> &'static Mutex<HashSet<String>> {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Height of a preview line, in pixels, at which tiles are drawn: twice the
/// size they are shown at.
pub const PREVIEW_PX: f32 = 40.0;

/// `text` drawn in `family` with `renderer`, white on transparent, as a PNG
/// at `path`.
fn draw_name(renderer: &TextRenderer, family: &str, text: &str, path: &Path) -> Result<(), String> {
    let request = TextRequest {
        content: text.to_string(),
        font_family: family.to_string(),
        font_size: PREVIEW_PX,
        ..TextRequest::default()
    };
    let raster = renderer.rasterize_uncached(&request, &RasterOptions::tight());
    if raster.width == 0 || raster.height == 0 {
        return Err(format!("{family} drew nothing"));
    }
    let image = image::RgbaImage::from_raw(raster.width, raster.height, raster.pixels)
        .ok_or("the preview did not fit its own size")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let part = path.with_extension("part.png");
    image
        .save(&part)
        .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    std::fs::rename(&part, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The `src: url(…)` of the first face in a CSS2 answer.
fn css_font_url(css: &str) -> Option<String> {
    let start = css.find("url(")? + 4;
    let end = css[start..].find(')')? + start;
    Some(css[start..end].trim_matches(['\'', '"']).to_string())
}

/// The face file a preview is drawn with, fetched once into `dir`.
fn preview_face(
    source: &FontSource,
    entry: &FontEntry,
    dir: &Path,
    host: PreviewHost,
) -> Result<PathBuf, String> {
    let weight = entry.nearest_weight(400);
    let style = if entry.styles.is_empty() || entry.styles.iter().any(|s| s == "normal") {
        "normal"
    } else {
        "italic"
    };
    let fontsource = || -> Result<PathBuf, String> {
        let path = dir.join(format!("{}-{weight}-{style}.ttf", entry.id));
        let url = format!(
            "{}/{}@latest/{}-{weight}-{style}.ttf",
            source.cdn,
            entry.id,
            entry.subset()
        );
        net::once(&url, &path, &format!("a preview of {}", entry.family))?;
        Ok(path)
    };
    if host == PreviewHost::Fontsource || entry.source != "google" {
        return fontsource();
    }
    let path = dir.join(format!("{}-text.ttf", entry.id));
    if path.exists() {
        return Ok(path);
    }
    let italic = if style == "italic" { "1," } else { "" };
    let axis = if italic.is_empty() {
        "wght"
    } else {
        "ital,wght"
    };
    let css_url = format!(
        "{}?family={}:{axis}@{italic}{weight}&text={}",
        source.google_css,
        crate::modules::cloud::http::encode(&entry.family),
        crate::modules::cloud::http::encode(&entry.family),
    );
    let google = net::get(&css_url)
        .ok()
        .and_then(|css| css_font_url(&String::from_utf8_lossy(&css)))
        .and_then(|url| net::download(&url, &path).ok());
    match google {
        Some(_) => Ok(path),
        None => fontsource(),
    }
}

/// The preview tile of a catalogue family: its name in its own face.
pub fn preview(
    source: &FontSource,
    entry: &FontEntry,
    dir: &Path,
    host: PreviewHost,
) -> Result<PathBuf, String> {
    let tile = dir.join(format!("{}.png", provenance::slug(&entry.id)));
    if tile.exists() {
        return Ok(tile);
    }
    let face = preview_face(source, entry, &dir.join("faces"), host)?;
    let private = format!("chukcut preview {}", entry.id);
    let renderer = preview_renderer();
    if registered_previews().lock().insert(private.clone()) {
        let bytes = std::fs::read(&face).map_err(|e| e.to_string())?;
        if renderer.register_font_as(bytes, Some(&private)).is_empty() {
            let _ = std::fs::remove_file(&face);
            registered_previews().lock().remove(&private);
            return Err(format!("the preview of {} is not a font", entry.family));
        }
    }
    draw_name(renderer, &private, &entry.family, &tile)
        .map(|()| tile)
        .map_err(|e| format!("no preview for {}: {e}", entry.family))
}

/// The preview tile of a family the system (or an install) provides, drawn
/// with the renderer titles use.
pub fn system_preview(family: &str, dir: &Path) -> Result<PathBuf, String> {
    let tile = dir.join(format!("system-{}.png", provenance::slug(family)));
    if tile.exists() {
        return Ok(tile);
    }
    draw_name(TextRenderer::shared(), family, family, &tile).map(|()| tile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::cloud::http::test_server;

    fn entry(id: &str, family: &str, weights: &[u16], styles: &[&str]) -> FontEntry {
        FontEntry {
            id: id.into(),
            family: family.into(),
            subsets: vec!["latin".into(), "latin-ext".into()],
            weights: weights.to_vec(),
            styles: styles.iter().map(|s| s.to_string()).collect(),
            def_subset: "latin".into(),
            variable: false,
            category: "display".into(),
            license: "OFL-1.1".into(),
            source: "other".into(),
        }
    }

    /// A real font from the system, to stand in for a download.
    fn some_font() -> Vec<u8> {
        for path in [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                return bytes;
            }
        }
        panic!("no test font on this machine");
    }

    #[test]
    fn an_install_fetches_regular_bold_and_italic() {
        let lobster = entry("lobster", "Lobster", &[400], &["normal"]);
        assert_eq!(lobster.faces(), vec![(400, "normal")]);
        let inter = entry(
            "inter",
            "Inter",
            &[100, 300, 400, 500, 700, 900],
            &["normal", "italic"],
        );
        assert_eq!(
            inter.faces(),
            vec![(400, "normal"), (700, "normal"), (400, "italic")]
        );
        let thin = entry("x", "X", &[200, 300], &["italic"]);
        assert_eq!(thin.faces(), vec![(300, "italic")]);
    }

    #[test]
    fn search_finds_by_name_and_keeps_popular_order() {
        let list = vec![
            entry("lobster", "Lobster", &[400], &["normal"]),
            entry("montserrat", "Montserrat", &[400], &["normal"]),
            entry("zilla", "Zilla Slab", &[400], &["normal"]),
        ];
        let popular = search(&list, "", FontCategory::Popular);
        assert_eq!(popular[0].id, "montserrat");
        assert_eq!(popular.len(), 2);
        assert_eq!(search(&list, "SLAB", FontCategory::All)[0].id, "zilla");
        assert!(search(&list, "slab", FontCategory::Serif).is_empty());
    }

    #[test]
    fn the_catalogue_drops_icon_fonts_and_unknown_licences() {
        let dir = super::super::scratch("fonts-catalogue");
        let body = br#"[
            {"id":"a","family":"A","category":"serif","license":"OFL-1.1","type":"google"},
            {"id":"b","family":"B Icons","category":"icons","license":"Apache-2.0","type":"google"},
            {"id":"c","family":"C","category":"display","license":"Proprietary","type":"other"}
        ]"#;
        let server = test_server::serve(vec![(200, "application/json", body.to_vec())]);
        let source = FontSource {
            api: server.url.clone(),
            ..FontSource::default()
        };
        let (list, stale) = catalogue(&source, &dir).unwrap();
        assert!(!stale);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].family, "A");
        assert_eq!(
            server.requests.lock().unwrap()[0].request_line,
            "GET /fonts HTTP/1.1"
        );
    }

    #[test]
    fn an_installed_font_is_recorded_and_drawn() {
        let dir = super::super::scratch("fonts-install");
        let font = some_font();
        let server = test_server::serve(vec![(200, "font/ttf", font)]);
        let source = FontSource {
            cdn: server.url.clone(),
            ..FontSource::default()
        };
        let lobster = entry("lobster", "Lobster", &[400], &["normal"]);
        let (record, paths) = install(&source, &lobster, &dir).unwrap();
        assert_eq!(record.licence.id, "OFL-1.1");
        assert_eq!(record.files, ["lobster-latin-400-normal.ttf"]);
        assert!(dir.join("lobster/LICENSE.txt").exists());
        assert_eq!(
            server.requests.lock().unwrap()[0].request_line,
            "GET /lobster@latest/latin-400-normal.ttf HTTP/1.1"
        );
        // Installed: the second call does not ask the network.
        let (again, _) = install(&source, &lobster, &dir).unwrap();
        assert_eq!(again, record);
        assert_eq!(installed(&dir), vec![record]);

        let renderer = TextRenderer::new();
        let names = register(&renderer, &paths);
        assert!(!names.is_empty(), "the file registered no family");
        assert!(renderer.font_families().contains(&names[0]));
    }

    #[test]
    fn a_preview_tile_is_the_name_in_its_own_face() {
        let dir = super::super::scratch("fonts-preview");
        let css = b"@font-face { src: url(FACE) format('truetype'); }".to_vec();
        let font = some_font();
        let server = test_server::serve_with(|url| {
            let css = String::from_utf8(css)
                .unwrap()
                .replace("FACE", &format!("{url}/face.ttf"));
            vec![(200, "text/css", css.into_bytes()), (200, "font/ttf", font)]
        });
        let source = FontSource {
            google_css: format!("{}/css2", server.url),
            ..FontSource::default()
        };
        let mut lobster = entry("lobster", "Lobster", &[400], &["normal"]);
        lobster.source = "google".into();
        let tile = preview(&source, &lobster, &dir, PreviewHost::Google).unwrap();
        let picture = image::open(&tile).unwrap();
        assert!(picture.width() > 40 && picture.height() > 10);
        let requests = server.requests.lock().unwrap();
        assert!(
            requests[0]
                .request_line
                .contains("family=Lobster:wght@400&text=Lobster"),
            "{}",
            requests[0].request_line
        );
    }

    #[test]
    fn css_urls_are_read() {
        let css = "@font-face {\n  src: url(https://fonts.gstatic.com/l/font?kit=x&v=1) format('truetype');\n}";
        assert_eq!(
            css_font_url(css).unwrap(),
            "https://fonts.gstatic.com/l/font?kit=x&v=1"
        );
    }
}
