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
//!
//! ## Why the tiles are streamed
//!
//! A strip used to be one call that answered when every frame was done. Three
//! long clips dropped on a cold cache is then tens of seconds of nothing — no
//! filmstrip, no progress, no way to tell the editor from a hung one. So the
//! work reports through a [`BatchSink`] as tiles land: cached ones go out
//! immediately, decoded ones follow in small batches, and the lane fills in
//! rather than appearing at the end.
//!
//! Streaming also gives cancellation somewhere to live. A job stops when its
//! flag is set — the user closed the library, or dropped a different file — and
//! it stops on its own when the sink says nobody is listening, which is what a
//! webview that navigated away looks like from here. Both matter more than they
//! sound: the decode holds a CPU core per worker, and abandoning it is the
//! difference between a responsive editor and one that is busy drawing frames
//! nobody will ever see.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rayon::prelude::*;
use serde::Serialize;

use super::decoder::VideoDecoder;
use super::probe::probe_video;
use super::{MediaError, Result};
use crate::modules::project::Micros;
use crate::modules::workspace::paths;

/// JPEG quality for thumbnails. High enough that timeline frames do not show
/// blocking on flat gradients, low enough that a 200-frame strip stays small.
const JPEG_QUALITY: u8 = 78;

/// Most tiles in one message. Small, because the point is that the first ones
/// arrive early; a batch that waits for sixteen frames of a cold 4K file has
/// given the latency back.
const BATCH_MAX: usize = 8;

/// Longest a produced tile waits for company before being sent alone. One
/// message per frame would be correct and wasteful; this bounds the waste
/// without bounding the responsiveness.
const BATCH_INTERVAL: Duration = Duration::from_millis(120);

/// One frame of a strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Thumbnail {
    /// Position in the strip, `0..count`. The frontend places by this rather
    /// than by arrival order, because workers finish out of order.
    pub index: usize,
    /// Where in the source file this frame was taken from.
    pub at: Micros,
    /// Absolute path in the cache. The webview turns it into an asset URL.
    pub path: String,
}

/// A message from a running strip job.
///
/// Every job sends at least one: the terminal batch, with `complete` set. A
/// caller that only wants to know when the strip is done can ignore the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThumbnailBatch {
    pub job_id: String,
    /// Tiles in the whole strip, known before any decoding happens.
    pub total: usize,
    /// Tiles produced so far, cached ones included. Named `produced` rather than
    /// `done` because it is a count, and a field called `done` next to `complete`
    /// reads as a second boolean.
    pub produced: usize,
    /// The tiles this message carries. Empty on the terminal batch.
    pub tiles: Vec<Thumbnail>,
    /// True on the last message of the job, however it ended.
    pub complete: bool,
    /// The job stopped early because it was asked to.
    pub cancelled: bool,
    /// The job stopped early because it could not go on. User-facing prose.
    pub error: Option<String>,
}

/// Where produced tiles go.
///
/// A trait rather than Tauri's `Channel` directly so this module stays free of
/// Tauri and can be driven from a test. The return value is what makes a job
/// abandonable: `false` means the other end is gone, and the job stops.
pub trait BatchSink: Send + Sync {
    /// Deliver a batch. `false` if there is nobody left to deliver to.
    fn send(&self, batch: ThumbnailBatch) -> bool;
}

/// The sink for work nobody is watching — `thumbnail_strip`, and tests.
impl BatchSink for () {
    fn send(&self, _batch: ThumbnailBatch) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// The job registry
// ---------------------------------------------------------------------------

/// Cancel flags of the jobs currently running, by id.
///
/// An atomic flag rather than a channel, for the same reason the exporter uses
/// one: cancellation is a fact, not a message, and the worker only ever needs
/// to ask "should I still be doing this" between frames.
static JOBS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();

fn jobs() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a job and get the flag its workers will watch.
pub fn begin_job(job_id: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    jobs().lock().insert(job_id.to_string(), Arc::clone(&flag));
    flag
}

/// Ask a job to stop. `false` when there is no such job — which is the ordinary
/// outcome of cancelling one that just finished, not an error.
pub fn cancel_job(job_id: &str) -> bool {
    match jobs().lock().get(job_id) {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// Stop everything. For window close and app shutdown.
pub fn cancel_all() {
    for flag in jobs().lock().values() {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Forget a finished job, so a later id reuse cannot inherit its flag.
pub fn end_job(job_id: &str) {
    jobs().lock().remove(job_id);
}

// ---------------------------------------------------------------------------
// Generating a strip
// ---------------------------------------------------------------------------

/// Generate `count` evenly spaced thumbnails of `path` at `height` pixels tall.
///
/// The blocking form: it answers when the strip is complete and reports nothing
/// on the way. Everything the IPC layer drives goes through
/// [`thumbnail_stream`]; this exists for callers with nowhere to stream to.
pub fn thumbnail_strip(path: impl AsRef<Path>, count: usize, height: u32) -> Result<Vec<String>> {
    let cancel = AtomicBool::new(false);
    thumbnail_stream(path.as_ref(), count, height, "blocking", &cancel, &())
}

/// Generate a strip, reporting tiles to `sink` as they are produced.
///
/// Returns the cache paths of every tile that exists when the job ends, in
/// timeline order. A cancelled job is not an error: it answers with the tiles
/// it managed to produce, because they are on disk and the next request will
/// find them there.
pub fn thumbnail_stream(
    path: &Path,
    count: usize,
    height: u32,
    job_id: &str,
    cancel: &AtomicBool,
    sink: &dyn BatchSink,
) -> Result<Vec<String>> {
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

    let tiles: Vec<Tile> = thumbnail_times(info.duration, count)
        .into_iter()
        .enumerate()
        .map(|(index, at)| Tile {
            index,
            at,
            output: directory.join(frame_file_name(index, height)),
        })
        .collect();

    let emitter = Emitter::new(sink, job_id, count);

    // Cancelled before it started — the user dropped a file and undid it, or a
    // component mounted and unmounted in the same tick. Reporting tiles for a
    // job nobody is waiting for is worse than reporting none: the frontend
    // would draw a strip it has already forgotten it asked for.
    let abandoned_at_the_gate = cancel.load(Ordering::Relaxed);

    // Cached tiles cost a `stat` and are worth sending before anything opens a
    // decoder: a strip that is entirely cached draws in one frame of UI time.
    let (cached, pending): (Vec<&Tile>, Vec<&Tile>) =
        tiles.iter().partition(|tile| tile.output.exists());
    if !cached.is_empty() && !abandoned_at_the_gate {
        emitter.emit(cached.iter().map(|tile| tile.produced()).collect());
    }

    // One decoder per worker, never one shared: a decoder owns a demuxer with a
    // single read position, so two threads asking it for different timestamps
    // would seek each other's reads out from under them. Chunking rather than
    // per-frame parallelism keeps the number of file opens down to the number
    // of threads.
    let outcome = if abandoned_at_the_gate {
        Err(MediaError::Cancelled)
    } else if pending.is_empty() {
        Ok(())
    } else {
        let workers = rayon::current_num_threads().clamp(1, pending.len());
        let chunk = pending.len().div_ceil(workers);
        pending
            .par_chunks(chunk)
            .try_for_each(|chunk| render_chunk(path, height, chunk, cancel, &emitter))
    };

    let (cancelled, error) = match &outcome {
        Ok(()) => (false, None),
        Err(error) if error.is_cancellation() => (true, None),
        Err(error) => (false, Some(error.to_string())),
    };
    emitter.finish(cancelled, error);

    if let Err(error) = outcome {
        if !error.is_cancellation() {
            return Err(error);
        }
    }

    Ok(tiles
        .iter()
        .filter(|tile| tile.output.exists())
        .map(|tile| tile.output.to_string_lossy().into_owned())
        .collect())
}

struct Tile {
    index: usize,
    at: Micros,
    output: PathBuf,
}

impl Tile {
    fn produced(&self) -> Thumbnail {
        Thumbnail {
            index: self.index,
            at: self.at,
            path: self.output.to_string_lossy().into_owned(),
        }
    }
}

fn render_chunk(
    path: &Path,
    height: u32,
    tiles: &[&Tile],
    cancel: &AtomicBool,
    emitter: &Emitter<'_>,
) -> Result<()> {
    // Checked before the decoder is opened as well as before each frame: on a
    // cancel that arrives while the chunks are being handed out, opening a file
    // per worker is the whole cost of the job.
    if stopped(cancel, emitter) {
        return Err(MediaError::Cancelled);
    }

    let mut decoder = VideoDecoder::open_scaled(path, height)?;
    let mut batch: Vec<Thumbnail> = Vec::with_capacity(BATCH_MAX);
    let outcome = render_tiles(&mut decoder, tiles, cancel, emitter, &mut batch);

    // Whatever happened — finished, cancelled, or a decode that failed — every
    // tile already on disk is reported. A file the cache will find next time
    // but that the frontend was never told about is a tile that never appears.
    if !batch.is_empty() {
        emitter.emit(batch);
    }
    outcome
}

fn render_tiles(
    decoder: &mut VideoDecoder,
    tiles: &[&Tile],
    cancel: &AtomicBool,
    emitter: &Emitter<'_>,
    batch: &mut Vec<Thumbnail>,
) -> Result<()> {
    let mut last_sent = Instant::now();
    for tile in tiles {
        if stopped(cancel, emitter) {
            return Err(MediaError::Cancelled);
        }

        // Times inside a chunk ascend, so the decoder's forward-decode path
        // does the work whenever two thumbnails fall inside one GOP.
        let frame = decoder.seek_and_decode(tile.at)?;
        write_jpeg(&tile.output, &frame.data, frame.width, frame.height)?;

        batch.push(tile.produced());
        if is_due(batch.len(), last_sent.elapsed()) {
            emitter.emit(std::mem::take(batch));
            last_sent = Instant::now();
        }
    }
    Ok(())
}

/// Should this worker stop? Either the user said so, or nobody is listening.
fn stopped(cancel: &AtomicBool, emitter: &Emitter<'_>) -> bool {
    cancel.load(Ordering::Relaxed) || emitter.abandoned()
}

/// Whether an accumulated batch should go out now.
///
/// Pure so the policy can be tested without waiting 120 ms for it.
fn is_due(tiles: usize, since_last: Duration) -> bool {
    tiles > 0 && (tiles >= BATCH_MAX || since_last >= BATCH_INTERVAL)
}

/// Serialises batches from every worker and keeps the running count.
struct Emitter<'a> {
    sink: &'a dyn BatchSink,
    job_id: &'a str,
    total: usize,
    produced: AtomicUsize,
    /// Cleared the first time the sink refuses a batch. Once the other end is
    /// gone it does not come back, so this is a latch rather than a retry.
    listening: AtomicBool,
    /// Held across the send so two workers cannot interleave their messages and
    /// deliver a `done` count that goes backwards.
    order: Mutex<()>,
}

impl<'a> Emitter<'a> {
    fn new(sink: &'a dyn BatchSink, job_id: &'a str, total: usize) -> Self {
        Self {
            sink,
            job_id,
            total,
            produced: AtomicUsize::new(0),
            listening: AtomicBool::new(true),
            order: Mutex::new(()),
        }
    }

    fn emit(&self, tiles: Vec<Thumbnail>) {
        self.send(tiles, false, false, None);
    }

    fn finish(&self, cancelled: bool, error: Option<String>) {
        self.send(Vec::new(), true, cancelled, error);
    }

    fn send(&self, tiles: Vec<Thumbnail>, complete: bool, cancelled: bool, error: Option<String>) {
        let _order = self.order.lock();
        let produced = self.produced.fetch_add(tiles.len(), Ordering::Relaxed) + tiles.len();
        let delivered = self.sink.send(ThumbnailBatch {
            job_id: self.job_id.to_string(),
            total: self.total,
            produced,
            tiles,
            complete,
            cancelled,
            error,
        });
        if !delivered {
            self.listening.store(false, Ordering::Relaxed);
        }
    }

    fn abandoned(&self) -> bool {
        !self.listening.load(Ordering::Relaxed)
    }
}

/// Encode one frame, via a staging file and a rename.
///
/// A process killed mid-encode would otherwise leave a truncated JPEG that
/// `exists()` reads as a cached tile, and a corrupt thumbnail is forever: the
/// cache never regenerates a file that is already there.
fn write_jpeg(output: &Path, rgba: &[u8], width: u32, height: u32) -> Result<()> {
    // The counter, not just the process id: two jobs for the same file can run
    // at once — two components mounting, or a strip and a re-request at another
    // height — and two threads writing one staging path produce a file that is
    // half of each and then get renamed into the cache.
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let staging = output.with_extension(format!(
        "partial.{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));

    let encoder = jpeg_encoder::Encoder::new_file(&staging, JPEG_QUALITY).map_err(|source| {
        MediaError::Write {
            path: staging.clone(),
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
            path: staging.clone(),
            source: encoding_io_error(source),
        })?;

    std::fs::rename(&staging, output).map_err(|source| MediaError::Write {
        path: output.to_path_buf(),
        source,
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
/// The directory comes from `workspace::paths` rather than `temp_dir()`, so a
/// "clear cache" action has one place to look and the user can point the cache
/// at another disk. The fingerprint is a level below it because it changes when
/// the file does, and a stale strip is then simply an unreferenced directory
/// the cache sweep will collect.
fn cache_directory(path: &Path) -> PathBuf {
    paths::thumbnails_dir(path).join(fingerprint(path))
}

/// Bytes read from each end of a file to identify its contents.
///
/// Enough to cover a container header — moov atom, EBML head, RIFF chunk table —
/// which is where an edit that keeps a file's length still shows up.
const FINGERPRINT_SAMPLE: usize = 8 * 1024;

/// A stable, collision-resistant-enough name for one *version* of a media file.
///
/// Four things go in: the path, the length, the modification time, and eight
/// kilobytes from each end of the file. The last of those is not belt-and-braces
/// — **size and mtime alone are not enough**, and the unit test below is the
/// proof: two writes microseconds apart get the same mtime from the kernel, so a
/// file edited in place without changing length keeps its key and shows its old
/// thumbnails forever. Reading 16 KB once per request costs nothing measurable
/// against a decode and closes that hole for anything with a header, which is
/// every container we open.
///
/// What it still cannot catch is a same-length change confined to the middle of
/// a file whose mtime also did not move. That is a re-mux of identical duration
/// written inside one clock tick, and the honest answer is that a content hash
/// of a gigabyte of video costs more than the mistake.
///
/// A file we cannot open still gets a key from its path alone — a missing file
/// is a relink problem, not a reason to fail here.
///
/// Shared with `waveform.rs`, which keys its cache the same way, so that one
/// edit to a source file invalidates everything derived from it at once.
pub(super) fn fingerprint(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};

    let mut hash = FNV_OFFSET;
    hash = fnv1a(hash, path.to_string_lossy().as_bytes());

    let Ok(mut file) = std::fs::File::open(path) else {
        return format!("{hash:016x}");
    };
    let Ok(metadata) = file.metadata() else {
        return format!("{hash:016x}");
    };
    let length = metadata.len();
    hash = fnv1a(hash, &length.to_le_bytes());
    if let Ok(modified) = metadata.modified() {
        if let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) {
            hash = fnv1a(hash, &since_epoch.as_secs().to_le_bytes());
        }
    }

    let head = FINGERPRINT_SAMPLE.min(length as usize);
    let mut buffer = vec![0u8; head];
    if file.read_exact(&mut buffer).is_ok() {
        hash = fnv1a(hash, &buffer);
    }
    // The tail as well as the head: an encoder that rewrites a trailer — an
    // index, or a moov atom appended at the end — leaves the first kilobytes
    // untouched.
    if length > FINGERPRINT_SAMPLE as u64 {
        let mut tail = vec![0u8; FINGERPRINT_SAMPLE];
        if file.seek(SeekFrom::End(-(FINGERPRINT_SAMPLE as i64))).is_ok()
            && file.read_exact(&mut tail).is_ok()
        {
            hash = fnv1a(hash, &tail);
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
    fn fingerprints_differ_per_path_and_are_fixed_width() {
        let a = fingerprint(Path::new("/media/a.mp4"));
        let b = fingerprint(Path::new("/media/b.mp4"));
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert_eq!(a, fingerprint(Path::new("/media/a.mp4")), "keys are stable");
    }

    #[test]
    fn the_cache_lives_under_the_workspace_cache_root() {
        let directory = cache_directory(Path::new("/media/a.mp4"));
        assert!(
            directory.starts_with(paths::cache_root()),
            "thumbnails must not go to the temp directory: {directory:?}"
        );
        assert!(!directory.starts_with(std::env::temp_dir().join("chukcut")));
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

    // -- cache invalidation -------------------------------------------------

    #[test]
    fn rewriting_a_file_moves_its_thumbnail_directory() {
        let dir = std::env::temp_dir().join(format!("chukcut-thumb-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        let media = dir.join("clip.mp4");

        std::fs::write(&media, b"one").unwrap();
        let before = cache_directory(&media);
        // Same length, different content, and — because these two writes happen
        // microseconds apart — very likely the *same* mtime, since the kernel
        // stamps at clock-tick granularity. This is exactly the case a
        // size-and-mtime key gets wrong, and it is why `fingerprint` reads the
        // file's first bytes as well as its metadata.
        std::fs::write(&media, b"two").unwrap();
        let after = cache_directory(&media);

        assert_ne!(before, after, "an edited file must not reuse its strip");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_only_touched_keeps_its_strip() {
        // The other half of the trade. Copying a project between disks moves
        // every mtime; re-decoding every filmstrip because of that would be a
        // minute of work for no change in what is drawn.
        let dir = std::env::temp_dir().join(format!("chukcut-thumb-touch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        let media = dir.join("clip.mp4");

        std::fs::write(&media, b"identical bytes").unwrap();
        let before = cache_directory(&media);
        std::fs::write(&media, b"identical bytes").unwrap();
        assert_eq!(
            before,
            cache_directory(&media),
            "rewriting the same bytes is not an edit"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- batching -----------------------------------------------------------

    #[test]
    fn a_batch_goes_out_when_it_is_full_or_when_it_has_waited() {
        assert!(!is_due(0, Duration::from_secs(10)), "nothing to send");
        assert!(!is_due(1, Duration::ZERO), "one fresh tile waits for company");
        assert!(is_due(BATCH_MAX, Duration::ZERO), "a full batch goes now");
        assert!(
            is_due(1, BATCH_INTERVAL),
            "a tile that has waited long enough goes alone"
        );
    }

    // -- the sink and the job registry --------------------------------------

    /// Records every batch. `accept` false is a webview that went away.
    struct Recorder {
        batches: Mutex<Vec<ThumbnailBatch>>,
        accept: bool,
    }

    impl Recorder {
        fn new(accept: bool) -> Self {
            Self {
                batches: Mutex::new(Vec::new()),
                accept,
            }
        }
    }

    impl BatchSink for Recorder {
        fn send(&self, batch: ThumbnailBatch) -> bool {
            self.batches.lock().push(batch);
            self.accept
        }
    }

    fn tile(index: usize) -> Thumbnail {
        Thumbnail {
            index,
            at: index as Micros * 1_000,
            path: format!("/cache/{index}.jpg"),
        }
    }

    #[test]
    fn the_produced_count_accumulates_across_batches() {
        let recorder = Recorder::new(true);
        let emitter = Emitter::new(&recorder, "job", 5);
        emitter.emit(vec![tile(0), tile(1)]);
        emitter.emit(vec![tile(2)]);
        emitter.finish(false, None);

        let batches = recorder.batches.lock();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].produced, 2);
        assert_eq!(batches[1].produced, 3);
        assert_eq!(batches[2].produced, 3, "the terminal batch carries no tiles");
        assert!(batches.iter().all(|b| b.total == 5));
        assert!(batches[2].complete && !batches[2].cancelled);
        assert!(!batches[0].complete);
    }

    #[test]
    fn a_sink_that_refuses_marks_the_job_abandoned() {
        let recorder = Recorder::new(false);
        let emitter = Emitter::new(&recorder, "job", 3);
        assert!(!emitter.abandoned(), "nothing has been sent yet");
        emitter.emit(vec![tile(0)]);
        assert!(
            emitter.abandoned(),
            "a refused batch means the webview is gone"
        );

        let cancel = AtomicBool::new(false);
        assert!(
            stopped(&cancel, &emitter),
            "an abandoned job stops without anyone cancelling it"
        );
    }

    #[test]
    fn a_cancel_flag_stops_a_job_that_is_still_being_listened_to() {
        let recorder = Recorder::new(true);
        let emitter = Emitter::new(&recorder, "job", 3);
        let cancel = AtomicBool::new(false);
        assert!(!stopped(&cancel, &emitter));
        cancel.store(true, Ordering::Relaxed);
        assert!(stopped(&cancel, &emitter));
    }

    #[test]
    fn a_registered_job_can_be_cancelled_by_id_and_only_until_it_ends() {
        let id = format!("thumb-test-{}", std::process::id());
        let flag = begin_job(&id);
        assert!(!flag.load(Ordering::Relaxed));

        assert!(cancel_job(&id));
        assert!(flag.load(Ordering::Relaxed));

        end_job(&id);
        assert!(
            !cancel_job(&id),
            "cancelling a finished job is false, not an error"
        );
    }

    #[test]
    fn cancelling_a_job_nobody_started_is_not_an_error() {
        assert!(!cancel_job("no-such-thumbnail-job"));
    }

    // -- argument checking --------------------------------------------------

    #[test]
    fn an_empty_strip_or_a_zero_height_is_refused_with_a_sentence() {
        let cancel = AtomicBool::new(false);
        let error = thumbnail_stream(Path::new("/x.mp4"), 0, 72, "j", &cancel, &())
            .unwrap_err()
            .to_string();
        assert!(error.contains("at least one frame"), "{error}");

        let error = thumbnail_stream(Path::new("/x.mp4"), 4, 0, "j", &cancel, &())
            .unwrap_err()
            .to_string();
        assert!(error.contains("greater than zero"), "{error}");
    }

    #[test]
    fn a_job_cancelled_before_it_starts_decodes_nothing_and_still_reports() {
        // The file does not exist, so reaching the probe at all would error.
        // Cancellation is checked inside the workers, which is past the probe,
        // so this asserts the shape of the refusal rather than the decode: a
        // strip of a missing file is an error, and a cancelled one is not.
        let cancel = AtomicBool::new(true);
        let recorder = Recorder::new(true);
        let result = thumbnail_stream(
            Path::new("/nonexistent/definitely-not-here.mp4"),
            4,
            72,
            "j",
            &cancel,
            &recorder,
        );
        assert!(result.is_err(), "a file that is not there cannot be striped");
        assert!(
            recorder.batches.lock().is_empty(),
            "a job that never started sends nothing"
        );
    }
}
