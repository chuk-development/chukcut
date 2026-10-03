//! Audio playback.
//!
//! The preview pipeline document says audio is the clock master and video is
//! matched to it: humans forgive a dropped frame and never forgive a stutter
//! in the sound. This module is the other half of that arrangement. It mixes
//! the timeline in Rust, plays it through the system device, and publishes the
//! device's own playback position as a
//! [`TimeSource`](crate::modules::preview::clock::TimeSource) for the preview
//! clock to run on.
//!
//! ## The shape of it
//!
//! ```text
//!   project snapshot
//!         │
//!         ▼
//!   TimelineMixer ──► stereo f32 blocks ──► channel map ──► ring buffer
//!    (fill thread)                                              │
//!         ▲                                                     ▼
//!    AudioClipReader (one open decoder per segment)      device callback
//!                                                          (real time)
//!                                                               │
//!                                              frames played ──►│
//!                                                               ▼
//!                                                        AudioTimeSource
//!                                                               │
//!                                                        PlaybackClock
//! ```
//!
//! Three threads, and the boundaries between them are the whole design:
//!
//! - The **device callback** is real-time. It copies from a lock-free ring,
//!   converts to the device's sample format, and adds to two atomics. It never
//!   allocates, never locks anything another thread holds, never does IO. An
//!   underrun is silence, not a wait.
//! - The **fill thread** owns the mixer, the decoders and the producing end of
//!   the ring. It is allowed to be slow — that is what the ring is for — and
//!   it is the only thread that touches FFmpeg.
//! - **Command threads** publish intentions in atomics and wake the fill
//!   thread. Nothing an IPC command does can block on a decoder or a device.
//!
//! ## What it deliberately does not do
//!
//! - **Own the playhead.** The playhead is `PlaybackClock`, in `preview`. This
//!   module reports where the device is; the clock decides what that means.
//! - **Mix the export.** `export::audio` mixes a whole project into one buffer
//!   in one pass, which is the right shape for writing a file and the wrong
//!   one for a preview that can be seeked mid-block. What the two share is
//!   [`decode::FileAudioSource`], which answers the export mixer's own
//!   interface with a real decoder.
//! - **Run stateful effects.** A pitch-preserving speed change, a speed curve
//!   and the audio effects are rendered per clip by `modules::audiofx` and
//!   cached; the plan points such a clip at its render, and the fill thread
//!   re-plans when a render lands (`audiofx::cache::generation`).

use std::path::PathBuf;

use ffmpeg_next as ffmpeg;

pub mod clock;
pub mod commands;
pub mod decode;
pub mod device;
pub mod engine;
pub mod mixer;
pub mod ring;

pub use clock::{AudioTimeSource, DeviceClock};
pub use decode::{AudioClipReader, ClipFactory, ClipReader, FileAudioSource, FileClipFactory};
pub use device::{has_output_device, AudioOutput, DeviceInfo};
pub use engine::{AudioEngine, AudioStatus};
pub use mixer::{plan, soft_limit, PlannedSegment, TimelineMixer, MIX_CHANNELS};
pub use ring::{ring, RingConsumer, RingProducer};

/// Everything that can go wrong between a file and a speaker.
///
/// As in `media`, the `Display` strings are written to be shown to a user
/// unchanged, because the IPC layer only calls `to_string()` on them. "no
/// audio output device is available" is actionable; `NoDevice` is not.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio output device is available")]
    NoDevice,

    #[error("the audio device could not be used: {0}")]
    Device(#[from] cpal::Error),

    #[error("cannot open {path}: {source}")]
    Open {
        path: PathBuf,
        source: ffmpeg::Error,
    },

    #[error("{0} has no audio stream")]
    NoAudioStream(PathBuf),

    #[error("cannot decode audio from {path}: {source}")]
    Decode {
        path: PathBuf,
        source: ffmpeg::Error,
    },

    #[error("{0}")]
    Unsupported(String),

    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, AudioError>;
