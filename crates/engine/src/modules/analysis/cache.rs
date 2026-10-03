//! Raw analysis results on disk, so asking again is instant.
//!
//! The cache is for speed, never a store of record
//! (`docs/research/ml-features.md` §2.5, "Caching"): what an edit depends on
//! is in the document, and deleting `cache_root()/analysis/` loses nothing
//! but time. Kept here: per-frame scene scores (so a new sensitivity does not
//! decode the file again) and per-frame camera motion (so an undone and
//! redone stabilisation, or a split clip analysed again, does not either).
//!
//! The key is a SHA-256 over the file's identity (path, size, modification
//! time), the analysed range, and an algorithm tag that is bumped whenever
//! the analysis changes what it would write.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::modules::project::document::TimeRange;
use crate::modules::workspace::paths::cache_root;

fn dir() -> PathBuf {
    cache_root().join("analysis")
}

/// The cache key of `algorithm` run over `range` of the file at `path`, or
/// `None` when the file cannot be read (then nothing is cached).
pub fn key(algorithm: &str, path: &str, range: TimeRange) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let mut hash = Sha256::new();
    hash.update(algorithm.as_bytes());
    hash.update([0]);
    hash.update(path.as_bytes());
    hash.update([0]);
    hash.update(meta.len().to_le_bytes());
    hash.update(modified.to_le_bytes());
    hash.update(range.start.to_le_bytes());
    hash.update(range.duration.to_le_bytes());
    let digest = hash.finalize();
    Some(digest.iter().take(16).map(|b| format!("{b:02x}")).collect())
}

fn file(key: &str) -> PathBuf {
    dir().join(format!("{key}.json"))
}

/// The cached value under `key`, if there is a readable one.
pub fn load<T: DeserializeOwned>(key: &str) -> Option<T> {
    let bytes = std::fs::read(file(key)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Cache `value` under `key`. Failing to write is not an error: the cache is
/// only an optimisation.
pub fn store<T: Serialize>(key: &str, value: &T) {
    let path = file(key);
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(dir())?;
        let tmp = path.with_extension("json.part");
        std::fs::write(
            &tmp,
            serde_json::to_vec(value).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(error) = result {
        tracing::debug!(%error, path = %Path::new(&path).display(), "analysis cache not written");
    }
}
