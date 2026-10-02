//! Data derived from real media: audio envelopes and thumbnail strips.
//!
//! The unit tests beside `media::waveform` and `media::thumbnails` cover the
//! arithmetic — bucket spans, batch policy, cache encoding. What they cannot
//! cover is whether FFmpeg will actually decode the file, and that is where
//! this subsystem's one long-standing defect lived: a plain PCM WAV declares a
//! channel *count* and no channel *layout*, FFmpeg 6 decodes it to a frame
//! whose `AVChannelLayout` is `AV_CHANNEL_ORDER_UNSPEC`, and swresample — which
//! `ffmpeg-next` still configures through the legacy bitmask API — rejects every
//! such frame with "Input changed". Every waveform of a WAV file failed.
//!
//! So the fixtures here are written byte by byte rather than encoded by
//! `ffmpeg`: a WAV built by hand is the *only* way to be sure the `fmt ` chunk
//! is the plain 16-byte kind with no channel mask, which is exactly the case
//! that broke. An `ffmpeg`-produced file might or might not carry one.
//!
//! The thumbnail tests need a real video and therefore need `ffmpeg`; they skip
//! with a reason when it is missing rather than failing, for the reason
//! `support` explains.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chukcut_engine::modules::media::{
    thumbnail_stream, waveform, BatchSink, ThumbnailBatch, Waveform,
};
use chukcut_engine::modules::workspace::paths;
use parking_lot::Mutex;

// ---------------------------------------------------------------------------
// Hand-built WAV fixtures
// ---------------------------------------------------------------------------

const WAV_RATE: u32 = 8_000;

/// A PCM WAV file with a channel count and **no** channel layout.
///
/// The `fmt ` chunk is the plain 16-byte form — `WAVE_FORMAT_PCM`, no extension
/// and therefore no `dwChannelMask`. That is what makes this fixture worth
/// having: it is the shape FFmpeg 6 decodes to `AV_CHANNEL_ORDER_UNSPEC`, and
/// before `name_the_layout` was transplanted into `media::waveform` every one of
/// these produced "cannot decode …: Input changed" instead of a waveform.
struct Wav {
    path: PathBuf,
}

impl Wav {
    /// Write `frames` sample frames of `channels` channels, each sample taken
    /// from `sample(frame, channel)` in `-1.0..=1.0`.
    fn new(name: &str, channels: u16, frames: usize, sample: impl Fn(usize, u16) -> f32) -> Self {
        let channels = channels.max(1);
        let data_len = frames * channels as usize * 2;

        let mut bytes = Vec::with_capacity(44 + data_len);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes()); // 16, not 40: no channel mask
        bytes.extend_from_slice(&1u16.to_le_bytes()); // WAVE_FORMAT_PCM
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&WAV_RATE.to_le_bytes());
        bytes.extend_from_slice(&(WAV_RATE * channels as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for frame in 0..frames {
            for channel in 0..channels {
                let value = sample(frame, channel).clamp(-1.0, 1.0);
                let quantised = (value * i16::MAX as f32).round() as i16;
                bytes.extend_from_slice(&quantised.to_le_bytes());
            }
        }

        let path = std::env::temp_dir().join(format!(
            "chukcut-waveform-{name}-{}.wav",
            std::process::id()
        ));
        std::fs::write(&path, bytes).expect("write the WAV fixture");
        Self { path }
    }

    /// A full-scale 400 Hz sine, `seconds` long.
    fn sine(name: &str, channels: u16, seconds: f32) -> Self {
        let frames = (WAV_RATE as f32 * seconds) as usize;
        Self::new(name, channels, frames, |frame, _| {
            (frame as f32 * std::f32::consts::TAU * 400.0 / WAV_RATE as f32).sin()
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Everything cached for this fixture, oldest name first.
    fn cache_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(paths::waveform_dir(&self.path))
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        files
    }
}

impl Drop for Wav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir_all(paths::waveform_dir(&self.path));
    }
}

#[track_caller]
fn envelope(path: &Path, buckets: usize) -> Waveform {
    waveform(path, buckets).unwrap_or_else(|error| {
        panic!("waveform of {} failed: {error}", path.display());
    })
}

// ---------------------------------------------------------------------------
// The defect: a channel count with no channel layout
// ---------------------------------------------------------------------------

#[test]
fn a_wav_that_declares_no_channel_layout_still_produces_a_waveform() {
    // The regression test for the whole exercise. Without `name_the_layout`
    // this fails with "cannot decode …: Input changed" — not on some files, on
    // every plain WAV, which is most of what a user drags onto an audio lane.
    let fixture = Wav::sine("stereo-no-layout", 2, 1.0);
    let drawn = envelope(fixture.path(), 256);

    assert_eq!(drawn.buckets, 256);
    assert_eq!(drawn.channels, 2);
    assert_eq!(drawn.sample_rate, WAV_RATE);
    assert!(
        drawn.max.iter().any(|v| *v > 0.9),
        "a full-scale sine must reach full scale: {:?}",
        &drawn.max[..8]
    );
    assert!(
        drawn.min.iter().any(|v| *v < -0.9),
        "and it is symmetric about zero"
    );
}

#[test]
fn a_mono_wav_is_not_a_special_case() {
    let fixture = Wav::sine("mono", 1, 0.5);
    let drawn = envelope(fixture.path(), 64);
    assert_eq!(drawn.channels, 1);
    assert!(drawn.max.iter().any(|v| *v > 0.9));
    assert!(drawn.rms.iter().any(|v| *v > 0.5));
}

#[test]
fn a_five_channel_wav_decodes_too() {
    // Five is deliberately not a layout anybody's `fmt ` chunk names, so the
    // fallback to the canonical layout for a channel count is what carries it.
    let fixture = Wav::sine("five-channel", 5, 0.25);
    let drawn = envelope(fixture.path(), 32);
    assert_eq!(drawn.channels, 5);
    assert!(drawn.max.iter().any(|v| *v > 0.9));
}

// ---------------------------------------------------------------------------
// The shape of the answer
// ---------------------------------------------------------------------------

#[test]
fn rms_separates_a_sine_from_its_own_peak() {
    // The reason the envelope carries three numbers rather than one. A sine's
    // RMS is 1/√2 of its peak; a peak-only lane draws both as the same block.
    let fixture = Wav::sine("rms", 1, 2.0);
    let drawn = envelope(fixture.path(), 1);

    assert!((drawn.max[0] - 1.0).abs() < 0.02, "peak {}", drawn.max[0]);
    assert!(
        (drawn.rms[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.05,
        "rms {} should be about 0.707 of the peak",
        drawn.rms[0]
    );
    assert!(
        drawn.rms[0] < drawn.max[0],
        "the body sits inside the outline"
    );
}

#[test]
fn a_silent_file_stays_silent_rather_than_being_amplified() {
    // Normalizing by the loudest bucket turns any nonzero file into a full
    // scale one; a silent file must not become full-scale noise.
    let fixture = Wav::new("silence", 2, 8_000, |_, _| 0.0);
    let drawn = envelope(fixture.path(), 128);

    assert_eq!(drawn.buckets, 128);
    assert!(drawn.min.iter().all(|v| *v == 0.0));
    assert!(drawn.max.iter().all(|v| *v == 0.0));
    assert!(drawn.rms.iter().all(|v| *v == 0.0));
    assert_eq!(drawn.peak, 0.0, "and it reports that it had no signal");
}

#[test]
fn a_file_shorter_than_one_bucket_is_answered_rather_than_refused() {
    // Eight sample frames at 8 kHz is a millisecond. A strip of 512 columns
    // over it is nonsense to draw and must still not panic or divide by zero.
    let fixture = Wav::new("very-short", 2, 8, |frame, _| frame as f32 / 8.0);
    let drawn = envelope(fixture.path(), 512);
    assert_eq!(drawn.buckets, 512);
    assert!(drawn.max.iter().all(|v| v.is_finite()));
    assert!(drawn.max.iter().any(|v| *v > 0.0), "the signal is in there");
}

#[test]
fn the_bucket_count_is_whatever_the_caller_asked_for() {
    // The point of caching at a fixed resolution: zoom changes the request and
    // does not change the decode. Both of these answers come from one decode,
    // and the coarse one is a reduction of the fine one rather than a
    // different measurement.
    let fixture = Wav::sine("resolutions", 2, 2.0);

    let fine = envelope(fixture.path(), 2_000);
    let coarse = envelope(fixture.path(), 100);
    assert_eq!(fine.buckets, 2_000);
    assert_eq!(coarse.buckets, 100);
    assert_eq!(fine.duration, coarse.duration);

    // Every coarse column must bound the twenty fine ones under it.
    for (index, bound) in coarse.max.iter().enumerate() {
        let inner = fine.max[index * 20..(index + 1) * 20]
            .iter()
            .fold(f32::MIN, |a, b| a.max(*b));
        assert!(
            (bound - inner).abs() < 1e-6,
            "coarse column {index} is {bound}, the fine ones peak at {inner}"
        );
    }
}

#[test]
fn a_file_with_no_audio_at_all_says_so_with_the_path_in_it() {
    let media = require_media!();
    let error = waveform(&media.counter, 256)
        .expect_err("the counter clip has no audio stream")
        .to_string();
    assert!(error.contains("no audio stream"), "{error}");
    assert!(
        error.contains("counter"),
        "the message names the file: {error}"
    );
}

#[test]
fn a_compressed_file_decodes_through_the_same_path() {
    // AAC in an MP4 declares a layout, so this is the case that always worked.
    // It is here so that a change made for WAV's sake cannot break it silently.
    let media = require_media!();
    let drawn = envelope(&media.audio_only, 128);
    assert_eq!(drawn.buckets, 128);
    assert!(
        drawn.max.iter().any(|v| *v > 0.5),
        "a sine reaches full scale"
    );
}

// ---------------------------------------------------------------------------
// The disk cache
// ---------------------------------------------------------------------------

#[test]
fn the_second_call_is_answered_from_disk_and_a_rewrite_invalidates_it() {
    let fixture = Wav::sine("cache", 1, 1.0);

    let first = envelope(fixture.path(), 64);
    assert!(first.max.iter().any(|v| *v > 0.9));

    let cached = fixture.cache_files();
    assert_eq!(cached.len(), 1, "one decode, one cache file: {cached:?}");
    assert!(
        cached[0].starts_with(paths::waveform_dir(fixture.path())),
        "the cache lives under the workspace cache root, not the temp directory"
    );

    // Tamper with the *cache* rather than the file, and read again. If the
    // second call decoded, it would return the sine; if it read the cache, it
    // returns what was put there. This is the only way to prove the cache is
    // consulted rather than merely written.
    // The header is magic, version, bucket count, rate, channels, duration and
    // peak; everything past it is the three float arrays. Zeroing those leaves
    // a structurally valid file whose contents cannot have come from a decode.
    let mut bytes = std::fs::read(&cached[0]).expect("read the cache");
    bytes[32..].fill(0);
    std::fs::write(&cached[0], &bytes).expect("tamper with the cache");

    let second = envelope(fixture.path(), 64);
    assert!(
        second.max.iter().all(|v| *v == 0.0),
        "the second call did not come from the cache"
    );

    // Now change the source file. Appending past the `data` chunk leaves the
    // audio identical to a decoder and changes both size and mtime, which is
    // what the cache key folds in — so the answer must be recomputed.
    let mut source = std::fs::read(fixture.path()).expect("read the fixture");
    source.extend_from_slice(&[0u8; 64]);
    std::fs::write(fixture.path(), source).expect("rewrite the fixture");

    let third = envelope(fixture.path(), 64);
    assert!(
        third.max.iter().any(|v| *v > 0.9),
        "an edited file must not be answered from the old envelope"
    );
    assert_eq!(
        fixture.cache_files().len(),
        2,
        "the new version is cached beside the old one rather than over it"
    );
}

#[test]
fn a_corrupted_cache_costs_a_decode_and_nothing_else() {
    let fixture = Wav::sine("corrupt-cache", 1, 0.5);
    let first = envelope(fixture.path(), 32);

    let cached = fixture.cache_files();
    std::fs::write(&cached[0], b"not a waveform").expect("corrupt the cache");

    let second = envelope(fixture.path(), 32);
    assert_eq!(
        first.max, second.max,
        "a cache file that is not one must be ignored, not trusted or fatal"
    );
}

// ---------------------------------------------------------------------------
// Thumbnail streaming
// ---------------------------------------------------------------------------

/// Records every batch, and optionally refuses them or cancels the job.
struct Recorder {
    batches: Mutex<Vec<ThumbnailBatch>>,
    accept: bool,
    /// Set on the first batch received, if present. This is how a mid-flight
    /// cancellation is provoked deterministically enough to assert on.
    cancel_on_first: Option<Arc<AtomicBool>>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            batches: Mutex::new(Vec::new()),
            accept: true,
            cancel_on_first: None,
        }
    }

    fn refusing() -> Self {
        Self {
            accept: false,
            ..Self::new()
        }
    }

    fn cancelling(flag: Arc<AtomicBool>) -> Self {
        Self {
            cancel_on_first: Some(flag),
            ..Self::new()
        }
    }

    fn recorded(&self) -> Vec<ThumbnailBatch> {
        self.batches.lock().clone()
    }
}

impl BatchSink for Recorder {
    fn send(&self, batch: ThumbnailBatch) -> bool {
        if let Some(flag) = &self.cancel_on_first {
            flag.store(true, Ordering::Relaxed);
        }
        self.batches.lock().push(batch);
        self.accept
    }
}

/// Every job ends with exactly one terminal message, whatever happened to it.
#[track_caller]
fn terminal(batches: &[ThumbnailBatch]) -> &ThumbnailBatch {
    let complete: Vec<&ThumbnailBatch> = batches.iter().filter(|b| b.complete).collect();
    assert_eq!(
        complete.len(),
        1,
        "a job must report completion exactly once: {batches:#?}"
    );
    assert!(
        std::ptr::eq(complete[0], batches.last().unwrap()),
        "the terminal message must be the last one"
    );
    complete[0]
}

/// The counter clip, copied to a path of its own.
///
/// The shared fixture is generated once per target directory and never changes,
/// so its thumbnail cache survives between runs — and a test about a *cold*
/// cache that silently ran warm on every run after the first would assert
/// nothing. A copy gets its own cache key, so every one of these starts empty.
struct ColdClip {
    path: PathBuf,
}

impl ColdClip {
    fn of(source: &Path, name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("chukcut-thumbs-{name}-{}.mp4", std::process::id()));
        std::fs::copy(source, &path).expect("copy the fixture");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ColdClip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir_all(paths::thumbnails_dir(&self.path));
    }
}

#[test]
fn tiles_arrive_before_the_strip_is_finished() {
    let media = require_media!();
    let clip = ColdClip::of(&media.counter, "streaming");
    let cancel = AtomicBool::new(false);
    let recorder = Recorder::new();

    let count = 24;
    let files = thumbnail_stream(clip.path(), count, 72, "streaming", &cancel, &recorder)
        .expect("the counter clip has a video stream");

    let batches = recorder.recorded();
    let end = terminal(&batches);
    assert!(!end.cancelled && end.error.is_none(), "{end:#?}");
    assert_eq!(end.total, count);
    assert_eq!(end.produced, count, "every tile is accounted for");

    // The whole point: something was reported before the job was done.
    assert!(
        batches.len() >= 2,
        "a strip of {count} tiles arrived as one message, so nothing streamed"
    );

    // Indices cover the strip exactly once, whatever order they arrived in.
    let mut indices: Vec<usize> = batches
        .iter()
        .flat_map(|b| b.tiles.iter().map(|t| t.index))
        .collect();
    indices.sort_unstable();
    assert_eq!(indices, (0..count).collect::<Vec<_>>());

    assert_eq!(files.len(), count);
    for path in &files {
        let path = Path::new(path);
        assert!(path.exists(), "{path:?} was reported but not written");
        assert!(
            path.starts_with(paths::cache_root()),
            "thumbnails must live under the workspace cache root: {path:?}"
        );
        assert!(
            std::fs::metadata(path)
                .map(|m| m.len() > 0)
                .unwrap_or(false),
            "{path:?} is empty"
        );
    }
}

#[test]
fn a_warm_cache_answers_without_decoding_anything() {
    let media = require_media!();
    let clip = ColdClip::of(&media.counter, "warm");
    let cancel = AtomicBool::new(false);

    let first = Recorder::new();
    thumbnail_stream(clip.path(), 8, 72, "warm-1", &cancel, &first).expect("first strip");

    let second = Recorder::new();
    let files =
        thumbnail_stream(clip.path(), 8, 72, "warm-2", &cancel, &second).expect("second strip");

    let batches = second.recorded();
    assert_eq!(files.len(), 8);
    // Cached tiles go out in one message before any decoder is opened, so a
    // fully warm strip is two messages: the tiles, then the terminal one.
    assert_eq!(
        batches.len(),
        2,
        "a warm strip should not have streamed in instalments: {batches:#?}"
    );
    assert_eq!(batches[0].tiles.len(), 8);
    assert_eq!(terminal(&batches).produced, 8);
}

#[test]
fn a_job_cancelled_before_it_starts_writes_nothing_and_still_reports() {
    let media = require_media!();
    let clip = ColdClip::of(&media.counter, "pre-cancelled");
    let cancel = AtomicBool::new(true);
    let recorder = Recorder::new();

    let files = thumbnail_stream(clip.path(), 16, 72, "pre-cancelled", &cancel, &recorder)
        .expect("a cancelled strip is not an error");

    let batches = recorder.recorded();
    let end = terminal(&batches);
    assert!(end.cancelled, "the terminal message says why it stopped");
    assert!(
        end.error.is_none(),
        "cancellation is not a failure to report to the user"
    );
    assert_eq!(end.produced, 0, "no frame was decoded");
    assert!(files.is_empty(), "and nothing was written: {files:?}");
}

#[test]
fn cancelling_mid_flight_stops_the_job_and_keeps_what_it_had() {
    let media = require_media!();
    let clip = ColdClip::of(&media.counter, "mid-flight");
    let cancel = Arc::new(AtomicBool::new(false));
    let recorder = Recorder::cancelling(Arc::clone(&cancel));

    let count = 48;
    let files = thumbnail_stream(clip.path(), count, 72, "mid-flight", &cancel, &recorder)
        .expect("a cancelled strip is not an error");

    let batches = recorder.recorded();
    let end = terminal(&batches);
    assert!(end.error.is_none(), "{end:#?}");

    // Whether the workers noticed in time is a race — a short clip can finish
    // before the flag is read. What must hold either way is that the job
    // terminated, that its own count agrees with what it wrote, and that a
    // cancelled job did not claim to be complete.
    assert_eq!(
        end.produced,
        files.len(),
        "the reported count and the files on disk disagree"
    );
    if end.cancelled {
        assert!(
            files.len() < count,
            "a job that reported cancellation still produced every tile"
        );
    }
    // Every tile that *was* produced is a real file, cancelled or not: a
    // half-written JPEG would be cached forever.
    for path in &files {
        assert!(std::fs::metadata(path)
            .map(|m| m.len() > 0)
            .unwrap_or(false));
    }
}

#[test]
fn a_sink_that_refuses_does_not_make_the_job_fail() {
    // The webview went away. The job stops as soon as it notices, and stopping
    // is not an error — there is nobody left to tell.
    let media = require_media!();
    let clip = ColdClip::of(&media.counter, "abandoned");
    let cancel = AtomicBool::new(false);
    let recorder = Recorder::refusing();

    let result = thumbnail_stream(clip.path(), 32, 72, "abandoned", &cancel, &recorder);
    assert!(result.is_ok(), "{result:?}");
    assert!(
        !recorder.recorded().is_empty(),
        "the job must have tried to report at least once"
    );
}

#[test]
fn a_file_that_cannot_be_striped_fails_before_it_reports_anything() {
    let cancel = AtomicBool::new(false);
    let recorder = Recorder::new();
    let error = thumbnail_stream(
        Path::new("/nonexistent/definitely-not-here.mp4"),
        4,
        72,
        "missing",
        &cancel,
        &recorder,
    )
    .expect_err("a file that is not there cannot be striped")
    .to_string();

    assert!(error.contains("definitely-not-here"), "{error}");
    assert!(
        recorder.recorded().is_empty(),
        "nothing to report before the file is open"
    );
}
