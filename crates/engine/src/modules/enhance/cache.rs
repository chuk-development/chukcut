//! Where remade frames live, and how a source time finds its frame.
//!
//! ```text
//! <cache>/chukcut/enhance/<media digest>-<ops>-<signature>-<provider>-<long side>/
//!     000000000000.jpg      the frame shown from 0 µs
//!     000000033367.jpg      the frame shown from 33 367 µs
//!     …
//! ```
//!
//! One JPEG (quality 95, 4:4:4) per source frame, named by its presentation
//! time like a matte (`matting::cache`), in display orientation, at the
//! chain's output size. `<ops>` says what was done (`remove`, `x2`,
//! `remove_x4`) for a human reading the directory; the signature
//! (`Chain::signature`) is what keys it. The provider is part of the key for
//! the reason it is for mattes: CUDA and CPU answers differ in the last
//! bits, and a clip draws from one directory, the one with the most frames.
//!
//! JPEG rather than PNG for the reason optical-flow frames are JPEG: a 4K
//! PNG is ~20 MB and slow to decode on the render thread. A preview that
//! needs a quarter of the pixels decodes at a quarter of the size, which
//! libjpeg-turbo does in the DCT ([`read`]).

use std::path::{Path, PathBuf};

use super::Chain;
use crate::modules::project::document::Micros;
use crate::modules::proxy::cache::SourceKey;

/// JPEG quality of a stored frame.
const QUALITY: i32 = 95;

/// The root of all remade frames.
pub fn root() -> PathBuf {
    crate::modules::workspace::paths::cache_root().join("enhance")
}

/// The part of a directory name that says what made the frames, without
/// the provider and the size. Reads the media file's head and tail; callers
/// keep the answer.
pub fn key_for(media: &Path, chain: &Chain) -> Result<String, String> {
    let key = SourceKey::of(media).map_err(|e| format!("cannot read {}: {e}", media.display()))?;
    Ok(format!(
        "{}-{}-{}",
        key.digest(),
        chain.ops(),
        chain.signature()
    ))
}

/// The directory `provider` bakes `key`'s frames into, at `long` pixels.
pub fn dir_in(key: &str, provider: &str, long: u32) -> PathBuf {
    root().join(format!(
        "{key}-{}-{long}",
        crate::modules::matting::cache::provider_token(provider)
    ))
}

/// Every directory holding frames of `key`, whatever provider or size.
pub fn dirs_of(key: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root()) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(key)) else {
                return false;
            };
            matches!(rest.split('-').collect::<Vec<_>>().as_slice(),
                ["", provider, size] if !provider.is_empty() && size.parse::<u32>().is_ok())
        })
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

/// The presentation times of the frames in `dir`, sorted. Empty when it
/// does not exist.
pub fn list(dir: &Path) -> Vec<Micros> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut times: Vec<Micros> = entries
        .filter_map(|e| {
            let name = e.ok()?.file_name();
            name.to_str()?.strip_suffix(".jpg")?.parse().ok()
        })
        .collect();
    times.sort_unstable();
    times
}

/// The directory to draw `key` from and its frames: the one with the most.
pub fn best(key: &str) -> Option<(PathBuf, Vec<Micros>)> {
    dirs_of(key)
        .into_iter()
        .map(|dir| {
            let times = list(&dir);
            (dir, times)
        })
        .filter(|(_, times)| !times.is_empty())
        .max_by_key(|(_, times)| times.len())
}

/// The file of the frame shown from `pts`.
pub fn frame_path(dir: &Path, pts: Micros) -> PathBuf {
    dir.join(format!("{:012}.jpg", pts.max(0)))
}

/// Encode RGBA8 as the cache's JPEG.
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let image = turbojpeg::Image {
        pixels: rgba,
        width: width as usize,
        pitch: width as usize * 4,
        height: height as usize,
        format: turbojpeg::PixelFormat::RGBA,
    };
    let mut compressor = turbojpeg::Compressor::new().map_err(|e| format!("libjpeg-turbo: {e}"))?;
    compressor
        .set_quality(QUALITY)
        .and_then(|_| compressor.set_subsamp(turbojpeg::Subsamp::None))
        .map_err(|e| format!("libjpeg-turbo: {e}"))?;
    compressor
        .compress_to_vec(image)
        .map_err(|e| format!("libjpeg-turbo: {e}"))
}

/// Write one frame (RGBA8, `width × height`) atomically.
pub fn write(dir: &Path, pts: Micros, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let bytes = encode(width, height, rgba)?;
    let path = frame_path(dir, pts);
    let part = path.with_extension("jpg.part");
    std::fs::write(&part, bytes).map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    std::fs::rename(&part, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Read one frame as RGBA8, decoded at the smallest of full, half, quarter
/// or eighth size that still covers `want` (width, height) on both sides;
/// `(0, 0)` reads it whole. Answers width, height and pixels.
pub fn read(dir: &Path, pts: Micros, want: (u32, u32)) -> Result<(u32, u32, Vec<u8>), String> {
    let path = frame_path(dir, pts);
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut decompressor =
        turbojpeg::Decompressor::new().map_err(|e| format!("libjpeg-turbo: {e}"))?;
    let header = decompressor
        .read_header(&bytes)
        .map_err(|e| format!("cannot decode {}: {e}", path.display()))?;
    let scale = [8usize, 4, 2]
        .into_iter()
        .find(|&d| {
            want.0 > 0
                && want.1 > 0
                && header.width.div_ceil(d) >= want.0 as usize
                && header.height.div_ceil(d) >= want.1 as usize
        })
        .unwrap_or(1);
    let factor = turbojpeg::ScalingFactor::new(1, scale);
    decompressor
        .set_scaling_factor(factor)
        .map_err(|e| format!("libjpeg-turbo: {e}"))?;
    let (width, height) = (factor.scale(header.width), factor.scale(header.height));
    let mut image = turbojpeg::Image {
        pixels: vec![0u8; width * height * 4],
        width,
        pitch: width * 4,
        height,
        format: turbojpeg::PixelFormat::RGBA,
    };
    decompressor
        .decompress(&bytes, image.as_deref_mut())
        .map_err(|e| format!("cannot decode {}: {e}", path.display()))?;
    Ok((width as u32, height as u32, image.pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_survives_the_round_trip_and_reads_smaller_when_asked() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/enhance-cache-round-trip");
        let _ = std::fs::remove_dir_all(&dir);
        let (w, h) = (64u32, 32u32);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % w * 4) as u8, (i / w * 8) as u8, 128, 255])
            .collect();
        write(&dir, 66_667, w, h, &rgba).unwrap();
        write(&dir, 0, w, h, &rgba).unwrap();
        assert_eq!(list(&dir), vec![0, 66_667]);
        let (rw, rh, back) = read(&dir, 66_667, (0, 0)).unwrap();
        assert_eq!((rw, rh), (w, h));
        let worst = rgba
            .iter()
            .zip(&back)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap();
        assert!(worst <= 6, "quality 95 4:4:4 is close: worst {worst}");
        assert_eq!(read(&dir, 0, (16, 8)).unwrap().0, 16, "a quarter");
        assert_eq!(read(&dir, 0, (17, 8)).unwrap().0, 32, "a half covers 17");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
