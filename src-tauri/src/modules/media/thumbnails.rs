//! Thumbnail strips for the timeline and the media library.
//!
//! A strip is N evenly spaced frames of one file, scaled to a fixed height and
//! written to disk as JPEG. They go to disk rather than being returned inline
//! because a lane's worth of thumbnails is megabytes of pixels, and shipping
//! that through `invoke()` means base64 in a JSON string — the webview can load
//! a file far more cheaply than it can parse one.
//!
//! Generation is idempotent: the cache path is derived from the file's
//! identity and the requested height, so a second request for a strip that
//! already exists costs a `stat` per frame and no decoding at all.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use super::decoder::VideoDecoder;
use super::probe::probe_video;
use super::{MediaError, Result};
use crate::modules::project::Micros;

/// JPEG quality for thumbnails. High enough that timeline frames do not show
/// blocking on flat gradients, low enough that a 200-frame strip stays small.
const JPEG_QUALITY: u8 = 78;

/// Generate `count` evenly spaced thumbnails of `path` at `height` pixels tall.
///
/// Returns absolute paths in timeline order. Frames that are already cached are
/// not decoded again.
pub fn thumbnail_strip(path: impl AsRef<Path>, count: usize, height: u32) -> Result<Vec<String>> {
    let path = path.as_ref();

    if count == 0 {
        return Err(MediaError::Invalid(
            "a thumbnail strip needs at least one frame".into(),
        ));
    }
    if height == 0 {
        return Err(MediaError::Invalid(
            "thumbnail height must be greater than zero".into(),
        ));
    }

    let (info, _video) = probe_video(path)?;

    let directory = cache_directory(path);
    std::fs::create_dir_all(&directory).map_err(|source| MediaError::Write {
        path: directory.clone(),
        source,
    })?;

    let times = thumbnail_times(info.duration, count);
    let jobs: Vec<Job> = times
        .into_iter()
        .enumerate()
        .map(|(index, at)| Job {
            at,
            output: directory.join(frame_file_name(index, height)),
        })
        .filter(|job| !job.output.exists())
        .collect();

    // One decoder per worker, never one shared: a decoder owns a demuxer with a
    // single read position, so two threads asking it for different timestamps
    // would seek each other's reads out from under them. Chunking rather than
    // per-frame parallelism keeps the number of file opens down to the number
    // of threads.
    if !jobs.is_empty() {
        let workers = rayon::current_num_threads().clamp(1, jobs.len());
        let chunk = jobs.len().div_ceil(workers);
        jobs.par_chunks(chunk)
            .try_for_each(|chunk| render_chunk(path, height, chunk))?;
    }

    Ok((0..count)
        .map(|index| {
            directory
                .join(frame_file_name(index, height))
                .to_string_lossy()
                .into_owned()
        })
        .collect())
}

struct Job {
    at: Micros,
    output: PathBuf,
}

fn render_chunk(path: &Path, height: u32, jobs: &[Job]) -> Result<()> {
    let mut decoder = VideoDecoder::open_scaled(path, height)?;
    for job in jobs {
        // Times inside a chunk ascend, so the decoder's forward-decode path
        // does the work whenever two thumbnails fall inside one GOP.
        let frame = decoder.seek_and_decode(job.at)?;
        write_jpeg(&job.output, &frame.data, frame.width, frame.height)?;
    }
    Ok(())
}

fn write_jpeg(output: &Path, rgba: &[u8], width: u32, height: u32) -> Result<()> {
    let encoder = jpeg_encoder::Encoder::new_file(output, JPEG_QUALITY).map_err(|source| {
        MediaError::Write {
            path: output.to_path_buf(),
            source: encoding_io_error(source),
        }
    })?;
    encoder
        .encode(
            rgba,
            width as u16,
            height as u16,
            jpeg_encoder::ColorType::Rgba,
        )
        .map_err(|source| MediaError::Write {
            path: output.to_path_buf(),
            source: encoding_io_error(source),
        })
}

fn encoding_io_error(error: jpeg_encoder::EncodingError) -> std::io::Error {
    match error {
        jpeg_encoder::EncodingError::IoError(io) => io,
        other => std::io::Error::other(other.to_string()),
    }
}

/// The instants to sample for a strip of `count` thumbnails.
///
/// Samples sit at the *centre* of each slice rather than at its start. Sampling
/// at the start puts the first thumbnail on frame zero, which for a huge share
/// of real footage is a black fade-in, and puts the last one at the very end,
/// which is past the last frame of the file. Centres give every thumbnail a
/// frame that represents the span of timeline it sits under.
pub fn thumbnail_times(duration: Micros, count: usize) -> Vec<Micros> {
    if count == 0 {
        return Vec::new();
    }
    let duration = duration.max(0);
    (0..count)
        .map(|index| {
            if duration == 0 {
                return 0;
            }
            let numerator = duration as i128 * (2 * index as i128 + 1);
            (numerator / (2 * count as i128)) as Micros
        })
        .collect()
}

fn frame_file_name(index: usize, height: u32) -> String {
    format!("{index:05}_h{height}.jpg")
}

/// Where a file's thumbnails live.
///
/// TODO: the `workspace` module will own cache paths once it exists; this
/// should become `workspace::cache_dir().join("thumbs")` so the user can point
/// the cache at a different disk and so a "clear cache" action has one place to
/// look. The temp directory is a placeholder, not a decision.
fn cache_directory(path: &Path) -> PathBuf {
    std::env::temp_dir()
        .join("chukcut")
        .join("thumbs")
        .join(cache_key(path))
}

/// A stable, collision-resistant-enough directory name for one media file.
///
/// The key folds in size and modification time as well as the path, so
/// re-exporting a clip to the same name invalidates its thumbnails instead of
/// showing the old ones forever. A file we cannot stat still gets a key from
/// its path alone — a missing file is a relink problem, not a reason to fail
/// here.
fn cache_key(path: &Path) -> String {
    let mut hash = FNV_OFFSET;
    hash = fnv1a(hash, path.to_string_lossy().as_bytes());
    if let Ok(metadata) = std::fs::metadata(path) {
        hash = fnv1a(hash, &metadata.len().to_le_bytes());
        if let Ok(modified) = metadata.modified() {
            if let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) {
                hash = fnv1a(hash, &since_epoch.as_secs().to_le_bytes());
            }
        }
    }
    format!("{hash:016x}")
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_times_are_evenly_spaced_slice_centres() {
        let times = thumbnail_times(4_000_000, 4);
        assert_eq!(
            times,
            vec![500_000, 1_500_000, 2_500_000, 3_500_000],
            "four thumbnails of a four second clip sit at 0.5s intervals"
        );
    }

    #[test]
    fn thumbnail_times_never_hit_the_edges() {
        let duration = 10_000_000;
        let times = thumbnail_times(duration, 8);
        assert!(times.first().copied().unwrap() > 0, "first frame is not 0");
        assert!(
            times.last().copied().unwrap() < duration,
            "last frame is inside the file"
        );
    }

    #[test]
    fn a_single_thumbnail_is_taken_from_the_middle() {
        assert_eq!(thumbnail_times(10_000_000, 1), vec![5_000_000]);
    }

    #[test]
    fn thumbnail_times_ascend() {
        let times = thumbnail_times(7_777_777, 12);
        assert_eq!(times.len(), 12);
        assert!(times.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn thumbnail_times_of_an_unknown_duration_are_all_zero() {
        // A container with no duration still gets a strip; every frame is the
        // first one, which is better than refusing to show anything.
        assert_eq!(thumbnail_times(0, 3), vec![0, 0, 0]);
        assert_eq!(thumbnail_times(-1, 2), vec![0, 0]);
    }

    #[test]
    fn thumbnail_times_of_zero_count_is_empty() {
        assert!(thumbnail_times(1_000_000, 0).is_empty());
    }

    #[test]
    fn long_durations_do_not_overflow() {
        // Twenty-four hours in microseconds, times a large count, overflows an
        // i64 multiply if the arithmetic is not widened.
        let duration = 24 * 60 * 60 * 1_000_000;
        let times = thumbnail_times(duration, 4096);
        assert!(times.windows(2).all(|w| w[0] < w[1]));
        assert!(times.last().copied().unwrap() < duration);
    }

    #[test]
    fn cache_keys_differ_per_path_and_are_fixed_width() {
        let a = cache_key(Path::new("/media/a.mp4"));
        let b = cache_key(Path::new("/media/b.mp4"));
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert_eq!(a, cache_key(Path::new("/media/a.mp4")), "keys are stable");
    }

    #[test]
    fn frame_file_names_sort_in_timeline_order() {
        let mut names: Vec<String> = [12, 3, 100, 7]
            .into_iter()
            .map(|i| frame_file_name(i, 80))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                frame_file_name(3, 80),
                frame_file_name(7, 80),
                frame_file_name(12, 80),
                frame_file_name(100, 80),
            ]
        );
    }

    #[test]
    fn frame_file_names_are_per_height() {
        assert_ne!(frame_file_name(0, 80), frame_file_name(0, 160));
    }
}
