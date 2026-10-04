//! Where baked mattes live, and how a source time finds its frame.
//!
//! One directory per (media file, model, version, size):
//!
//! ```text
//! <cache>/chukcut/mattes/<media digest>-<model>-<version>-<long side>/
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
//! The cache is derived data: deleting it loses nothing the document cannot
//! rebuild (decision 0019, `docs/research/ml-features.md` §5.6).

use std::path::{Path, PathBuf};

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

/// The directory of `media`'s mattes made by `model` at `version`. Reads
/// the file's head and tail for its key, so callers cache the answer.
pub fn dir_for(media: &Path, model: &str, version: &str) -> Result<PathBuf, String> {
    let key = SourceKey::of(media).map_err(|e| format!("cannot read {}: {e}", media.display()))?;
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    Ok(root().join(format!(
        "{}-{}-{}-{}",
        key.digest(),
        safe(model),
        safe(version),
        MATTE_LONG_SIDE
    )))
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
