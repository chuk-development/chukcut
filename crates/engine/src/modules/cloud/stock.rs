//! Stock search: Pexels and Pixabay (photo, video), Freesound (sound).
//!
//! A search returns [`StockHit`]s that already carry their licence and
//! credit, so the panel can badge a hit before anyone downloads it.
//! Downloading puts the file in `cache_root()/library/<provider>/<id>/` with
//! an `asset.json` beside it; importing it then copies that record into the
//! project. Search answers are cached for a day, which Pixabay requires and
//! which spares everyone's quota (`docs/research/open-assets.md`).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::http;
use super::provenance::{self, Commercial, Licence, Origin, OriginKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StockKind {
    Video,
    Photo,
    Sound,
}

impl StockKind {
    fn extension(self) -> &'static str {
        match self {
            StockKind::Video => "mp4",
            StockKind::Photo => "jpg",
            StockKind::Sound => "mp3",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StockQuery {
    pub text: String,
    pub kind: StockKind,
    /// From 1.
    pub page: u32,
    pub per_page: u32,
    /// Show non-commercial sounds. Off by default: a creator monetising a
    /// video must not stumble into one.
    #[serde(default)]
    pub include_non_commercial: bool,
}

impl StockQuery {
    pub fn new(text: impl Into<String>, kind: StockKind) -> Self {
        Self {
            text: text.into(),
            kind,
            page: 1,
            per_page: 24,
            include_non_commercial: false,
        }
    }
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StockHit {
    pub provider: String,
    pub id: String,
    pub kind: StockKind,
    pub title: String,
    pub creator: String,
    #[serde(default)]
    pub creator_url: String,
    /// The item's page at the provider.
    pub source_url: String,
    /// A small picture for the grid: a frame, a photo, a waveform.
    pub thumb_url: Option<String>,
    /// A small file to play on hover or audition: a low-resolution video,
    /// a sound preview.
    pub preview_url: Option<String>,
    /// What "Download" fetches.
    pub download_url: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub duration_seconds: f64,
    pub licence: Licence,
    pub credit: String,
}

impl StockHit {
    /// The provenance record its download gets.
    pub fn origin(&self) -> Origin {
        Origin {
            title: self.title.clone(),
            source_id: self.id.clone(),
            creator: self.creator.clone(),
            creator_url: self.creator_url.clone(),
            source_url: self.source_url.clone(),
            licence: self.licence.clone(),
            credit: self.credit.clone(),
            ..Origin::new(OriginKind::Stock, &self.provider)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StockPage {
    pub hits: Vec<StockHit>,
    pub total: u64,
    pub has_more: bool,
    /// What the panel shows under the results: "Photos and videos provided
    /// by Pexels". Pexels and Pixabay require it.
    pub attribution: String,
    /// The provider's page, for that line's link.
    pub attribution_url: String,
}

/// A provider that can search its library.
pub trait StockSearch: Send + Sync {
    fn kinds(&self) -> &'static [StockKind];
    fn search(&self, query: &StockQuery) -> Result<StockPage, String>;
}

/// How long a search answer stays good.
pub const SEARCH_CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

/// `search` through a one-day cache under `dir`, keyed by provider and query
/// (never by key). `dir: None` searches straight through.
pub fn cached_search(
    provider: &str,
    searcher: &dyn StockSearch,
    query: &StockQuery,
    dir: Option<&Path>,
) -> Result<StockPage, String> {
    let Some(dir) = dir else {
        return searcher.search(query);
    };
    let key = {
        let mut hash = Sha256::new();
        hash.update(provider.as_bytes());
        hash.update(serde_json::to_vec(query).unwrap_or_default());
        hash.finalize()
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let path = dir.join(format!("{key}.json"));
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < SEARCH_CACHE_TTL);
    if fresh {
        if let Some(page) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<StockPage>(&bytes).ok())
        {
            return Ok(page);
        }
    }
    let page = searcher.search(query)?;
    if std::fs::create_dir_all(dir).is_ok() {
        if let Ok(bytes) = serde_json::to_vec(&page) {
            let _ = std::fs::write(&path, bytes);
        }
    }
    Ok(page)
}

/// The file a hit downloads to, under `root` (normally the library cache).
pub fn download_path(root: &Path, hit: &StockHit) -> PathBuf {
    let dir = root
        .join(&hit.provider)
        .join(provenance::slug(&format!("{:?}-{}", hit.kind, hit.id)));
    let extension = Path::new(hit.download_url.split('?').next().unwrap_or(""))
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| hit.kind.extension().to_string());
    let stem = if hit.title.trim().is_empty() {
        format!("{}-{}", hit.provider, hit.id)
    } else {
        provenance::slug(&hit.title)
    };
    dir.join(format!("{stem}.{extension}"))
}

/// Download `hit` under `root` with its sidecar, or hand back the copy that
/// is already there.
pub fn download(
    root: &Path,
    hit: &StockHit,
    progress: &dyn Fn(u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let path = download_path(root, hit);
    if path.exists() && provenance::read_sidecar(&path).is_some() {
        return Ok(path);
    }
    if hit.download_url.is_empty() {
        return Err("this item has no file to download".to_string());
    }
    http::download(&hit.download_url, &path, progress, cancel)?;
    provenance::write_sidecar(&path, &hit.origin())?;
    Ok(path)
}

/// A thumbnail or preview fetched once into `dir`, named by the URL's hash,
/// for the panel to draw or play. Not a licensed copy: no sidecar.
pub fn cached_preview(dir: &Path, url: &str) -> Result<PathBuf, String> {
    let hash: String = Sha256::digest(url.as_bytes())
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect();
    let extension = Path::new(url.split('?').next().unwrap_or(""))
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let path = dir.join(format!("{hash}.{extension}"));
    if path.exists() {
        return Ok(path);
    }
    http::download(url, &path, &|_| {}, &AtomicBool::new(false))?;
    Ok(path)
}

/// Keep commercial hits only, unless the user asked for all of them.
pub(crate) fn filter_licences(hits: Vec<StockHit>, include_non_commercial: bool) -> Vec<StockHit> {
    if include_non_commercial {
        return hits;
    }
    hits.into_iter()
        .filter(|h| h.licence.commercial != Commercial::No)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Counting(AtomicUsize);

    impl StockSearch for Counting {
        fn kinds(&self) -> &'static [StockKind] {
            &[StockKind::Photo]
        }
        fn search(&self, _: &StockQuery) -> Result<StockPage, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(StockPage {
                total: 7,
                ..StockPage::default()
            })
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/cloud")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_search_is_answered_from_the_cache_for_a_day() {
        let dir = scratch("stock-cache");
        let searcher = Counting(AtomicUsize::new(0));
        let query = StockQuery::new("sea", StockKind::Photo);
        for _ in 0..3 {
            let page = cached_search("pexels", &searcher, &query, Some(&dir)).unwrap();
            assert_eq!(page.total, 7);
        }
        assert_eq!(searcher.0.load(Ordering::SeqCst), 1);
        let other = StockQuery::new("lake", StockKind::Photo);
        cached_search("pexels", &searcher, &other, Some(&dir)).unwrap();
        assert_eq!(searcher.0.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_download_lands_with_its_licence_record() {
        let server =
            super::super::http::test_server::serve(vec![(200, "video/mp4", b"mp4-bytes".to_vec())]);
        let root = scratch("stock-download");
        let hit = StockHit {
            provider: "pexels".into(),
            id: "123".into(),
            kind: StockKind::Video,
            title: "Ocean waves".into(),
            creator: "Jane".into(),
            creator_url: String::new(),
            source_url: "https://www.pexels.com/video/123/".into(),
            thumb_url: None,
            preview_url: None,
            download_url: format!("{}/files/123.mp4?token=x", server.url),
            width: 1920,
            height: 1080,
            duration_seconds: 12.0,
            licence: Licence::free("LicenseRef-Pexels", "Pexels License", ""),
            credit: "Video by Jane on Pexels".into(),
        };
        let cancel = AtomicBool::new(false);
        let path = download(&root, &hit, &|_| {}, &cancel).unwrap();
        assert!(
            path.ends_with("pexels/video-123/ocean-waves.mp4"),
            "{}",
            path.display()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"mp4-bytes");
        let origin = provenance::read_sidecar(&path).unwrap();
        assert_eq!(origin.kind, OriginKind::Stock);
        assert_eq!(origin.credit, "Video by Jane on Pexels");
        // A second download is the cached file; the server has no second
        // answer to give.
        assert_eq!(download(&root, &hit, &|_| {}, &cancel).unwrap(), path);
    }
}
