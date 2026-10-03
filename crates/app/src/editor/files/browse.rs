//! The file browser's logic, without a window: what a folder shows, which
//! files a request accepts, where the side bar points, what a typed path
//! means, and which folders were used last. `dialog.rs` draws it.

use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// What a request is for, which decides the files shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Filter {
    /// `.chukcut` project files.
    Projects,
    /// Video, audio and pictures: what Import takes.
    Media,
    /// `.srt` and `.vtt`.
    Subtitles,
    /// `.cube` colour lookup tables.
    Luts,
    /// PNG stills (Save frame).
    Png,
    /// Everything.
    Any,
}

const VIDEO: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "mts", "m2ts", "ts", "mpg", "mpeg", "wmv", "flv",
    "3gp", "mxf", "ogv",
];
const AUDIO: &[&str] = &[
    "mp3", "wav", "flac", "aac", "m4a", "ogg", "oga", "opus", "wma", "aif", "aiff",
];
const IMAGE: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "avif", "heic",
];

impl Filter {
    /// The extensions it accepts, lower case; empty for [`Filter::Any`].
    pub(crate) fn extensions(self) -> Vec<&'static str> {
        match self {
            Filter::Projects => vec!["chukcut"],
            Filter::Media => [VIDEO, AUDIO, IMAGE].concat(),
            Filter::Subtitles => vec!["srt", "vtt"],
            Filter::Luts => vec!["cube"],
            Filter::Png => vec!["png"],
            Filter::Any => Vec::new(),
        }
    }

    /// What the filter switch says.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Filter::Projects => "chukcut projects",
            Filter::Media => "Video, audio and images",
            Filter::Subtitles => "Subtitles (.srt, .vtt)",
            Filter::Luts => "LUTs (.cube)",
            Filter::Png => "PNG images",
            Filter::Any => "All files",
        }
    }

    pub(crate) fn accepts(self, path: &Path) -> bool {
        if self == Filter::Any {
            return true;
        }
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        let extension = extension.to_ascii_lowercase();
        self.extensions().iter().any(|e| *e == extension)
    }
}

/// What the user is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Existing files; several when `multiple`.
    Open { multiple: bool },
    /// One existing folder.
    Folder,
    /// A new file's path, starting from the suggested `name`.
    Save { name: String },
}

/// One row of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// The rows of `dir` for `mode`: folders first, then the files `filter`
/// accepts (none when choosing a folder), each by name without regard to
/// case. Dot files are left out unless `hidden`.
pub(crate) fn list(
    dir: &Path,
    mode: &Mode,
    filter: Filter,
    hidden: bool,
) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let path = item.path();
        // `metadata` follows links, so a link to a folder is a folder; a
        // dangling link is left out.
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let is_dir = metadata.is_dir();
        if !is_dir && (*mode == Mode::Folder || !filter.accepts(&path)) {
            continue;
        }
        entries.push(Entry {
            name,
            path,
            is_dir,
            size: if is_dir { 0 } else { metadata.len() },
            modified: metadata.modified().ok(),
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

/// A side-bar entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub label: String,
    pub path: PathBuf,
}

/// The standard folders that exist: Home, the XDG user folders (under their
/// localised names from `user-dirs.dirs`), and the file system root.
pub(crate) fn places(home: &Path, config: &Path) -> Vec<Place> {
    let mut out = vec![Place {
        label: "Home".into(),
        path: home.to_path_buf(),
    }];
    let user_dirs = std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    for (key, label) in [
        ("XDG_DESKTOP_DIR", "Desktop"),
        ("XDG_VIDEOS_DIR", "Videos"),
        ("XDG_PICTURES_DIR", "Pictures"),
        ("XDG_MUSIC_DIR", "Music"),
        ("XDG_DOCUMENTS_DIR", "Documents"),
        ("XDG_DOWNLOAD_DIR", "Downloads"),
    ] {
        let path = user_dir(&user_dirs, key, home).unwrap_or_else(|| home.join(label));
        if path != home && path.is_dir() && !out.iter().any(|p| p.path == path) {
            out.push(Place {
                label: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| label.into()),
                path,
            });
        }
    }
    out.push(Place {
        label: "Computer".into(),
        path: PathBuf::from("/"),
    });
    out
}

/// One folder from `user-dirs.dirs`: `XDG_VIDEOS_DIR="$HOME/Videos"`.
fn user_dir(file: &str, key: &str, home: &Path) -> Option<PathBuf> {
    let line = file
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(key) && line[key.len()..].starts_with('='))?;
    let value = line[key.len() + 1..].trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    path.is_absolute().then_some(path)
}

/// What a typed path means: `~` is home, a relative path is taken from the
/// folder on screen, and `.` and `..` are resolved without touching the disk.
pub(crate) fn resolve_typed(input: &str, current: &Path, home: &Path) -> PathBuf {
    let input = input.trim();
    let raw = if input == "~" {
        home.to_path_buf()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(input).is_absolute() {
        PathBuf::from(input)
    } else {
        current.join(input)
    };
    let mut out = PathBuf::new();
    for component in raw.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        out
    }
}

/// The file a Save writes: `name` in `dir`, with `filter`'s first extension
/// added when the name has none of the accepted ones.
pub(crate) fn save_target(dir: &Path, name: &str, filter: Filter) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return None;
    }
    let mut path = dir.join(name);
    if let Some(extension) = filter.extensions().first() {
        if !filter.accepts(&path) {
            path = dir.join(format!("{name}.{extension}"));
        }
    }
    Some(path)
}

/// Where the browser opens: the asked-for folder if it exists, else its
/// nearest existing parent, else home.
pub(crate) fn starting_dir(wanted: Option<&Path>, home: &Path) -> PathBuf {
    let mut dir = wanted.map(Path::to_path_buf);
    while let Some(candidate) = dir {
        if candidate.is_dir() {
            return candidate;
        }
        dir = candidate.parent().map(Path::to_path_buf);
    }
    home.to_path_buf()
}

/// The folders used last, newest first, at most [`RECENT_MAX`].
pub(crate) const RECENT_MAX: usize = 8;

pub(crate) fn remember(recent: &mut Vec<PathBuf>, dir: &Path) {
    recent.retain(|p| p != dir);
    recent.insert(0, dir.to_path_buf());
    recent.truncate(RECENT_MAX);
}

/// The recent folders, from `file` (a JSON array of paths); those gone from
/// the disk are left out.
pub(crate) fn load_recent(file: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<PathBuf>>(&text).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.is_dir())
        .take(RECENT_MAX)
        .collect()
}

pub(crate) fn save_recent(file: &Path, recent: &[PathBuf]) {
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(recent) {
        // A lost list of recent folders costs one click; not worth an error.
        let _ = std::fs::write(file, json);
    }
}

/// A size for the list: "812 KB", "4.2 MB".
pub(crate) fn size_label(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K * K {
        format!("{:.1} GB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.1} MB", b / (K * K))
    } else if b >= K {
        format!("{:.0} KB", b / K)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder under the build directory; `/tmp` is not ours to write.
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/file-browser")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    #[test]
    fn a_folder_lists_folders_first_and_only_the_files_asked_for() {
        let dir = scratch("listing");
        std::fs::create_dir(dir.join("b-folder")).unwrap();
        std::fs::create_dir(dir.join("A-folder")).unwrap();
        std::fs::create_dir(dir.join(".hidden-folder")).unwrap();
        for name in [
            "clip.MP4",
            "song.mp3",
            "notes.txt",
            "cut.chukcut",
            ".secret.mov",
        ] {
            touch(&dir, name);
        }
        let names = |mode: &Mode, filter, hidden| -> Vec<String> {
            list(&dir, mode, filter, hidden)
                .unwrap()
                .into_iter()
                .map(|e| e.name)
                .collect()
        };
        let open = Mode::Open { multiple: true };
        assert_eq!(
            names(&open, Filter::Media, false),
            ["A-folder", "b-folder", "clip.MP4", "song.mp3"]
        );
        assert_eq!(
            names(&open, Filter::Projects, false),
            ["A-folder", "b-folder", "cut.chukcut"]
        );
        assert_eq!(
            names(&Mode::Folder, Filter::Any, false),
            ["A-folder", "b-folder"]
        );
        assert_eq!(
            names(&open, Filter::Media, true),
            [
                ".hidden-folder",
                "A-folder",
                "b-folder",
                ".secret.mov",
                "clip.MP4",
                "song.mp3"
            ]
        );
        assert_eq!(names(&open, Filter::Any, false).len(), 6);
    }

    #[test]
    fn typed_paths_resolve_like_a_shell_would() {
        let home = Path::new("/home/me");
        let here = Path::new("/home/me/Videos");
        assert_eq!(resolve_typed("~", here, home), home);
        assert_eq!(resolve_typed("~/Music", here, home), home.join("Music"));
        assert_eq!(
            resolve_typed("raw/a.mp4", here, home),
            here.join("raw/a.mp4")
        );
        assert_eq!(
            resolve_typed("../Music/./x", here, home),
            home.join("Music/x")
        );
        assert_eq!(resolve_typed("/etc/../srv ", here, home), Path::new("/srv"));
        assert_eq!(resolve_typed("/..", here, home), Path::new("/"));
    }

    #[test]
    fn a_save_name_gets_its_extension_once() {
        let dir = Path::new("/p");
        assert_eq!(
            save_target(dir, "Cut", Filter::Projects),
            Some(dir.join("Cut.chukcut"))
        );
        assert_eq!(
            save_target(dir, "Cut.chukcut", Filter::Projects),
            Some(dir.join("Cut.chukcut"))
        );
        assert_eq!(
            save_target(dir, "subs.vtt", Filter::Subtitles),
            Some(dir.join("subs.vtt"))
        );
        assert_eq!(
            save_target(dir, "subs", Filter::Subtitles),
            Some(dir.join("subs.srt"))
        );
        assert_eq!(save_target(dir, "  ", Filter::Png), None);
        assert_eq!(save_target(dir, "a/b.png", Filter::Png), None);
        assert_eq!(save_target(dir, "x", Filter::Any), Some(dir.join("x")));
    }

    #[test]
    fn places_read_the_localised_user_folders() {
        let home = scratch("home");
        std::fs::create_dir(home.join("Filme")).unwrap();
        std::fs::create_dir(home.join("Music")).unwrap();
        let config = home.join(".config");
        std::fs::create_dir(&config).unwrap();
        std::fs::write(
            config.join("user-dirs.dirs"),
            "# comment\nXDG_VIDEOS_DIR=\"$HOME/Filme\"\nXDG_DESKTOP_DIR=\"$HOME/\"\n",
        )
        .unwrap();
        let labels: Vec<String> = places(&home, &config)
            .into_iter()
            .map(|p| p.label)
            .collect();
        // Desktop is home itself here and is not repeated; Music has no entry
        // in the file and is found by its English name.
        assert_eq!(labels, ["Home", "Filme", "Music", "Computer"]);
    }

    #[test]
    fn recent_folders_are_newest_first_and_bounded() {
        let mut recent = Vec::new();
        for i in 0..10 {
            remember(&mut recent, Path::new(&format!("/d{i}")));
        }
        remember(&mut recent, Path::new("/d5"));
        assert_eq!(recent.len(), RECENT_MAX);
        assert_eq!(recent[0], Path::new("/d5"));
        assert_eq!(recent[1], Path::new("/d9"));
        assert_eq!(recent.iter().filter(|p| *p == Path::new("/d5")).count(), 1);
    }

    #[test]
    fn the_browser_opens_in_the_nearest_folder_that_exists() {
        let dir = scratch("start");
        let home = Path::new("/");
        assert_eq!(starting_dir(Some(&dir.join("gone/deeper")), home), dir);
        assert_eq!(starting_dir(None, home), home);
    }
}
