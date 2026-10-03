//! Where things live on disk.
//!
//! Every module that needs to write something — thumbnails, waveform peaks,
//! proxies, rendered frame caches, settings — asks here rather than deciding
//! for itself. One place to change when we add a "cache location" setting, one
//! place to look when the disk fills up, and one place that knows the platform
//! conventions.
//!
//! The layout follows the XDG spec on Linux and its equivalents elsewhere:
//! caches are disposable and go in the cache directory, settings are precious
//! and go in the config directory. Deleting the whole cache tree must never
//! lose user data — that invariant is what lets us offer a "clear cache"
//! button without a scary warning.

use std::path::{Path, PathBuf};

const APP_DIR: &str = "chukcut";

/// Root for disposable derived data.
///
/// Linux: `~/.cache/chukcut`, macOS: `~/Library/Caches/chukcut`,
/// Windows: `%LOCALAPPDATA%\chukcut\cache`.
pub fn cache_root() -> PathBuf {
    dirs_cache()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

/// Root for settings and other state we must not lose.
pub fn config_root() -> PathBuf {
    dirs_config()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

/// Where the log files go.
///
/// Neither cache nor config: a log is not derived data the app can regenerate,
/// and it is not something the user would miss if it vanished. That is exactly
/// what the XDG *state* directory is for, so on Linux this is
/// `$XDG_STATE_HOME/chukcut/logs`, or `~/.local/state/chukcut/logs` when the
/// variable is unset. macOS and Windows have their own conventions for the same
/// idea and get them.
///
/// It matters that this is not under [`cache_root`]: "clear cache" must not
/// delete the log of the export the user is about to ask us about.
#[cfg(target_os = "linux")]
pub fn logs_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".local/state")))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
        .join("logs")
}

/// See the Linux version. `~/Library/Logs/chukcut` is where a macOS user — and
/// Console.app — expects to find this.
#[cfg(target_os = "macos")]
pub fn logs_dir() -> PathBuf {
    home()
        .map(|h| h.join("Library/Logs"))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

/// See the Linux version.
#[cfg(target_os = "windows")]
pub fn logs_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
        .join("logs")
}

/// Thumbnail strips for a media file, keyed by a hash of its path so two files
/// with the same name in different folders do not collide.
pub fn thumbnails_dir(media_path: &Path) -> PathBuf {
    cache_root().join("thumbnails").join(path_key(media_path))
}

/// Audio peak data for a media file.
pub fn waveform_dir(media_path: &Path) -> PathBuf {
    cache_root().join("waveforms").join(path_key(media_path))
}

/// Reduced-resolution copies of heavy source media.
pub fn proxies_dir() -> PathBuf {
    cache_root().join("proxies")
}

/// Rendered preview frames, per session. Sessions are disposable and cleaned
/// up when the app starts, since a frame cache from a previous run describes a
/// document state that no longer exists.
pub fn preview_dir(session: u64) -> PathBuf {
    cache_root().join("preview").join(session.to_string())
}

pub fn settings_file() -> PathBuf {
    config_root().join("settings.json")
}

/// The working copy of whatever is currently open.
///
/// Written after every edit and reloaded at startup, so closing the app — or a
/// crash, or a rebuild during development — never costs the work in progress.
/// This is not the user's saved project: it is a crash-recovery copy, and
/// saving explicitly still writes wherever they chose.
pub fn autosave_file() -> PathBuf {
    config_root().join("autosave.chukcut")
}

pub fn recent_projects_file() -> PathBuf {
    config_root().join("recent.json")
}

/// Root for user data the app keeps on the user's behalf — files they
/// imported and would miss, which are neither settings nor cache.
///
/// Linux: `$XDG_DATA_HOME/chukcut`, or `~/.local/share/chukcut` when the
/// variable is unset.
pub fn data_root() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

/// The LUT library: `.cube` files imported through the colour panel are
/// copied here, so a project keeps its look when the original download is
/// cleaned up. Under [`data_root`], never the cache: "clear cache" must not
/// take a look away from a project that uses it.
pub fn luts_dir() -> PathBuf {
    data_root().join("luts")
}

/// Stills made by "Freeze frame". Under [`data_root`], never the cache: the
/// project references the file, and "clear cache" must not take a picture
/// out of somebody's cut.
pub fn freeze_frames_dir() -> PathBuf {
    data_root().join("freeze-frames")
}

/// Create a directory and every missing parent, returning it for chaining.
pub fn ensure(dir: PathBuf) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Remove everything under the cache root. Safe by construction: nothing that
/// cannot be regenerated is stored there.
pub fn clear_cache() -> std::io::Result<()> {
    let root = cache_root();
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    Ok(())
}

/// Total bytes currently held in the cache, for the settings UI.
pub fn cache_size() -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|e| match e.metadata() {
                Ok(m) if m.is_dir() => walk(&e.path()),
                Ok(m) => m.len(),
                Err(_) => 0,
            })
            .sum()
    }
    walk(&cache_root())
}

/// A short, filesystem-safe, collision-resistant key for an absolute path.
///
/// FNV-1a rather than a cryptographic hash: this is a cache key, not a
/// security boundary, and it needs to be cheap enough to call per media file
/// without thinking about it.
fn path_key(path: &Path) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let bytes = path.to_string_lossy();
    let mut hash = OFFSET;
    for byte in bytes.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }

    // Prefix with the file stem so a human browsing the cache can tell what is
    // in there; the hash is what actually guarantees uniqueness.
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem: String = stem
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

#[cfg(target_os = "linux")]
fn dirs_cache() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".cache")))
}

#[cfg(target_os = "linux")]
fn dirs_config() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".config")))
}

#[cfg(target_os = "macos")]
fn dirs_cache() -> Option<PathBuf> {
    home().map(|h| h.join("Library/Caches"))
}

#[cfg(target_os = "macos")]
fn dirs_config() -> Option<PathBuf> {
    home().map(|h| h.join("Library/Application Support"))
}

#[cfg(target_os = "windows")]
fn dirs_cache() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("cache"))
}

#[cfg(target_os = "windows")]
fn dirs_config() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(not(target_os = "windows"))]
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_keys_are_stable_and_distinct() {
        let a = path_key(Path::new("/videos/a/clip.mp4"));
        let b = path_key(Path::new("/videos/b/clip.mp4"));
        assert_ne!(a, b, "same filename in different folders must not collide");
        assert_eq!(a, path_key(Path::new("/videos/a/clip.mp4")));
        assert!(a.starts_with("clip-"));
    }

    #[test]
    fn path_keys_are_filesystem_safe() {
        let key = path_key(Path::new("/tmp/my clip (final)!.mov"));
        assert!(key
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn cache_and_config_are_separate_trees() {
        assert_ne!(cache_root(), config_root());
    }

    /// The point of putting logs in the state directory: "clear cache" is
    /// allowed to delete everything under [`cache_root`], and the log of the
    /// export somebody is about to report must not be in there.
    #[test]
    fn logs_are_not_inside_the_cache() {
        assert!(!logs_dir().starts_with(cache_root()));
    }
}
