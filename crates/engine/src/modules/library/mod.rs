//! The built-in asset library: fonts, stickers, music, sound effects and
//! looks, from sources whose licences let a creator use them in a monetised
//! video (`docs/research/open-assets.md`).
//!
//! Nothing here is committed media. The repository holds the *addresses* and
//! the licence facts; the files are fetched on first use into the cache (or,
//! for fonts, the data directory) with a licence record beside each one, and
//! from then on everything works without a network. The looks are the one
//! exception, and only because they are ours: they are generated here, from
//! code, and written into the LUT library.
//!
//! - [`licence`]: SPDX ids to [`Licence`] values, and the policy that decides
//!   whether an item is shown, shown with a warning, or hidden.
//! - [`net`]: one cached fetch for every catalogue — fresh copy when the
//!   network answers, the stale copy when it does not, a sentence the user can
//!   act on when there is neither.
//! - [`fonts`]: the Fontsource catalogue, preview tiles, download on use and
//!   registration with the text renderer.
//! - [`stickers`]: Fluent Emoji (MIT) and Noto Emoji (Apache-2.0) by Unicode
//!   group, and Iconify icon sets filtered to permissive licences.
//! - [`sounds`]: a curated Incompetech list (CC BY 4.0), the whole Incompetech
//!   catalogue, and CC0 sound-effect packs from Kenney and OpenGameArt.
//! - [`looks`]: our own colour looks as `.cube` files and their tiles.
//! - [`commands`]: the shell-facing API.
//!
//! Every file that can reach the timeline goes through
//! `cloud::provenance`, so the export's licence summary and credits file see
//! it exactly as they see a stock download.
//!
//! [`Licence`]: crate::modules::cloud::provenance::Licence

pub mod animated_emoji;
pub mod commands;
pub mod fonts;
pub mod licence;
pub mod looks;
pub mod net;
pub mod sounds;
pub mod stickers;

use std::path::PathBuf;

use crate::modules::workspace::paths;

/// Where library downloads live: beside the stock downloads, because they
/// can be fetched again (`cloud::provenance::stock_dir`).
pub fn cache_dir() -> PathBuf {
    paths::cache_root().join("library")
}

/// Catalogues: the font list, the emoji index, the music list.
pub fn catalogue_dir() -> PathBuf {
    cache_dir().join("catalogues")
}

/// Small pictures for the panel: not licensed copies, so no sidecar.
pub fn thumbs_dir() -> PathBuf {
    cache_dir().join("thumbs")
}

#[cfg(test)]
pub(crate) fn scratch(name: &str) -> PathBuf {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-scratch/library")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
