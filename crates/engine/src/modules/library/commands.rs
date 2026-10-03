//! Commands for the asset library.
//!
//! Everything that touches the network blocks; the shells call these off the
//! UI thread. None of them edits the document except [`library_sticker_add`],
//! which goes through `project_import_media` (which copies the licence record
//! into the project) and one `EditCommand`, like any placement.
//!
//! The catalogues are held in memory after the first read, so typing in a
//! search field filters a list rather than reading a file per keystroke.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::fonts::{self, FontCategory, FontEntry, FontSource, InstalledFont, PreviewHost};
use super::looks::{self, LookEntry};
use super::sounds::{self, MusicSource, SfxPack, Sound, Track};
use super::stickers::{self, IconHit, IconSet, Sticker, StickerIndex, StickerSource, StickerStyle};
use crate::modules::text::{user_fonts_dir, TextRenderer};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::workspace::paths;
use crate::state::AppState;

// --- settings ------------------------------------------------------------------

/// The library's own settings, in `library.json` beside the others.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LibrarySettings {
    /// Where font previews come from. Google's subsets are ten times smaller
    /// but tell Google the user's IP address; Fontsource is jsDelivr.
    #[serde(default)]
    pub font_previews: PreviewHost,
}

fn settings_path() -> PathBuf {
    paths::config_root().join("library.json")
}

pub fn library_settings() -> LibrarySettings {
    std::fs::read(settings_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn library_set_settings(settings: &LibrarySettings) -> Result<(), String> {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("cannot save the library settings: {e}"))
}

// --- fonts ---------------------------------------------------------------------

/// The font catalogue and whether it is an old copy.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FontList {
    pub fonts: Vec<FontEntry>,
    pub stale: bool,
}

fn font_list() -> &'static Mutex<Option<Arc<FontList>>> {
    static LIST: OnceLock<Mutex<Option<Arc<FontList>>>> = OnceLock::new();
    LIST.get_or_init(|| Mutex::new(None))
}

/// The Fontsource catalogue, read once per process (and from disk for a
/// week).
pub fn library_font_catalogue() -> Result<Arc<FontList>, String> {
    let cached = font_list().lock().clone();
    if let Some(list) = cached {
        return Ok(list);
    }
    let (fonts, stale) = fonts::catalogue(&FontSource::default(), &super::catalogue_dir())?;
    let list = Arc::new(FontList { fonts, stale });
    // A stale list is kept for this run only if nothing better comes; the
    // next call after a failure tries the network again.
    if !stale {
        *font_list().lock() = Some(Arc::clone(&list));
    }
    Ok(list)
}

/// Catalogue families matching `query` in `category`.
pub fn library_font_search(query: &str, category: FontCategory) -> Result<Vec<FontEntry>, String> {
    let list = library_font_catalogue()?;
    Ok(fonts::search(&list.fonts, query, category))
}

fn font_previews_dir() -> PathBuf {
    super::thumbs_dir().join("fonts")
}

/// A catalogue family's preview tile: its name in its own face.
pub fn library_font_preview(entry: &FontEntry) -> Result<PathBuf, String> {
    fonts::preview(
        &FontSource::default(),
        entry,
        &font_previews_dir(),
        library_settings().font_previews,
    )
}

/// The preview tile of a family the text renderer already has.
pub fn library_font_system_preview(family: &str) -> Result<PathBuf, String> {
    fonts::system_preview(family, &font_previews_dir())
}

/// What an install did.
#[derive(Debug, Clone, Serialize)]
pub struct FontInstalled {
    pub font: InstalledFont,
    /// The family names titles now use for it.
    pub families: Vec<String>,
}

/// Download a family and register it with the text renderer, so the next
/// frame of a title draws with it.
pub fn library_font_install(entry: &FontEntry) -> Result<FontInstalled, String> {
    let (font, files) = fonts::install(&FontSource::default(), entry, &user_fonts_dir())?;
    let families = fonts::register(TextRenderer::shared(), &files);
    if families.is_empty() {
        return Err(format!(
            "{} downloaded but is not a usable font",
            entry.family
        ));
    }
    Ok(FontInstalled { font, families })
}

/// The families the library has installed.
pub fn library_fonts_installed() -> Vec<InstalledFont> {
    fonts::installed(&user_fonts_dir())
}

/// Every family the text renderer can draw now: the system's and the
/// installed ones.
pub fn library_system_fonts() -> Vec<String> {
    TextRenderer::shared().font_families()
}

/// Set a title's typeface, as one undo step. The family must be one the
/// renderer knows ([`library_system_fonts`]); install a catalogue family
/// first with [`library_font_install`].
pub fn library_font_use_for_title(
    state: &Arc<AppState>,
    material_id: &str,
    family: &str,
) -> Result<EditResponse, String> {
    let before = state
        .with_project(|p| p.materials.text(material_id).cloned())?
        .ok_or_else(|| "the title is no longer in the project".to_string())?;
    if before.font_family == family {
        return Err(format!("the title is already in {family}"));
    }
    let mut after = before.clone();
    after.font_family = family.to_string();
    crate::modules::timeline::commands::timeline_apply(
        state,
        crate::modules::timeline::ops::EditCommand::SetTextMaterial { before, after },
    )
}

// --- stickers ------------------------------------------------------------------

fn sticker_index_cell() -> &'static Mutex<Option<Arc<StickerIndex>>> {
    static INDEX: OnceLock<Mutex<Option<Arc<StickerIndex>>>> = OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(None))
}

/// The emoji index (see `stickers::index`).
pub fn library_sticker_index() -> Result<Arc<StickerIndex>, String> {
    let cached = sticker_index_cell().lock().clone();
    if let Some(index) = cached {
        return Ok(index);
    }
    let index = Arc::new(stickers::index(
        &StickerSource::default(),
        &super::catalogue_dir(),
    )?);
    if !index.stale {
        *sticker_index_cell().lock() = Some(Arc::clone(&index));
    }
    Ok(index)
}

pub fn library_sticker_thumb(sticker: &Sticker, style: StickerStyle) -> Result<PathBuf, String> {
    stickers::thumb(
        &StickerSource::default(),
        sticker,
        style,
        &super::thumbs_dir().join("stickers"),
    )
}

/// The sticker's file with its licence record, ready to import.
pub fn library_sticker_fetch(sticker: &Sticker, style: StickerStyle) -> Result<PathBuf, String> {
    stickers::fetch(
        &StickerSource::default(),
        sticker,
        style,
        &super::cache_dir(),
    )
}

fn icon_sets_cell() -> &'static Mutex<Option<Arc<Vec<IconSet>>>> {
    static SETS: OnceLock<Mutex<Option<Arc<Vec<IconSet>>>>> = OnceLock::new();
    SETS.get_or_init(|| Mutex::new(None))
}

/// Icons matching `query` from the sets the licence policy allows.
pub fn library_icon_search(query: &str) -> Result<Vec<IconHit>, String> {
    // Read the cell into a local first: a guard in the match scrutinee
    // would live through the arms and deadlock the store below.
    let cached = icon_sets_cell().lock().clone();
    let sets = match cached {
        Some(sets) => sets,
        None => {
            let sets = Arc::new(stickers::icon_sets(
                &StickerSource::default(),
                &super::catalogue_dir(),
            )?);
            *icon_sets_cell().lock() = Some(Arc::clone(&sets));
            sets
        }
    };
    stickers::icon_search(&StickerSource::default(), &sets, query, 96)
}

pub fn library_icon_thumb(icon: &IconHit) -> Result<PathBuf, String> {
    stickers::icon_thumb(
        &StickerSource::default(),
        icon,
        &super::thumbs_dir().join("icons"),
    )
}

pub fn library_icon_fetch(icon: &IconHit) -> Result<PathBuf, String> {
    stickers::icon_fetch(&StickerSource::default(), icon, &super::cache_dir())
}

/// Import the sticker file at `path` (its licence record comes with it) and
/// put it on an overlay lane at `at`, centred, as one undo step.
pub async fn library_sticker_add(
    state: &Arc<AppState>,
    path: &Path,
    at: crate::modules::project::Micros,
) -> Result<EditResponse, String> {
    let imported = crate::modules::project::commands::project_import_media(
        state,
        path.to_string_lossy().to_string(),
    )
    .await?;
    let command = {
        let guard = state.project.read();
        let project = guard.as_ref().ok_or("no project is open")?;
        stickers::place(project, &imported.id, at)?
    };
    crate::modules::timeline::commands::timeline_apply(state, command)
}

// --- music and sound effects ---------------------------------------------------

pub fn library_music_curated(mood: Option<&str>) -> Vec<Track> {
    sounds::curated(mood)
}

/// The music catalogue and whether it is an old copy.
type MusicList = Arc<(Vec<Track>, bool)>;

fn music_cell() -> &'static Mutex<Option<MusicList>> {
    static LIST: OnceLock<Mutex<Option<MusicList>>> = OnceLock::new();
    LIST.get_or_init(|| Mutex::new(None))
}

/// The whole Incompetech catalogue matching `query`, and whether the list is
/// an old copy.
pub fn library_music_search(query: &str) -> Result<(Vec<Track>, bool), String> {
    let cached = music_cell().lock().clone();
    let list = match cached {
        Some(list) => list,
        None => {
            let list = Arc::new(sounds::catalogue(
                &MusicSource::default(),
                &super::catalogue_dir(),
            )?);
            if !list.1 {
                *music_cell().lock() = Some(Arc::clone(&list));
            }
            list
        }
    };
    Ok((sounds::search(&list.0, query), list.1))
}

/// A track's file with its licence record (downloaded once): what plays in
/// the audition and what goes on the timeline.
pub fn library_music_fetch(track: &Track) -> Result<PathBuf, String> {
    sounds::fetch_track(&MusicSource::default(), track, &super::cache_dir())
}

/// Whether a track is on disk already, for the panel's "downloaded" mark.
pub fn library_music_cached(track: &Track) -> bool {
    track.path(&super::cache_dir()).exists()
}

pub fn library_sfx_packs() -> &'static [SfxPack] {
    sounds::SFX_PACKS
}

/// Whether a pack is unpacked already (opening it needs no network).
pub fn library_sfx_ready(pack: &SfxPack) -> bool {
    pack.is_ready(&super::cache_dir())
}

/// A pack's sounds, downloading it the first time.
pub fn library_sfx_open(pack_id: &str) -> Result<Vec<Sound>, String> {
    let pack = SfxPack::by_id(pack_id).ok_or_else(|| format!("there is no pack {pack_id}"))?;
    sounds::open_pack(pack, &super::cache_dir())
}

// --- looks ---------------------------------------------------------------------

/// Write our looks into the LUT library, once per looks version. Cheap when
/// they are there: one small file read.
pub fn library_looks_install() -> Result<usize, String> {
    looks::install(&paths::luts_dir())
}

/// Our looks as installed, in the Filters tab's order.
pub fn library_looks() -> Vec<LookEntry> {
    looks::installed(&paths::luts_dir())
}

/// A look's tile, drawn by the compositor and kept.
pub fn library_look_tile(lut_path: &str, size: (u32, u32)) -> Result<PathBuf, String> {
    crate::modules::fx::tiles::look_tile(lut_path, size)
}
