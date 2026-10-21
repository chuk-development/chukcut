// Adapted from FilmCraft (https://github.com/storytold/filmcraft),
// crates/engine/src/relink.rs: the folder index, the prefix remap and the
// idea of checking a candidate before it replaces the original.
// Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
// Licensed under the Apache License, Version 2.0 or the MIT License.
// Changes for chukcut (its document, the probe-based check, the undo step)
// are under the GPL-3.0 like the rest of this repository. See NOTICE.md.

//! Finding media that moved: the offline state of decision 0009 gets its
//! way back.
//!
//! - **What is missing**: every video, sound and picture in the pool whose
//!   file is not on disk ([`missing`]).
//! - **Is this the same file?** A candidate is probed and compared with what
//!   the pool remembers of the original: picture or not, sound or not, size
//!   in pixels, length ([`check`]). A different take with the same name is
//!   refused unless forced, so it cannot silently replace the original.
//! - **The rest follows**: relinking one file derives a folder remap (the old
//!   path's prefix → the new one, [`derive_remap`]) and tries it on every
//!   other missing file, so moving a project folder to another disk is one
//!   relink.
//! - **Search**: a folder is walked for files named like the missing ones
//!   ([`index_folder`]).
//!
//! The relink itself is an ordinary document edit: each material is removed
//! and added back at the same place with its new path, one undo step for
//! all of them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::modules::project::{MaterialPool, Micros};
use crate::modules::timeline::ops::{EditCommand, PoolMaterial};

/// A file of the pool that is not on disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Missing {
    pub material_id: String,
    pub path: String,
    /// `video`, `audio` or `image`.
    pub kind: &'static str,
}

/// Every file-backed material whose file is gone, in pool order. Template
/// placeholders (generated into the cache) are included like anything else:
/// a missing cache file is regenerated elsewhere, not relinked, so callers
/// that show this to a user filter them.
pub fn missing(pool: &MaterialPool) -> Vec<Missing> {
    let gone = |p: &str| !Path::new(p).exists();
    let mut out = Vec::new();
    for m in pool.videos.iter().filter(|m| gone(&m.path)) {
        out.push(Missing {
            material_id: m.id.clone(),
            path: m.path.clone(),
            kind: "video",
        });
    }
    for m in pool.audios.iter().filter(|m| gone(&m.path)) {
        out.push(Missing {
            material_id: m.id.clone(),
            path: m.path.clone(),
            kind: "audio",
        });
    }
    for m in pool.images.iter().filter(|m| gone(&m.path)) {
        out.push(Missing {
            material_id: m.id.clone(),
            path: m.path.clone(),
            kind: "image",
        });
    }
    out
}

/// What the pool knows about a material, to compare a candidate with.
#[derive(Debug, Clone, PartialEq)]
pub struct Expected {
    pub path: String,
    pub has_video: bool,
    pub has_audio: bool,
    /// Picture size; either orientation is accepted, as the pool stores what
    /// the import saw and a rotation flag can swap them.
    pub size: Option<(u32, u32)>,
    pub duration: Option<Micros>,
}

/// The material `id` as a pool entry, and what a replacement must match.
pub fn expected(pool: &MaterialPool, id: &str) -> Option<(PoolMaterial, usize, Expected)> {
    if let Some(i) = pool.videos.iter().position(|m| m.id == id) {
        let m = &pool.videos[i];
        return Some((
            PoolMaterial::Video(m.clone()),
            i,
            Expected {
                path: m.path.clone(),
                has_video: true,
                has_audio: m.has_audio,
                size: Some((m.width, m.height)),
                duration: Some(m.duration),
            },
        ));
    }
    if let Some(i) = pool.audios.iter().position(|m| m.id == id) {
        let m = &pool.audios[i];
        return Some((
            PoolMaterial::Audio(m.clone()),
            i,
            Expected {
                path: m.path.clone(),
                has_video: false,
                has_audio: true,
                size: None,
                duration: Some(m.duration),
            },
        ));
    }
    if let Some(i) = pool.images.iter().position(|m| m.id == id) {
        let m = &pool.images[i];
        return Some((
            PoolMaterial::Image(m.clone()),
            i,
            Expected {
                path: m.path.clone(),
                has_video: true,
                has_audio: false,
                size: Some((m.width, m.height)),
                duration: None,
            },
        ));
    }
    None
}

/// What a probe of a candidate found, in the terms [`check`] compares.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub has_video: bool,
    pub has_audio: bool,
    pub size: Option<(u32, u32)>,
    pub duration: Micros,
}

impl Found {
    pub fn probe(path: &Path) -> Result<Found, String> {
        let info = super::probe(path).map_err(|e| e.to_string())?;
        Ok(Found {
            has_video: info.has_video,
            has_audio: info.has_audio,
            size: info.video.as_ref().map(|v| (v.width, v.height)),
            duration: info.duration,
        })
    }
}

/// Lengths may differ by this much and still be the same recording: a
/// remux or a copy through another tool can move the container's duration
/// by a frame or two. Half a second, or 1 %, whichever is larger.
fn duration_slack(d: Micros) -> Micros {
    (d / 100).max(500_000)
}

/// Why `found` is not the file `want` described, in sentences; empty when it
/// matches.
pub fn check(want: &Expected, found: &Found) -> Vec<String> {
    let mut problems = Vec::new();
    if want.has_video && !found.has_video {
        problems.push("it has no picture".to_string());
    }
    if want.has_audio && !found.has_audio {
        problems.push("it has no sound".to_string());
    }
    if let (Some((w, h)), Some((fw, fh))) = (want.size, found.size) {
        if !((w, h) == (fw, fh) || (w, h) == (fh, fw)) {
            problems.push(format!("it is {fw}×{fh}, the original was {w}×{h}"));
        }
    }
    if let Some(d) = want.duration.filter(|d| *d > 0) {
        if found.duration > 0 && (found.duration - d).abs() > duration_slack(d) {
            problems.push(format!(
                "it is {:.1} s long, the original was {:.1} s",
                found.duration as f64 / 1e6,
                d as f64 / 1e6
            ));
        }
    }
    problems
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The folder move that turns `old` into `new`: the longest common tail of
/// path components is dropped from both, the heads are the remap.
/// `/a/proj/clip.mp4` → `/b/proj/clip.mp4` gives `(/a, /b)`. `None` when the
/// file names differ or nothing changes.
pub fn derive_remap(old: &str, new: &str) -> Option<(String, String)> {
    let oc: Vec<&str> = old.split('/').collect();
    let nc: Vec<&str> = new.split('/').collect();
    let mut k = 0;
    while k < oc.len().min(nc.len()) && oc[oc.len() - 1 - k] == nc[nc.len() - 1 - k] {
        k += 1;
    }
    if k == 0 {
        return None;
    }
    let from = oc[..oc.len() - k].join("/");
    let to = nc[..nc.len() - k].join("/");
    (from != to).then_some((from, to))
}

/// `path` under the remap, or `None` when it does not start with `from`.
pub fn apply_remap(path: &str, from: &str, to: &str) -> Option<String> {
    let rest = path.strip_prefix(from.trim_end_matches('/'))?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    Some(format!("{}{rest}", to.trim_end_matches('/')))
}

/// The files under `root`, by lower-case name, breadth first, hidden folders
/// skipped, at most `limit` files so a search of `/` ends.
pub fn index_folder(root: &Path, limit: usize) -> BTreeMap<String, Vec<PathBuf>> {
    let mut out: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut n = 0;
    while let Some(dir) = queue.pop_front() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = read.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(kind) = e.file_type() else {
                continue;
            };
            if kind.is_dir() {
                queue.push_back(e.path());
            } else {
                out.entry(name.to_ascii_lowercase())
                    .or_default()
                    .push(e.path());
                n += 1;
                if n >= limit {
                    return out;
                }
            }
        }
    }
    out
}

/// The edit that points material `old` at `path`: removed and added back at
/// its place, so undo restores the old path byte for byte.
pub fn relink_edit(old: PoolMaterial, index: usize, path: &str) -> Vec<EditCommand> {
    let mut new = old.clone();
    match &mut new {
        PoolMaterial::Video(m) => m.path = path.to_string(),
        PoolMaterial::Audio(m) => m.path = path.to_string(),
        PoolMaterial::Image(m) => m.path = path.to_string(),
        PoolMaterial::Text(_) => return Vec::new(),
    }
    vec![
        EditCommand::RemoveMaterial {
            material: old,
            index,
        },
        EditCommand::AddMaterial {
            material: new,
            index,
        },
    ]
}

/// The candidates for `want` in an index: same name, case-insensitive.
pub fn candidates<'a>(index: &'a BTreeMap<String, Vec<PathBuf>>, want: &str) -> &'a [PathBuf] {
    index
        .get(&file_name(want).to_ascii_lowercase())
        .map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want() -> Expected {
        Expected {
            path: "/old/a.mp4".into(),
            has_video: true,
            has_audio: true,
            size: Some((1920, 1080)),
            duration: Some(10_000_000),
        }
    }

    #[test]
    fn a_remap_is_the_part_of_the_path_that_moved() {
        assert_eq!(
            derive_remap("/mnt/laptop/proj/media/a.mp4", "/home/me/proj/media/a.mp4"),
            Some(("/mnt/laptop".into(), "/home/me".into()))
        );
        assert_eq!(derive_remap("/x/a.mp4", "/x/a.mp4"), None);
        assert_eq!(derive_remap("/x/a.mp4", "/y/b.mp4"), None);
        assert_eq!(
            apply_remap("/mnt/laptop/proj/b.wav", "/mnt/laptop", "/home/me"),
            Some("/home/me/proj/b.wav".into())
        );
        // A prefix of a folder name is not the folder.
        assert_eq!(
            apply_remap("/mnt/laptop2/b.wav", "/mnt/laptop", "/home/me"),
            None
        );
    }

    #[test]
    fn the_same_file_passes_and_another_take_is_refused_in_words() {
        let same = Found {
            has_video: true,
            has_audio: true,
            size: Some((1920, 1080)),
            duration: 10_020_000,
        };
        assert!(check(&want(), &same).is_empty());
        // A portrait flag can swap the stored size.
        let turned = Found {
            size: Some((1080, 1920)),
            ..same.clone()
        };
        assert!(check(&want(), &turned).is_empty());
        let other = Found {
            size: Some((3840, 2160)),
            duration: 31_000_000,
            ..same.clone()
        };
        let problems = check(&want(), &other);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("3840×2160"));
        assert!(problems[1].contains("31.0 s"));
        let wav = Found {
            has_video: false,
            has_audio: true,
            size: None,
            duration: 10_000_000,
        };
        assert!(check(&want(), &wav)[0].contains("no picture"));
    }

    #[test]
    fn the_index_finds_files_by_name_in_any_case_and_skips_hidden_folders() {
        let root = std::env::temp_dir().join(format!("chukcut-relink-{}", std::process::id()));
        std::fs::create_dir_all(root.join("deep/er")).unwrap();
        std::fs::create_dir_all(root.join(".trash")).unwrap();
        std::fs::write(root.join("deep/er/Clip.MP4"), b"x").unwrap();
        std::fs::write(root.join(".trash/clip.mp4"), b"x").unwrap();
        let index = index_folder(&root, 1_000);
        let found = candidates(&index, "/gone/clip.mp4");
        assert_eq!(found, [root.join("deep/er/Clip.MP4")]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
