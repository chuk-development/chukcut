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
pub mod estimate;
pub mod hwaccel;
pub mod hwframes;
pub mod job;
pub mod presets;
pub mod queue;
pub mod snapshot;
pub mod store;

pub use audio::{
    mix_timeline, mix_timeline_unclamped, AudioMixer, AudioRequest, AudioSource, SilentAudioSource,
};
pub use encoder::{AudioStreamSpec, MediaWriter, VideoStreamSpec, WriterStats};
pub use estimate::{EstimateMethod, SizeEstimate};
pub use hwaccel::{HwAccel, HwEncoder, RateControl};
pub use hwframes::{HwDeviceContext, HwFramesContext};
pub use job::{
    export_options, gpu_color_convert, register_audio_source, resolve_settings, run_export,
    set_gpu_color_convert, set_zero_copy, walk_frames, zero_copy_enabled, ExportJob, ExportOptions,
    ExportOutcome, ExportOverrides, ExportProgress, ExportRequest, ExportSettings, ExportStage,
    FnSink, ProgressSink,
};
pub use presets::{
    AudioCodec, Container, ExportPreset, Fps, PresetCategory, Quality, VideoCodec, CUSTOM_PRESET_ID,
};
pub use queue::{ExportQueue, QueueEvent, QueueItem, QueueStatus};
pub use store::ExportMemory;

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

/// Fixtures shared by this module's unit tests.
#[cfg(test)]
pub(crate) mod testing {
    use std::path::PathBuf;

    use crate::modules::project::document::{
        CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
        VideoMaterial,
    };

    /// A project with one video clip of `duration` on a canvas of the given
    /// size. The clip's file does not exist; settings never open it.
    pub fn project(width: u32, height: u32, duration: Micros) -> Project {
        let mut project = Project::new(
            "t",
            CanvasConfig {
                width,
                height,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            30.0,
        );
        project.materials.videos.push(VideoMaterial {
            id: "v1".into(),
            path: "/nonexistent/v1.mp4".into(),
            width,
            height,
            duration,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(Segment {
            id: "s1".into(),
            material_id: "v1".into(),
            target_range: TimeRange::new(0, duration),
            source_range: TimeRange::new(0, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        project.tracks.push(track);
        project
    }

    /// An empty directory for one test, under the build's target directory.
    pub fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/export")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the test directory");
        dir
    }
}
