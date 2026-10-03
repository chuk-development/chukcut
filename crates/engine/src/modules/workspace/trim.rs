//! Keeping the disk cache under the user's limit (Settings → Cache limit).
//!
//! Everything under [`paths::cache_root`] is derived and rebuildable, so the
//! rule is plain least-recently-used: walk the tree, and delete the files used
//! longest ago until the total fits. "Used" is the later of a file's access and
//! modification times — Linux's default `relatime` still moves atime forward
//! once a day on read, which is resolution enough to tell this week's
//! thumbnails from last month's. Proxies are the exception: their cache index
//! records every lookup to the millisecond, and that is the better clock, so it
//! overrides the file's own times.
//!
//! ## What is never deleted
//!
//! - **The open project's derived files**, as far as that is cheap to know:
//!   the thumbnail and waveform directories of its media (both are keyed by
//!   path alone) and the proxies the cache index maps to its media. Voice
//!   cleanup renders are keyed by path *and* strength *and* engine, which this
//!   module cannot reconstruct without the document's clip settings; they are
//!   ordinary LRU candidates and are re-rendered on demand if trimmed.
//! - **Files being written**: dot-prefixed (`.partial-proxy.mp4`) and
//!   `*.part` / `*.tmp` names. Deleting one mid-write would fail a job that
//!   is about to make the cache useful.
//! - **`whisper/`**, the speech models. They live under the cache root because
//!   they can be fetched again, but a 1.5 GB model is a download the user chose,
//!   not derived data, and evicting it would turn the next transcription into
//!   a surprise download. It is outside the count as well as the deletion, so
//!   the limit is about what the app derives on its own.
//! - **The proxy index**, `proxies/index.json`.
//!
//! Protected files still count towards the total. A limit smaller than what
//! the open project needs deletes everything else and stops there, rather than
//! breaking the project in use.
//!
//! ## When it runs
//!
//! - **At startup**, but not before the engine knows what is in use: the first
//!   time a project is opened, created, restored or closed. `crate::init`
//!   runs before the startup project is open, and a trim started there would
//!   race the open and could delete the thumbnails it is about to show.
//! - **When the limit changes** (`workspace_settings_apply`), once the startup
//!   trim has been allowed to run.
//! - **After every proxy** that lands in the cache (the shared proxy queue's
//!   ready hook).
//!
//! Always on a background thread, because the walk is IO. Two trims never
//! overlap; the second waits for the first.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::paths;

/// The configured limit in bytes. 0 means none, and is also the state before
/// a shell has applied the settings — so a test that never does cannot trim
/// the real cache by accident.
static LIMIT: AtomicU64 = AtomicU64::new(0);

/// The open project's media paths, as the document stores them.
static IN_USE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Held for the whole of a trim, so two never interleave their deletions.
static RUNNING: Mutex<()> = Mutex::new(());

/// Whether the open document's media has been reported at least once, which
/// is what makes a trim safe to start. See "When it runs".
static IN_USE_KNOWN: AtomicBool = AtomicBool::new(false);

/// Top-level directories under the cache root that are never trimmed or
/// counted. See the module docs.
const EXEMPT_DIRS: [&str; 1] = ["whisper"];

/// Set the limit trims run against. Returns the previous one.
pub fn set_limit(bytes: u64) -> u64 {
    LIMIT.swap(bytes, Ordering::Relaxed)
}

pub fn limit() -> u64 {
    LIMIT.load(Ordering::Relaxed)
}

/// Record which media the open project uses, so trimming leaves their derived
/// files alone. An empty list when nothing is open.
///
/// The first call of a run is also the startup trim.
pub fn set_in_use(media: Vec<PathBuf>) {
    *IN_USE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = media;
    if !IN_USE_KNOWN.swap(true, Ordering::Relaxed) {
        trim_in_background();
    }
}

/// Whether a trim may start without knowing the open project — false until
/// the first [`set_in_use`].
pub fn in_use_known() -> bool {
    IN_USE_KNOWN.load(Ordering::Relaxed)
}

pub fn in_use() -> Vec<PathBuf> {
    IN_USE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// What one trim did, for a log line and a settings readout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrimReport {
    pub limit_bytes: u64,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub removed_files: u64,
    pub removed_bytes: u64,
    /// Bytes that counted towards the total but could not be deleted.
    pub protected_bytes: u64,
}

/// One file the trim considered.
#[derive(Debug, Clone)]
pub struct CacheFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub last_used: SystemTime,
    pub protected: bool,
}

/// What must survive a trim: exact files and whole directories.
#[derive(Debug, Clone, Default)]
pub struct Protection {
    pub files: HashSet<PathBuf>,
    pub dirs: Vec<PathBuf>,
}

impl Protection {
    fn covers(&self, path: &Path) -> bool {
        self.files.contains(path) || self.dirs.iter().any(|dir| path.starts_with(dir))
    }
}

/// Which files to delete, as indices into `files`: least recently used first,
/// until the total — protected files included — is at or under `limit`.
/// `limit` 0 means no limit and deletes nothing.
///
/// Pure, so the order is testable without a filesystem.
pub fn plan(files: &[CacheFile], limit: u64) -> Vec<usize> {
    if limit == 0 {
        return Vec::new();
    }
    let mut total: u64 = files.iter().map(|f| f.bytes).sum();
    if total <= limit {
        return Vec::new();
    }
    let mut candidates: Vec<usize> = (0..files.len()).filter(|&i| !files[i].protected).collect();
    // The path breaks ties so two files stamped in the same tick still go in
    // an order a test can name.
    candidates.sort_by(|&a, &b| {
        (files[a].last_used, &files[a].path).cmp(&(files[b].last_used, &files[b].path))
    });
    let mut doomed = Vec::new();
    for index in candidates {
        if total <= limit {
            break;
        }
        total = total.saturating_sub(files[index].bytes);
        doomed.push(index);
    }
    doomed
}

/// Trim the tree under `root` to `limit` bytes.
///
/// `last_used` overrides a file's own times where the caller knows better (the
/// proxy index). Directories left empty below the first level are removed;
/// first-level ones (`proxies/`, `thumbnails/`) are kept, because their owners
/// expect them to exist.
pub fn trim_dir(
    root: &Path,
    limit: u64,
    protection: &Protection,
    last_used: &HashMap<PathBuf, SystemTime>,
) -> TrimReport {
    let mut files = Vec::new();
    collect(root, root, protection, last_used, &mut files);

    let before: u64 = files.iter().map(|f| f.bytes).sum();
    let mut report = TrimReport {
        limit_bytes: limit,
        before_bytes: before,
        after_bytes: before,
        protected_bytes: files.iter().filter(|f| f.protected).map(|f| f.bytes).sum(),
        ..TrimReport::default()
    };

    for index in plan(&files, limit) {
        let file = &files[index];
        match std::fs::remove_file(&file.path) {
            Ok(()) => {
                report.removed_files += 1;
                report.removed_bytes += file.bytes;
                remove_empty_parents(root, &file.path);
            }
            // Gone already: somebody else cleaned up, which is the same result.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                report.removed_bytes += file.bytes;
            }
            Err(error) => {
                tracing::warn!(%error, path = %file.path.display(), "could not trim a cache file");
            }
        }
    }
    report.after_bytes = before.saturating_sub(report.removed_bytes);
    report
}

fn collect(
    root: &Path,
    dir: &Path,
    protection: &Protection,
    last_used: &HashMap<PathBuf, SystemTime>,
    out: &mut Vec<CacheFile>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `symlink_metadata`: a link inside the cache is followed by nobody,
        // and deleting one deletes the link, not its target.
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            let exempt = dir == root
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| EXEMPT_DIRS.contains(&name));
            if !exempt {
                collect(root, &path, protection, last_used, out);
            }
            continue;
        }
        let used = last_used.get(&path).copied().unwrap_or_else(|| {
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            let accessed = metadata.accessed().unwrap_or(UNIX_EPOCH);
            modified.max(accessed)
        });
        out.push(CacheFile {
            protected: being_written(&path) || protection.covers(&path),
            path,
            bytes: metadata.len(),
            last_used: used,
        });
    }
}

fn being_written(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    name.starts_with('.') || name.ends_with(".part") || name.ends_with(".tmp")
}

fn remove_empty_parents(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir {
        // Stop at the first level: `root/proxies` must outlive its contents.
        if current.parent() == Some(root) || !current.starts_with(root) || current == root {
            break;
        }
        if std::fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}

/// Trim the real cache to `limit`, protecting the open project's files.
pub fn trim_cache(limit: u64) -> TrimReport {
    let _running = RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let proxies = crate::modules::proxy::ProxyCache::shared();
    let media = in_use();
    let mut protection = Protection::default();
    for path in &media {
        protection.dirs.push(paths::thumbnails_dir(path));
        protection.dirs.push(paths::waveform_dir(path));
    }
    protection.files.insert(proxies.root().join("index.json"));
    let wanted: HashSet<String> = media
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut last_used = HashMap::new();
    for entry in proxies.entries() {
        let file = proxies.root().join(&entry.file);
        if wanted.contains(&entry.key.path) {
            protection.files.insert(file.clone());
        }
        last_used.insert(file, UNIX_EPOCH + Duration::from_millis(entry.last_used));
    }

    let report = trim_dir(&paths::cache_root(), limit, &protection, &last_used);
    if report.removed_files > 0 {
        // Deleted proxy files would otherwise stay in the index, counted in
        // the settings readout until something looked them up.
        proxies.forget_missing();
        tracing::info!(?report, "trimmed the cache to its limit");
    }
    report
}

/// [`trim_cache`] against the configured limit; nothing when there is none,
/// or before the open project is known.
pub fn trim_to_configured_limit() {
    let limit = limit();
    if limit > 0 && in_use_known() {
        trim_cache(limit);
    }
}

/// [`trim_to_configured_limit`] on its own thread.
pub fn trim_in_background() {
    if limit() == 0 || !in_use_known() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("chukcut-cache-trim".into())
        .spawn(trim_to_configured_limit);
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the cache trim");
    }
}

/// A scratch directory inside the build's own target directory, for tests
/// that need a real filesystem. Next to the test binary rather than in the
/// system temp directory, so a test run never writes outside `target/`.
#[cfg(test)]
pub(crate) fn test_scratch(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("test binary path");
    // target/<profile>/deps/<binary> → target/<profile>/test-scratch/<name>
    let base = exe
        .parent()
        .and_then(Path::parent)
        .expect("test binary inside target/");
    let dir = base.join("test-scratch").join(format!(
        "{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds)
    }

    fn file(name: &str, bytes: u64, used: u64, protected: bool) -> CacheFile {
        CacheFile {
            path: PathBuf::from(name),
            bytes,
            last_used: at(used),
            protected,
        }
    }

    fn names(files: &[CacheFile], picked: &[usize]) -> Vec<String> {
        picked
            .iter()
            .map(|&i| files[i].path.display().to_string())
            .collect()
    }

    #[test]
    fn the_plan_takes_the_oldest_first_and_stops_at_the_limit() {
        let files = [
            file("new", 100, 30, false),
            file("old", 100, 10, false),
            file("middle", 100, 20, false),
        ];
        // 300 bytes against 150: two files must go, oldest first.
        assert_eq!(names(&files, &plan(&files, 150)), ["old", "middle"]);
        // Exactly at the limit is under it.
        assert!(plan(&files, 300).is_empty());
        // 0 is "no limit", not "delete everything".
        assert!(plan(&files, 0).is_empty());
    }

    #[test]
    fn the_plan_counts_protected_files_but_never_picks_them() {
        let files = [
            file("in-use", 500, 1, true),
            file("old", 100, 2, false),
            file("new", 100, 3, false),
        ];
        // The protected file alone is over the limit: everything else goes
        // and the plan stops there.
        assert_eq!(names(&files, &plan(&files, 300)), ["old", "new"]);
    }

    fn write(path: &Path, bytes: usize, seconds: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![7u8; bytes]).unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(at(seconds))
                .set_accessed(at(seconds)),
        )
        .unwrap();
    }

    /// The end-to-end shape on a real directory: oldest first down to the
    /// limit, the open project's files and in-progress writes untouched, the
    /// speech models outside the count, emptied per-media directories gone.
    #[test]
    fn trimming_removes_the_oldest_first_and_leaves_protected_files() {
        let root = test_scratch("trim");
        let thumbs_in_use = root.join("thumbnails/clip-in-use");
        write(&thumbs_in_use.join("strip.jpg"), 1000, 1);
        write(&root.join("thumbnails/old-clip/strip.jpg"), 1000, 2);
        write(&root.join("waveforms/old-clip/peaks.bin"), 1000, 3);
        write(&root.join("voice/aaaa.wav"), 1000, 4);
        write(&root.join("proxies/recent.mp4"), 1000, 5);
        write(&root.join("proxies/.partial-proxy.mp4"), 1000, 0);
        write(&root.join("whisper/ggml-base.bin"), 50_000, 0);

        let protection = Protection {
            files: HashSet::new(),
            dirs: vec![thumbs_in_use.clone()],
        };
        // Six counted files, 6000 bytes; 3500 leaves room for the two
        // protected ones and the most recent unprotected one.
        let report = trim_dir(&root, 3500, &protection, &HashMap::new());

        assert_eq!(report.before_bytes, 6000, "whisper is outside the count");
        assert_eq!(report.removed_files, 3);
        assert_eq!(report.after_bytes, 3000);
        assert_eq!(report.protected_bytes, 2000);
        assert!(thumbs_in_use.join("strip.jpg").exists(), "in use");
        assert!(
            root.join("proxies/.partial-proxy.mp4").exists(),
            "being written"
        );
        assert!(root.join("proxies/recent.mp4").exists(), "most recent");
        assert!(root.join("whisper/ggml-base.bin").exists(), "exempt");
        assert!(
            !root.join("thumbnails/old-clip").exists(),
            "oldest, and its empty dir"
        );
        assert!(!root.join("waveforms/old-clip/peaks.bin").exists());
        assert!(!root.join("voice/aaaa.wav").exists());
        assert!(root.join("voice").exists(), "first-level dirs stay");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The proxy index's millisecond clock wins over the file's own times: a
    /// proxy looked up a moment ago is recent, however old its mtime.
    #[test]
    fn a_known_last_use_overrides_the_file_times() {
        let root = test_scratch("trim-override");
        write(&root.join("proxies/looked-up.mp4"), 1000, 1);
        write(&root.join("thumbnails/a/strip.jpg"), 1000, 2);

        let mut last_used = HashMap::new();
        last_used.insert(root.join("proxies/looked-up.mp4"), at(100));
        let report = trim_dir(&root, 1000, &Protection::default(), &last_used);

        assert_eq!(report.removed_files, 1);
        assert!(root.join("proxies/looked-up.mp4").exists());
        assert!(!root.join("thumbnails/a/strip.jpg").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_limit_touches_nothing() {
        let root = test_scratch("trim-unlimited");
        write(&root.join("voice/a.wav"), 1000, 1);
        let report = trim_dir(&root, 0, &Protection::default(), &HashMap::new());
        assert_eq!(report.removed_files, 0);
        assert!(root.join("voice/a.wav").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
