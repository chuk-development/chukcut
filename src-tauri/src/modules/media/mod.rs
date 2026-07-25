//! Media inspection and decoding.
//!
//! Owns every interaction with FFmpeg: probing a file for its streams,
//! decoding frames at a requested timestamp, building thumbnail strips for the
//! timeline, and extracting audio peaks for waveforms. Nothing outside this
//! module links against libav* directly, so the day we swap a decoder for a
//! hardware path or a proxy file, the change is contained here.
//!
//! ## What this module hands back
//!
//! Everything leaves in the document's units. Times are `Micros` (`i64`
//! microseconds), never floats and never frames, because a caller that gets a
//! float has to guess a frame rate to make sense of it. Pixels leave as tightly
//! packed RGBA8 with the container's display rotation already applied, because
//! rotation is a property of the *file* and a renderer that has to remember to
//! ask about it will eventually forget.
//!
//! ## Why the decoder is a long-lived object
//!
//! `VideoDecoder` opens a file once and answers repeated "give me the frame at
//! t" questions. That shape is not an optimisation detail, it is the whole
//! point: a video frame is only reachable by decoding forward from the nearest
//! preceding keyframe, so the cost of a frame depends entirely on what the
//! decoder already has in flight. Reopening the file per frame turns scrubbing
//! and playback into a keyframe-decode storm. See `decoder.rs` for the seek
//! policy.
//!
//! ## Threading
//!
//! A `VideoDecoder` wraps raw FFmpeg contexts: it is `Send` but not `Sync`.
//! Parallel work (thumbnail strips) gives each worker its own decoder rather
//! than sharing one behind a lock, which is both correct and faster — two
//! threads seeking one demuxer would fight over its read position.

use std::path::PathBuf;
use std::sync::Once;

use ffmpeg_next as ffmpeg;

pub mod commands;
pub mod decoder;
pub mod dmabuf;
pub mod hwdecode;
pub mod probe;
pub mod provider;
pub mod thumbnails;

pub mod waveform;

pub use decoder::{Acceleration, DecodedFrame, VideoDecoder};
pub use dmabuf::DmabufFrame;
pub use hwdecode::{HwCodec, HwDecodeSupport};
pub use provider::MediaSourceProvider;
pub use probe::{probe, AudioStreamInfo, MediaInfo, VideoStreamInfo};
pub use thumbnails::thumbnail_strip;
pub use waveform::waveform;

/// Everything that can go wrong reaching for a media file.
///
/// The `Display` strings are written to be shown to a user unchanged — the IPC
/// layer only calls `to_string()` on these. That is why each variant carries
/// the path: "cannot open /x/y.mp4: invalid data" is actionable, "InvalidData"
/// is not.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("cannot open {path}: {source}")]
    Open {
        path: PathBuf,
        source: ffmpeg::Error,
    },

    #[error("{0} has no video stream")]
    NoVideoStream(PathBuf),

    #[error("{0} has no audio stream")]
    NoAudioStream(PathBuf),

    #[error("no decoder is available for the {codec} stream in {path}")]
    NoDecoder { path: PathBuf, codec: String },

    #[error("cannot decode {path}: {source}")]
    Decode {
        path: PathBuf,
        source: ffmpeg::Error,
    },

    #[error("no frame could be decoded from {path} at {at} µs")]
    NoFrameAt { path: PathBuf, at: i64 },

    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },

    /// The GPU cannot be used for this, and software is the answer.
    ///
    /// Carries no path because it is a property of the *machine*, not of a
    /// file: no render node, a driver that will not initialise, a codec this
    /// chip does not decode. Every caller's response is the same — fall back —
    /// so the string exists to be logged once, not shown per file.
    #[error("hardware decode is unavailable: {0}")]
    NoHardware(String),

    /// A libav call on the hardware path failed.
    #[error("{what}: {source}")]
    Hardware {
        what: String,
        source: ffmpeg::Error,
    },

    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, MediaError>;

static FFMPEG_INIT: Once = Once::new();

/// Bring libav* up exactly once per process.
///
/// Every entry point in this module calls this rather than relying on the app
/// to have initialised FFmpeg at startup, so a unit test or a future headless
/// tool can use the decoder without booting Tauri.
pub fn ensure_initialized() {
    FFMPEG_INIT.call_once(|| {
        if let Err(error) = ffmpeg::init() {
            tracing::error!(%error, "FFmpeg failed to initialise");
        }
        // libav writes a great deal of per-frame chatter to stderr at the
        // default level, which drowns our own logs during playback.
        ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Error);
    });
}

/// Microseconds per second, as a float, for the rational rescales below.
const MICROS_PER_SECOND_F: f64 = 1_000_000.0;

/// Convert a timestamp in some stream's time base into microseconds.
///
/// FFmpeg timestamps are integers in a per-stream unit; the document only
/// speaks microseconds. Rounding rather than truncating matters: a 30 fps
/// stream in a 1/30 time base would otherwise drift a microsecond per frame and
/// a seek to "frame 900" would land on frame 899.
pub(crate) fn ts_to_micros(ts: i64, time_base: ffmpeg::Rational) -> i64 {
    (ts as f64 * f64::from(time_base) * MICROS_PER_SECOND_F).round() as i64
}

/// Inverse of [`ts_to_micros`].
pub(crate) fn micros_to_ts(micros: i64, time_base: ffmpeg::Rational) -> i64 {
    let base = f64::from(time_base);
    if base <= 0.0 {
        return 0;
    }
    (micros as f64 / MICROS_PER_SECOND_F / base).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffmpeg::Rational;

    #[test]
    fn timestamps_round_trip_through_microseconds() {
        // 1/30 s per tick: frame 900 is exactly 30 seconds in.
        let tb = Rational::new(1, 30);
        assert_eq!(ts_to_micros(900, tb), 30_000_000);
        assert_eq!(micros_to_ts(30_000_000, tb), 900);

        // 1/90000 is the MPEG-TS clock; 90000 ticks is one second.
        let tb = Rational::new(1, 90_000);
        assert_eq!(ts_to_micros(90_000, tb), 1_000_000);
        assert_eq!(micros_to_ts(1_000_000, tb), 90_000);
    }

    #[test]
    fn timestamp_conversion_rounds_instead_of_truncating() {
        // 1/30 does not divide evenly into microseconds: one frame is
        // 33333.33… µs. Truncating here is what makes a long seek land one
        // frame early.
        let tb = Rational::new(1, 30);
        assert_eq!(ts_to_micros(1, tb), 33_333);
        assert_eq!(micros_to_ts(33_333, tb), 1);
        assert_eq!(micros_to_ts(66_667, tb), 2);
    }

    #[test]
    fn degenerate_time_base_does_not_divide_by_zero() {
        assert_eq!(micros_to_ts(1_000_000, Rational::new(0, 1)), 0);
    }
}
