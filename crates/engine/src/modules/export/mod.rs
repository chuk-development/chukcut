//! Export pipeline.
//!
//! Walks the timeline at full resolution through `render`, feeds frames to an
//! FFmpeg encoder, mixes audio, and reports progress. Runs off the UI thread
//! and never holds the project lock — it takes a snapshot of the document and
//! renders from that, so the user can keep editing while an export runs.
//!
//! ## The shape of it
//!
//! ```text
//!   ExportRequest ──► resolved against a preset ──► ExportSettings
//!                                                        │
//!   Project snapshot ──┐                                 ▼
//!                      ├──► frame loop ──► Compositor ──► MediaWriter ──► file
//!   SourceProvider ────┘        │                             ▲
//!                               │           AudioMixer ───────┘
//!                               ▼
//!                        Channel<ExportProgress>
//! ```
//!
//! The loop is deliberately dull: for frame `i` in `0..total`, ask [`Fps`] for
//! the timeline instant, ask the compositor for the pixels, hand them to the
//! encoder. Everything interesting is at the edges — the frame-rate arithmetic
//! in [`presets`], the time bases and the final flush in [`encoder`], the
//! per-segment gain walk in [`audio`].
//!
//! ## What it owns
//!
//! - **Presets.** Platform targets as data, plus the rational frame rate type
//!   the rest of the module counts in. Nothing here knows what "TikTok" is
//!   except [`presets`].
//! - **Muxing and encoding.** One output container, one video stream, at most
//!   one audio stream, through `ffmpeg-next`.
//! - **Hardware encoding.** Which encoders this machine has and whether they
//!   work ([`hwaccel`]), and the libavutil frame pool the ones that need it
//!   draw surfaces from ([`hwframes`]). Software stays the default; hardware is
//!   something the user chooses, for the reasons at the top of [`hwaccel`].
//! - **The audio mix.** Per-segment and per-track gain, speed, and the sum into
//!   one stereo bed at the output sample rate.
//! - **Job lifecycle.** A thread per export, an atomic cancel flag checked
//!   every frame, and progress with an ETA.
//!
//! ## What it deliberately does not own
//!
//! - **Decoding.** Video pixels arrive through `render`'s [`SourceProvider`]
//!   and audio samples through [`audio::AudioSource`], both injected. This
//!   module never opens an input file.
//! - **The GPU.** It borrows a `Compositor`; it does not create devices.
//! - **The document.** It reads a snapshot and mutates nothing.
//!
//! [`SourceProvider`]: crate::modules::render::SourceProvider
//! [`Fps`]: presets::Fps

pub mod audio;
pub mod commands;
pub mod encoder;
pub mod hwaccel;
pub mod hwframes;
pub mod job;
pub mod presets;
pub mod snapshot;

pub use audio::{mix_timeline, AudioMixer, AudioRequest, AudioSource, SilentAudioSource};
pub use encoder::{AudioStreamSpec, MediaWriter, VideoStreamSpec, WriterStats};
pub use hwaccel::{HwAccel, HwEncoder, RateControl};
pub use hwframes::{HwDeviceContext, HwFramesContext};
pub use job::{
    export_options, gpu_color_convert, register_audio_source, resolve_settings, run_export,
    set_gpu_color_convert, set_zero_copy, walk_frames, zero_copy_enabled, ExportJob, ExportOptions,
    ExportOutcome, ExportOverrides, ExportProgress, ExportRequest, ExportSettings, ExportStage,
    FnSink, ProgressSink,
};
pub use presets::{
    AudioCodec, Container, ExportPreset, Fps, Quality, VideoCodec, CUSTOM_PRESET_ID,
};

use crate::modules::render::RenderError;

/// Everything that can stop an export.
///
/// Unlike `RenderError` these are written as user-facing prose from the start:
/// an export failure is always shown to the person who asked for it, and there
/// is no layer between here and the dialog that knows enough to turn
/// "InvalidData" into something actionable. The command layer only calls
/// `to_string()`.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("{0}")]
    Settings(String),

    #[error("this build of FFmpeg has no {0} encoder")]
    NoEncoder(String),

    #[error("cannot write {path}: {source}")]
    Open {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    /// Anything libav refuses. `what` is the step, so the message reads as a
    /// sentence: "cannot open the H.264 encoder: Invalid argument".
    #[error("{what}: {source}")]
    Ffmpeg {
        what: String,
        source: ffmpeg_next::Error,
    },

    #[error("rendering frame {frame} failed: {source}")]
    Render {
        frame: u64,
        #[source]
        source: RenderError,
    },

    #[error("mixing audio failed: {0}")]
    Audio(#[source] anyhow::Error),

    /// Not a failure — the user asked to stop. Carried as an error so the frame
    /// loop can unwind with `?` instead of threading an outcome through every
    /// call, and turned back into a normal outcome at the top.
    #[error("the export was cancelled")]
    Cancelled,
}

impl ExportError {
    /// `map_err(ExportError::ffmpeg("opening the H.264 encoder"))`.
    pub(crate) fn ffmpeg(
        what: impl Into<String>,
    ) -> impl FnOnce(ffmpeg_next::Error) -> ExportError {
        let what = what.into();
        move |source| ExportError::Ffmpeg { what, source }
    }

    pub fn is_cancellation(&self) -> bool {
        matches!(self, ExportError::Cancelled)
    }
}

pub type Result<T> = std::result::Result<T, ExportError>;
