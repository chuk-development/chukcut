//! Where baked mattes live, and how a source time finds its frame.
//!
//! One directory per (media file, model, version, prompt, provider, size):
//!
//! ```text
//! <cache>/chukcut/mattes/<media digest>-<model>-<version>[-p<prompt>]-<provider>-<long side>/
//!     000000000000.png      the frame shown from 0 µs
//!     000000033367.png      the frame shown from 33 367 µs
//!     …
//! ```
//!
//! Each file is an 8-bit greyscale PNG, the matte of the source frame whose
//! presentation time (µs) is its name, in display orientation (the decoder
//! has applied rotation), at the bake's analysis size. A PNG per frame
//! rather than one matte video: frames are written as the bake reaches them,
//! so the preview shows the part already baked; a frame is read without
//! seeking a decoder; a cancelled or crashed bake leaves only whole frames
//! behind (each is written to a `.part` file and renamed); and a re-bake of
//! some frames touches only those files.
//!
//! The media digest is the proxy cache's `SourceKey` (path, size, mtime and
//! the head and tail bytes), so a re-encoded file under the same name gets
//! new mattes and a matte is never applied to footage it was not made from.
//!
//! **The provider is part of the key.** The same model on CUDA and on the
//! CPU differs by about 1/255 (RVM's recurrent state carries the rounding),
//! which is invisible in one frame and visible as a flicker where a clip
//! changes from one to the other. So each provider bakes into its own
//! directory, the bake fills the directory of the provider it runs on, and
//! the compositor draws a clip from one directory — the one with the most
//! frames — never a mix. Directories from before this rule (no provider in
//! the name) are read as one more provider.
//!
//! **The prompt is part of the key** for "Select object": a hash of the
//! clicked points and their frame, so another selection never shows the
//! last one's mattes.
//!
//! The cache is derived data: deleting it loses nothing the document cannot
//! rebuild (decision 0019, `docs/research/ml-features.md` §5.6). It counts
//! towards the cache limit; the open project's mattes are kept by a trim
//! (`workspace::trim`), as its thumbnails are.

use std::path::{Path, PathBuf};

use crate::modules::project::compositing::{BackgroundRemoval, ObjectPrompt};
use crate::modules::project::document::{Micros, SAMPLE_SLACK};
use crate::modules::proxy::cache::SourceKey;

/// The long side, in pixels, mattes are made and stored at. RVM's encoder
/// sees about 512 px whatever it is given (`rvm::downsample_ratio`); its
/// guided filter brings edges back up to this size. 960 keeps hair at
/// 1080p sharper than 640 did and costs half of 1280.
pub const MATTE_LONG_SIDE: u32 = 960;

/// The root of all mattes.
pub fn root() -> PathBuf {
    crate::modules::workspace::paths::cache_root().join("mattes")
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            // `-` stays: the directories of the first build were named with
            // it (`…-rvm-1.0.0-mobilenetv3-960`) and must still be found.
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A short, stable hash of a prompt: its frame and its points, rounded to
/// a ten-thousandth of the frame so a value that went through JSON and back
/// keys the same.
pub fn prompt_hash(prompt: &ObjectPrompt) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(prompt.time.to_le_bytes());
    for p in &prompt.points {
        let q = |v: f32| (v * 10_000.0).round() as i32;
        hasher.update(q(p.x).to_le_bytes());
        hasher.update(q(p.y).to_le_bytes());
        hasher.update([p.keep as u8]);
    }
    format!("{:x}", hasher.finalize())[..12].to_string()
}

/// The part of a matte directory's name that says what made it, without the
/// provider and the size: `<digest>-<model>-<version>[-p<prompt>]`. Reads
/// the media file's head and tail for its digest, so callers keep the
/// answer.
pub fn key_for(media: &Path, setting: &BackgroundRemoval) -> Result<String, String> {
    let key = SourceKey::of(media).map_err(|e| format!("cannot read {}: {e}", media.display()))?;
    let mut name = format!(
        "{}-{}-{}",
        key.digest(),
        safe(&setting.model),
        safe(&setting.version)
    );
    if let Some(prompt) = &setting.prompt {
        name.push_str(&format!("-p{}", prompt_hash(prompt)));
    }
    Ok(name)
}

/// The prefix of every matte directory of `media`, whatever made it: what a
/// trim protects for the open project.
pub fn media_prefix(media: &Path) -> Result<String, String> {
    let key = SourceKey::of(media).map_err(|e| format!("cannot read {}: {e}", media.display()))?;
    Ok(format!("{}-", key.digest()))
}

/// A provider as a directory-name token: `CUDA` → `cuda`.
pub fn provider_token(provider: &str) -> String {
    let token = provider.to_ascii_lowercase();
    if PROVIDERS.contains(&token.as_str()) {
        token
    } else {
        "unknown".into()
    }
}

/// Provider tokens a directory name may carry ([`provider_token`] of the
/// worker's provider names).
const PROVIDERS: [&str; 7] = [
    "cpu", "cuda", "openvino", "tensorrt", "migraphx", "webgpu", "unknown",
];

/// The directory `provider` bakes `key`'s mattes into.
pub fn dir_in(key: &str, provider: &str) -> PathBuf {
    root().join(format!(
        "{key}-{}-{MATTE_LONG_SIDE}",
        provider_token(provider)
    ))
}

/// Every directory holding mattes of `key`, with its provider token
/// (`legacy` for one from before providers were part of the name).
pub fn dirs_of(key: &str) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root()) else {
        return Vec::new();
    };
    let size = MATTE_LONG_SIDE.to_string();
    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let rest = name.strip_prefix(key)?.strip_prefix('-')?;
            let parts: Vec<&str> = rest.split('-').collect();
            let provider = match parts.as_slice() {
                // Only known tokens: a version with a dash in it must not
                // read as another version's provider.
                [provider, s] if *s == size && PROVIDERS.contains(provider) => provider.to_string(),
                [s] if *s == size => "legacy".to_string(),
                _ => return None,
            };
            Some((provider, e.path()))
        })
        .collect();
    found.sort();
    found
}

/// The directory to draw `key` from and its frames: the one with the most
/// frames, so a clip shows one provider's mattes, never a mix.
pub fn best(key: &str) -> Option<(PathBuf, Vec<Micros>)> {
    dirs_of(key)
        .into_iter()
        .map(|(_, dir)| {
            let times = list(&dir);
            (dir, times)
        })
        .filter(|(_, times)| !times.is_empty())
        .max_by_key(|(_, times)| times.len())
}

/// The file of the frame shown from `pts`.
pub fn frame_path(dir: &Path, pts: Micros) -> PathBuf {
    dir.join(format!("{:012}.png", pts.max(0)))
}

/// The presentation times of the mattes present in `dir`, sorted. Empty
/// when the directory does not exist.
pub fn list(dir: &Path) -> Vec<Micros> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut times: Vec<Micros> = entries
        .filter_map(|e| {
            let name = e.ok()?.file_name();
            let stem = name.to_str()?.strip_suffix(".png")?;
            stem.parse().ok()
        })
        .collect();
    times.sort_unstable();
    times
}

/// The matte for `source_time` among sorted `times`: the last frame shown at
/// or before it (with the same slack the decoder uses, so a time that is a
/// rounded frame time finds that frame and not the one before). `None` when
/// that frame is missing: either nothing is baked before `source_time`, or
/// the newest matte before it is a whole `period` or more older — the frame
/// shown then has no matte of its own (a hole in the bake), and drawing an
/// older one would cut out the wrong shape.
pub fn lookup(times: &[Micros], source_time: Micros, period: Micros) -> Option<Micros> {
    let at = source_time + SAMPLE_SLACK;
    let index = times.partition_point(|&t| t <= at);
    let found = *times.get(index.checked_sub(1)?)?;
    (at - found < period).then_some(found)
}

/// Write one matte (`width × height` bytes) atomically.
pub fn write(
    dir: &Path,
    pts: Micros,
    width: u32,
    height: u32,
    alpha: Vec<u8>,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let image = image::GrayImage::from_raw(width, height, alpha)
        .ok_or_else(|| format!("a {width}x{height} matte has the wrong number of bytes"))?;
    let path = frame_path(dir, pts);
    let part = path.with_extension("png.part");
    image
        .save_with_format(&part, image::ImageFormat::Png)
        .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    std::fs::rename(&part, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Read one matte: width, height, one byte per pixel.
pub fn read(dir: &Path, pts: Micros) -> Result<(u32, u32, Vec<u8>), String> {
    let path = frame_path(dir, pts);
    let image = image::open(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .into_luma8();
    Ok((image.width(), image.height(), image.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_finds_the_frame_shown_then_and_not_across_a_hole() {
        let period = 33_333;
        let times = [0, 33_333, 66_667, 200_000];
        assert_eq!(lookup(&times, 0, period), Some(0));
        assert_eq!(lookup(&times, 33_332, period), Some(33_333));
        assert_eq!(lookup(&times, 50_000, period), Some(33_333));
        assert_eq!(lookup(&times, 66_666, period), Some(66_667));
        assert_eq!(lookup(&times, 99_900, period), Some(66_667));
        // The frame from 100 ms has no matte: the newest is a frame older.
        assert_eq!(lookup(&times, 110_000, period), None);
        assert_eq!(lookup(&times, 133_000, period), None);
        assert_eq!(lookup(&times, 210_000, period), Some(200_000));
        assert_eq!(lookup(&[], 10, period), None);
        assert_eq!(lookup(&[50_000], 10, period), None);
    }

    #[test]
    fn the_prompt_and_the_provider_are_part_of_the_key() {
        use crate::modules::project::compositing::PromptPoint;
        let prompt = |x: f32| ObjectPrompt {
            time: 1_000,
            points: vec![PromptPoint {
                x,
                y: 0.5,
                keep: true,
            }],
        };
        assert_eq!(prompt_hash(&prompt(0.25)), prompt_hash(&prompt(0.250_001)));
        assert_ne!(prompt_hash(&prompt(0.25)), prompt_hash(&prompt(0.26)));
        assert_eq!(provider_token("CUDA"), "cuda");
        assert_eq!(provider_token("OpenVINO"), "openvino");
        assert_ne!(dir_in("k", "CUDA"), dir_in("k", "CPU"));
    }

    #[test]
    fn a_matte_survives_the_round_trip_and_is_listed_in_order() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/matte-cache-round-trip");
        let _ = std::fs::remove_dir_all(&dir);
        let alpha: Vec<u8> = (0..12).map(|i| i * 20).collect();
        write(&dir, 66_667, 4, 3, alpha.clone()).unwrap();
        write(&dir, 0, 4, 3, vec![255; 12]).unwrap();
        assert_eq!(list(&dir), vec![0, 66_667]);
        assert_eq!(read(&dir, 66_667).unwrap(), (4, 3, alpha));
        assert!(write(&dir, 5, 4, 3, vec![0; 5]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
