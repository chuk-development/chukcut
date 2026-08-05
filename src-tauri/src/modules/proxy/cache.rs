//! The proxy cache: what has been built, where it is, and what gets thrown
//! away when the disk fills.
//!
//! Lives under [`paths::proxies_dir`], which existed before this module did and
//! until now nothing wrote to. Everything here is disposable by construction —
//! deleting the whole tree costs the user nothing but the time to rebuild.
//!
//! ## The key, and why it is four things
//!
//! A proxy is keyed on the source's **absolute path, byte size, modification
//! time, and eight kilobytes from each end of the file**. Path alone is the
//! obvious choice and it is wrong in the one case that matters: a source
//! re-exported over its own filename — a colour grade redone, a clip
//! re-rendered out of another tool — is a different picture at the same path,
//! and a path-keyed cache would go on serving the old one. The symptom is an
//! editor showing footage that no longer exists on disk, which is impossible to
//! diagnose from the outside.
//!
//! Size and mtime are what every build system uses, and on their own they are
//! **not enough**: the kernel stamps mtime at clock-tick granularity, so two
//! writes microseconds apart get the same one, and a file rewritten in place
//! without changing length keeps its key forever. That is not a theory — it is
//! what `media::thumbnails::fingerprint` was changed to fix, with a unit test
//! that writes `one` then `two` and watches both land in the same tick. The
//! same technique is used here (that function is `pub(super)` to `media`, so
//! this is the same idea rather than the same code) and the same test is below.
//!
//! What sixteen kilobytes still cannot catch is a same-length change confined to
//! the middle of a file whose mtime also did not move — a re-mux of identical
//! duration written inside one clock tick. Content-hashing a 40 GB source at
//! import costs more than that mistake, so this is a deliberate, documented
//! limit rather than an oversight.
//!
//! ## The cap
//!
//! A cache without a ceiling is a disk-full bug on a long enough timeline, and
//! proxies are big. [`ProxyCache`] holds a total byte cap and evicts
//! least-recently-*used* — not least-recently-built — because the useful thing
//! to keep is the footage the user is still cutting with, and a proxy built
//! this morning for a project already delivered is exactly what should go.
//!
//! ## The index
//!
//! `index.json` next to the files. It is a cache of a cache: if it is missing
//! or corrupt the answer is an empty index, not an error, and the proxies are
//! rebuilt. Written atomically (temp file plus rename) so a crash mid-write
//! leaves the previous index rather than half of the new one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::modules::workspace::paths;

use super::{ProxyError, Result};

/// How much disk proxies may occupy in total, by default.
///
/// 20 GiB holds roughly four hours of 720p all-intra proxy, which is more
/// footage than a single project on this class of machine. It is a constant
/// today and belongs in `workspace::settings` the moment anybody asks.
pub const DEFAULT_CAP_BYTES: u64 = 20 * 1024 * 1024 * 1024;

const INDEX_FILE: &str = "index.json";

/// Bytes read from each end of a file to identify its contents.
///
/// Enough to cover a container header — moov atom, EBML head, RIFF chunk table
/// — which is where an edit that keeps a file's length still shows up, and the
/// tail as well because an encoder that appends an index or a trailing moov
/// leaves the first kilobytes untouched.
const FINGERPRINT_SAMPLE: usize = 8 * 1024;

/// What identifies a source file's content, for cache purposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceKey {
    /// Absolute path as a string, exactly as the document stores it.
    pub path: String,
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch. `0` when the
    /// filesystem does not report one, which is rare and simply weakens the
    /// key to path-plus-size-plus-content.
    pub mtime_nanos: u128,
    /// FNV-1a over [`FINGERPRINT_SAMPLE`] bytes from each end of the file.
    ///
    /// The field that makes this key work, rather than merely look like it
    /// works — see the module documentation. `0` for a file that cannot be
    /// read, which cannot happen on the path that builds one of these but is
    /// the honest answer if it ever does.
    #[serde(default)]
    pub content: u64,
}

impl SourceKey {
    /// Read the key of a file that exists.
    pub fn of(path: &Path) -> Result<Self> {
        let mut file = std::fs::File::open(path).map_err(ProxyError::io(path))?;
        let metadata = file.metadata().map_err(ProxyError::io(path))?;
        let mtime_nanos = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Ok(Self {
            path: path.to_string_lossy().into_owned(),
            size: metadata.len(),
            mtime_nanos,
            content: head_and_tail(&mut file, metadata.len()),
        })
    }

    /// A short, filesystem-safe digest of the whole key.
    ///
    /// FNV-1a, as in `workspace::paths::path_key`, and for the same reason:
    /// this is a cache name, not a security boundary. The stem is prefixed so
    /// that a person browsing `~/.cache/chukcut/proxies` can tell what they are
    /// looking at.
    pub fn digest(&self) -> String {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;

        let mut hash = OFFSET;
        let mut eat = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(PRIME);
            }
        };
        eat(self.path.as_bytes());
        eat(&self.size.to_le_bytes());
        eat(&self.mtime_nanos.to_le_bytes());
        eat(&self.content.to_le_bytes());

        let stem: String = Path::new(&self.path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .take(32)
            .collect();

        if stem.is_empty() {
            format!("{hash:016x}")
        } else {
            format!("{stem}-{hash:016x}")
        }
    }
}

/// One built proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyEntry {
    pub key: SourceKey,
    /// File name inside the cache directory, not a full path — so moving the
    /// cache (a changed `XDG_CACHE_HOME`, a copied home directory) does not
    /// invalidate every entry.
    pub file: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    /// Milliseconds since the Unix epoch.
    pub created: u64,
    /// Milliseconds since the Unix epoch. The eviction order.
    ///
    /// Milliseconds rather than seconds because two proxies touched inside the
    /// same second would otherwise be indistinguishable, and the eviction order
    /// would fall back to a tie-break nobody can reason about.
    pub last_used: u64,
}

/// Totals for the settings UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheStats {
    pub entries: u64,
    pub bytes: u64,
    pub cap_bytes: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    entries: Vec<ProxyEntry>,
}

pub struct ProxyCache {
    root: PathBuf,
    cap_bytes: u64,
    index: Mutex<Index>,
}

impl ProxyCache {
    /// Open, or create, a cache at `root`.
    ///
    /// Never fails: a root that cannot be created and an index that cannot be
    /// parsed both degrade to "nothing is cached", which costs a transcode and
    /// loses nothing.
    pub fn open(root: impl Into<PathBuf>, cap_bytes: u64) -> Self {
        let root = root.into();
        if let Err(error) = std::fs::create_dir_all(&root) {
            tracing::warn!(%error, root = %root.display(), "could not create the proxy cache");
        }
        let index = load_index(&root);
        Self {
            root,
            cap_bytes,
            index: Mutex::new(index),
        }
    }

    /// The process-wide cache, under `paths::proxies_dir()`.
    ///
    /// One instance, shared. Two `ProxyCache` values over the same directory
    /// would each hold their own copy of the index and each write it back
    /// whole, so the last writer would silently delete the other's entries.
    pub fn shared() -> Arc<ProxyCache> {
        static SHARED: std::sync::OnceLock<Arc<ProxyCache>> = std::sync::OnceLock::new();
        Arc::clone(
            SHARED.get_or_init(|| {
                Arc::new(ProxyCache::open(paths::proxies_dir(), DEFAULT_CAP_BYTES))
            }),
        )
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cap_bytes(&self) -> u64 {
        self.cap_bytes
    }

    /// Where a proxy for `key` would be written.
    pub fn path_for(&self, key: &SourceKey) -> PathBuf {
        self.root.join(format!(
            "{}.{}",
            key.digest(),
            super::generate::PROXY_EXTENSION
        ))
    }

    /// The proxy for `source`, if one exists and is still valid.
    ///
    /// "Still valid" is the whole point: the key is recomputed from the file on
    /// disk *now*, so a source that has been re-exported since the proxy was
    /// built misses, and the stale entry is dropped on the way past.
    pub fn lookup(&self, source: &Path) -> Option<PathBuf> {
        let key = SourceKey::of(source).ok()?;
        self.lookup_by_key(&key)
    }

    /// [`Self::lookup`] with a key the caller already computed.
    pub fn lookup_by_key(&self, key: &SourceKey) -> Option<PathBuf> {
        let mut index = self.index.lock();
        let position = index.entries.iter().position(|e| e.key.path == key.path)?;

        // A different size or mtime at the same path is a different file. The
        // entry is not merely useless, it is actively wrong, so it goes.
        if &index.entries[position].key != key {
            let stale = index.entries.remove(position);
            drop_file(&self.root, &stale);
            self.save(&index);
            return None;
        }

        let file = self.root.join(&index.entries[position].file);
        if !file.exists() {
            // Somebody cleared the cache directory without telling us. The
            // index is the fiction here, not the disk.
            index.entries.remove(position);
            self.save(&index);
            return None;
        }

        index.entries[position].last_used = now_millis();
        self.save(&index);
        Some(file)
    }

    /// Record a proxy that has just been written, then evict down to the cap.
    ///
    /// The file must already be in place at [`Self::path_for`]; this records
    /// it. Returns the number of entries evicted, which is only interesting to
    /// a log line and a test.
    pub fn insert(&self, key: SourceKey, width: u32, height: u32) -> Result<usize> {
        let file = self.path_for(&key);
        let bytes = std::fs::metadata(&file)
            .map_err(ProxyError::io(&file))?
            .len();
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let now = now_millis();
        let entry = ProxyEntry {
            key,
            file: name,
            bytes,
            width,
            height,
            created: now,
            last_used: now,
        };

        let mut index = self.index.lock();
        index.entries.retain(|e| e.key.path != entry.key.path);
        index.entries.push(entry);
        let evicted = self.evict_locked(&mut index);
        self.save(&index);
        Ok(evicted)
    }

    /// Forget and delete the proxy for `source`, if there is one.
    pub fn remove(&self, source: &Path) -> bool {
        let wanted = source.to_string_lossy().into_owned();
        let mut index = self.index.lock();
        let Some(position) = index.entries.iter().position(|e| e.key.path == wanted) else {
            return false;
        };
        let entry = index.entries.remove(position);
        drop_file(&self.root, &entry);
        self.save(&index);
        true
    }

    /// Delete every proxy and forget them all.
    pub fn clear(&self) -> Result<()> {
        let mut index = self.index.lock();
        for entry in index.entries.drain(..) {
            drop_file(&self.root, &entry);
        }
        self.save(&index);
        Ok(())
    }

    pub fn stats(&self) -> CacheStats {
        let index = self.index.lock();
        CacheStats {
            entries: index.entries.len() as u64,
            bytes: index.entries.iter().map(|e| e.bytes).sum(),
            cap_bytes: self.cap_bytes,
        }
    }

    /// Every entry, newest use first. For a settings pane.
    pub fn entries(&self) -> Vec<ProxyEntry> {
        let mut entries = self.index.lock().entries.clone();
        entries.sort_by(|a, b| b.last_used.cmp(&a.last_used));
        entries
    }

    /// Bring the cache under its cap by deleting the least recently used
    /// entries. Returns how many went.
    pub fn evict(&self) -> usize {
        let mut index = self.index.lock();
        let evicted = self.evict_locked(&mut index);
        if evicted > 0 {
            self.save(&index);
        }
        evicted
    }

    fn evict_locked(&self, index: &mut Index) -> usize {
        let mut total: u64 = index.entries.iter().map(|e| e.bytes).sum();
        let mut evicted = 0;

        while total > self.cap_bytes && !index.entries.is_empty() {
            // Least recently used. Ties broken by creation time, so two
            // proxies touched in the same second still evict deterministically
            // — which is what makes the eviction order testable at all.
            let victim = index
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(position, e)| (e.last_used, e.created, *position))
                .map(|(position, _)| position);
            let Some(position) = victim else { break };

            let entry = index.entries.remove(position);
            total = total.saturating_sub(entry.bytes);
            tracing::info!(
                source = %entry.key.path,
                bytes = entry.bytes,
                "evicting a proxy to stay under the cache cap"
            );
            drop_file(&self.root, &entry);
            evicted += 1;
        }
        evicted
    }

    /// Persist the index. Failures are logged and swallowed: an index that
    /// cannot be written costs a rebuild after a restart, and there is nothing
    /// useful for a caller to do about it mid-transcode.
    fn save(&self, index: &Index) {
        let path = self.root.join(INDEX_FILE);
        let temporary = path.with_extension("json.tmp");
        let write = serde_json::to_vec_pretty(index)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&temporary, bytes))
            .and_then(|()| std::fs::rename(&temporary, &path));
        if let Err(error) = write {
            tracing::warn!(%error, path = %path.display(), "could not write the proxy index");
        }
    }
}

fn load_index(root: &Path) -> Index {
    let path = root.join(INDEX_FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        return Index::default();
    };
    match serde_json::from_slice::<Index>(&bytes) {
        Ok(index) => index,
        Err(error) => {
            tracing::warn!(%error, "the proxy index is unreadable; starting empty");
            Index::default()
        }
    }
}

fn drop_file(root: &Path, entry: &ProxyEntry) {
    let path = root.join(&entry.file);
    if let Err(error) = std::fs::remove_file(&path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%error, path = %path.display(), "could not delete a proxy");
        }
    }
}

/// FNV-1a over the first and last [`FINGERPRINT_SAMPLE`] bytes of `file`.
///
/// Reading 16 KB costs nothing measurable against the transcode it guards, and
/// it is what turns "size and mtime" from a guess into an answer for every
/// container that has a header — which is all of them.
fn head_and_tail(file: &mut std::fs::File, length: u64) -> u64 {
    use std::io::{Read, Seek, SeekFrom};

    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
        for byte in bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(PRIME);
        }
        hash
    }

    let mut hash = OFFSET;
    let head = FINGERPRINT_SAMPLE.min(length as usize);
    let mut buffer = vec![0u8; head];
    if file.read_exact(&mut buffer).is_ok() {
        hash = fnv1a(hash, &buffer);
    }
    if length > FINGERPRINT_SAMPLE as u64 {
        let mut tail = vec![0u8; FINGERPRINT_SAMPLE];
        if file
            .seek(SeekFrom::End(-(FINGERPRINT_SAMPLE as i64)))
            .is_ok()
            && file.read_exact(&mut tail).is_ok()
        {
            hash = fnv1a(hash, &tail);
        }
    }
    hash
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve a batch of sources against the cache in one pass.
///
/// Used by `media::provider` when it builds a snapshot: one lock, one pass, no
/// syscalls per frame.
pub fn resolve_all<'a>(
    cache: &ProxyCache,
    sources: impl IntoIterator<Item = &'a Path>,
) -> HashMap<PathBuf, PathBuf> {
    let mut resolved = HashMap::new();
    for source in sources {
        if let Some(proxy) = cache.lookup(source) {
            resolved.insert(source.to_path_buf(), proxy);
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A scratch directory that removes itself. `tempfile` is not a dependency
    /// of this crate and this is the whole of what it would be used for.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "chukcut-proxy-test-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            let mut file = std::fs::File::create(&path).expect("create");
            file.write_all(contents).expect("write");
            file.sync_all().ok();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Put a fake proxy of `bytes` bytes where the cache expects one.
    fn place(cache: &ProxyCache, key: &SourceKey, bytes: usize) {
        let path = cache.path_for(key);
        std::fs::write(&path, vec![0u8; bytes]).expect("place a proxy");
    }

    #[test]
    fn a_hit_is_a_hit() {
        let scratch = Scratch::new("hit");
        let cache = ProxyCache::open(scratch.path().join("cache"), DEFAULT_CAP_BYTES);
        let source = scratch.file("clip.mp4", b"original contents");

        assert!(cache.lookup(&source).is_none(), "nothing is cached yet");

        let key = SourceKey::of(&source).expect("key");
        place(&cache, &key, 1024);
        cache.insert(key.clone(), 1280, 720).expect("insert");

        let hit = cache.lookup(&source).expect("the proxy is there now");
        assert_eq!(hit, cache.path_for(&key));
        assert_eq!(cache.stats().entries, 1);
        assert_eq!(cache.stats().bytes, 1024);
    }

    /// The reason the key is three things and not one.
    #[test]
    fn rewriting_the_source_invalidates_its_proxy() {
        let scratch = Scratch::new("mtime");
        let cache = ProxyCache::open(scratch.path().join("cache"), DEFAULT_CAP_BYTES);
        let source = scratch.file("clip.mp4", b"take one");

        let key = SourceKey::of(&source).expect("key");
        place(&cache, &key, 1024);
        cache.insert(key.clone(), 1280, 720).expect("insert");
        assert!(cache.lookup(&source).is_some());

        // Re-export over the same filename, same length. Only the mtime moves.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&source, b"take two").expect("rewrite");
        let after = SourceKey::of(&source).expect("key");
        assert_eq!(after.size, key.size, "the test needs the size to be equal");
        assert_ne!(after.mtime_nanos, key.mtime_nanos, "mtime must have moved");

        assert!(
            cache.lookup(&source).is_none(),
            "a re-exported source must not keep serving the old proxy"
        );
        // And the wrong file is gone, not merely unreferenced.
        assert_eq!(cache.stats().entries, 0);
        assert!(!cache.path_for(&key).exists());
    }

    /// The one size and mtime cannot catch, and the reason the key reads bytes.
    ///
    /// Two writes microseconds apart get the same mtime from the kernel, so a
    /// source rewritten in place without changing length would otherwise keep
    /// its key and serve the old proxy forever. `media::thumbnails` has the
    /// same test for the same reason.
    #[test]
    fn a_rewrite_inside_one_clock_tick_still_invalidates() {
        let scratch = Scratch::new("same-tick");
        let cache = ProxyCache::open(scratch.path().join("cache"), DEFAULT_CAP_BYTES);
        let source = scratch.file("clip.mp4", b"one");

        let before = SourceKey::of(&source).expect("key");
        place(&cache, &before, 128);
        cache.insert(before.clone(), 1280, 720).expect("insert");
        assert!(cache.lookup(&source).is_some());

        // Same length, immediately after — no sleep, so the mtime very likely
        // does not move at all.
        std::fs::write(&source, b"two").expect("rewrite");
        let after = SourceKey::of(&source).expect("key");
        assert_eq!(after.size, before.size);
        assert_ne!(
            after.content, before.content,
            "the content sample must notice a same-length rewrite"
        );

        assert!(
            cache.lookup(&source).is_none(),
            "a source rewritten in place must not keep serving the old proxy"
        );
    }

    #[test]
    fn a_changed_size_invalidates_too() {
        let scratch = Scratch::new("size");
        let cache = ProxyCache::open(scratch.path().join("cache"), DEFAULT_CAP_BYTES);
        let source = scratch.file("clip.mp4", b"short");

        let key = SourceKey::of(&source).expect("key");
        place(&cache, &key, 1024);
        cache.insert(key, 1280, 720).expect("insert");

        std::fs::write(&source, b"a good deal longer than before").expect("rewrite");
        assert!(cache.lookup(&source).is_none());
    }

    #[test]
    fn deleting_the_proxy_behind_the_index_is_a_miss_not_a_crash() {
        let scratch = Scratch::new("gone");
        let cache = ProxyCache::open(scratch.path().join("cache"), DEFAULT_CAP_BYTES);
        let source = scratch.file("clip.mp4", b"x");

        let key = SourceKey::of(&source).expect("key");
        place(&cache, &key, 32);
        cache.insert(key.clone(), 1280, 720).expect("insert");

        std::fs::remove_file(cache.path_for(&key)).expect("remove");
        assert!(cache.lookup(&source).is_none());
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn eviction_takes_the_least_recently_used_first() {
        let scratch = Scratch::new("evict");
        // Cap of 250 bytes; three 100-byte proxies will not fit.
        let cache = ProxyCache::open(scratch.path().join("cache"), 250);

        let mut keys = Vec::new();
        for name in ["a.mp4", "b.mp4", "c.mp4"] {
            let source = scratch.file(name, name.as_bytes());
            let key = SourceKey::of(&source).expect("key");
            place(&cache, &key, 100);
            cache.insert(key.clone(), 1280, 720).expect("insert");
            keys.push((source, key));
        }

        // Two survive, and the survivors are the two most recently used. All
        // three were inserted in order, so `a` is the oldest use.
        let stats = cache.stats();
        assert_eq!(stats.entries, 2, "the cap must have forced an eviction");
        assert!(stats.bytes <= 250);
        assert!(!cache.path_for(&keys[0].1).exists(), "a should have gone");
        assert!(cache.path_for(&keys[1].1).exists());
        assert!(cache.path_for(&keys[2].1).exists());

        // Now touch `b` so it is the most recent, add a fourth, and watch `c`
        // go rather than `b` — which is the difference between least recently
        // *used* and least recently *built*.
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(cache.lookup(&keys[1].0).is_some(), "touch b");

        std::thread::sleep(std::time::Duration::from_millis(5));
        let fourth = scratch.file("d.mp4", b"d.mp4");
        let key = SourceKey::of(&fourth).expect("key");
        place(&cache, &key, 100);
        cache.insert(key.clone(), 1280, 720).expect("insert");

        assert_eq!(cache.stats().entries, 2);
        assert!(cache.path_for(&key).exists(), "the new one stays");
        assert!(
            cache.path_for(&keys[1].1).exists(),
            "b was used most recently"
        );
        assert!(!cache.path_for(&keys[2].1).exists(), "c was the stalest");
    }

    #[test]
    fn re_proxying_the_same_source_replaces_rather_than_accumulates() {
        let scratch = Scratch::new("replace");
        let cache = ProxyCache::open(scratch.path().join("cache"), DEFAULT_CAP_BYTES);
        let source = scratch.file("clip.mp4", b"one");

        let first = SourceKey::of(&source).expect("key");
        place(&cache, &first, 100);
        cache.insert(first, 1280, 720).expect("insert");

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&source, b"two").expect("rewrite");
        let second = SourceKey::of(&source).expect("key");
        place(&cache, &second, 200);
        cache.insert(second, 1280, 720).expect("insert");

        assert_eq!(cache.stats().entries, 1, "one source, one proxy");
        assert_eq!(cache.stats().bytes, 200);
    }

    #[test]
    fn the_index_survives_a_reopen() {
        let scratch = Scratch::new("reopen");
        let root = scratch.path().join("cache");
        let source = scratch.file("clip.mp4", b"contents");
        let key = SourceKey::of(&source).expect("key");

        {
            let cache = ProxyCache::open(&root, DEFAULT_CAP_BYTES);
            place(&cache, &key, 512);
            cache.insert(key.clone(), 1280, 720).expect("insert");
        }

        let reopened = ProxyCache::open(&root, DEFAULT_CAP_BYTES);
        assert_eq!(reopened.stats().entries, 1);
        assert!(reopened.lookup(&source).is_some());
    }

    #[test]
    fn a_corrupt_index_starts_empty_rather_than_failing() {
        let scratch = Scratch::new("corrupt");
        let root = scratch.path().join("cache");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join(INDEX_FILE), b"{ this is not json").expect("write");

        let cache = ProxyCache::open(&root, DEFAULT_CAP_BYTES);
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn keys_distinguish_files_and_survive_a_round_trip() {
        let a = SourceKey {
            path: "/footage/clip.mp4".into(),
            size: 100,
            mtime_nanos: 5,
            content: 7,
        };
        let same_path_new_content = SourceKey {
            mtime_nanos: 6,
            ..a.clone()
        };
        let other_file = SourceKey {
            path: "/elsewhere/clip.mp4".into(),
            ..a.clone()
        };

        let same_everything_but_content = SourceKey {
            content: 8,
            ..a.clone()
        };

        assert_ne!(a.digest(), same_path_new_content.digest());
        assert_ne!(a.digest(), other_file.digest());
        assert_ne!(a.digest(), same_everything_but_content.digest());
        assert_eq!(a.digest(), a.clone().digest());
        // Readable in a file browser, and safe as a filename.
        assert!(a.digest().starts_with("clip-"));
        assert!(a
            .digest()
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn clearing_removes_the_files_too() {
        let scratch = Scratch::new("clear");
        let cache = ProxyCache::open(scratch.path().join("cache"), DEFAULT_CAP_BYTES);
        let source = scratch.file("clip.mp4", b"x");
        let key = SourceKey::of(&source).expect("key");
        place(&cache, &key, 64);
        cache.insert(key.clone(), 1280, 720).expect("insert");

        cache.clear().expect("clear");
        assert_eq!(cache.stats().entries, 0);
        assert!(!cache.path_for(&key).exists());
    }
}
