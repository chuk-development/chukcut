//! Preview frame server and playback clock.
//!
//! Turns the compositor's output into something a webview can display at video
//! rates. This module is the sink at the end of the render pipeline: it decides
//! *when* a frame is needed, at *what* resolution, and *how* it reaches the
//! screen. The compositor decides what is in it.
//!
//! ## Why frames do not go through `invoke()`
//!
//! Because it does not work, at any resolution. A 1080p RGBA frame is 8.3 MB;
//! at 30 fps that is 249 MB/s. `invoke()` is JSON, so those bytes would be
//! base64 — a third larger — and every frame would be parsed on the UI thread,
//! the same thread that has to stay responsive for the timeline. The preview
//! would be a slideshow and the editor would be unusable while it played.
//!
//! So frames leave by a different door. Rust renders at proxy resolution,
//! encodes JPEG, and serves the bytes over a registered URI scheme:
//!
//! ```text
//! chukcut-frame://preview/<session>/<frame>
//! ```
//!
//! The webview fetches those like any other image, which means the platform's
//! own image decoding on its own threads, no JSON, no base64, no copy through
//! the IPC bridge. At 960x540 quality 80 a frame is ~120 KB and 30 fps is
//! ~3.6 MB/s, which is nothing. `invoke()` still carries the control plane —
//! play, pause, seek — because that is small and wants an answer. The full
//! argument, including the two options we did not take, is in
//! `docs/architecture/preview-pipeline.md`.
//!
//! ## What it owns
//!
//! - **The render resolution.** [`session::preview_size`]. The default is the
//!   size of the panel on screen, in device pixels, because every pixel above
//!   that is composited, read back and JPEG-encoded so that it can be thrown
//!   away by a CSS downscale — a 1080p project in a 700 px panel was 7.5× the
//!   work the screen could use. The project canvas is the ceiling (never
//!   render above the source), the table in [`session::proxy_long_edge`] and
//!   `settings.preview_max_edge` cap it further, and
//!   `settings.preview_full_quality` overrides the lot. The aspect ratio
//!   always matches the export's.
//! - **The quality ladder.** [`ladder`]. Playback — and only playback — may
//!   drop to three-quarter or half size and a lower JPEG quality when the
//!   renderer cannot hold the frame budget, and climbs back after three quiet
//!   seconds. A **paused** frame is never on the ladder: pausing after a
//!   degraded run supersedes the session so the frame the user sits and looks
//!   at is rendered again at the session's own size and quality.
//! - **Sessions.** A monotonic id over a project snapshot and a resolution
//!   ([`PreviewSession`]). The id is in every frame URL, which is what stops a
//!   frame from a superseded seek painting over the current view — it comes
//!   back 410, never a stale image.
//! - **The ring buffer.** [`FrameCache`], 90 encoded frames by default.
//!   Playback reads ahead into it; a seek invalidates it. This is what turns
//!   render jitter into smooth playback.
//! - **The clock.** [`PlaybackClock`] over a swappable [`clock::TimeSource`],
//!   monotonic today and the audio device later, because audio position is the
//!   authority for the playhead. Late frames are dropped rather than shown
//!   late; time is never stretched to let the renderer catch up. A frame the
//!   playhead passes *while it is being composited* is thrown away without
//!   being encoded, counted separately as `discarded` — but never two in a row,
//!   or a renderer that is permanently behind would show nothing at all.
//! - **JPEG encoding.** On the GPU where the machine has a VAAPI JPEG
//!   entrypoint ([`vaapi`]), on libjpeg-turbo everywhere else. The compositor
//!   already hands back sRGB-encoded bytes, so the software path is a pure
//!   re-encode with no colour conversion; the hardware path converts to
//!   full-range NV12 because that is what the encoder eats.
//!   [`encoder::encode_preview_jpeg`] picks, and falls back without telling
//!   the caller. [`zerocopy`] is the version that does the conversion in the
//!   compositor's own compute pass and hands the encoder that memory —
//!   2.4–3.1× where a driver will read it, off on the Raptor Lake iGPU where
//!   the JPEG engine will not.
//! - **The evidence that playback is or is not smooth.** [`stats`] folds the
//!   per-frame numbers into one INFO line a second — frames shown, dropped,
//!   mean and p99 frame time, decode path, resolution — because the per-frame
//!   lines are DEBUG and the file log is INFO, so without it a stutter the
//!   owner reports leaves nothing on disk to investigate.
//!
//! ## What it deliberately does not own
//!
//! - **Compositing.** It drives one shared [`Compositor`] and knows nothing
//!   about how a frame is put together.
//! - **Decoding.** Pixels arrive through an injected
//!   [`SourceProvider`], which `media` implements. Until it is wired the
//!   preview renders the canvas background and the compositor's solid-colour
//!   provider fills in for it in tests.
//! - **Audio.** Audio is not part of the frame pipeline at all — it is mixed
//!   and played elsewhere, and this module will consume its position as a time
//!   source rather than produce it.
//! - **Export.** Export never touches the webview and never comes through
//!   here; it drives the same compositor headless at full resolution.
//!
//! [`Compositor`]: crate::modules::render::Compositor
//! [`SourceProvider`]: crate::modules::render::SourceProvider

pub mod cache;
pub mod clock;
pub mod commands;
pub mod encoder;
pub mod error;
pub mod ladder;
pub mod player;
pub mod probe;
pub mod server;
pub mod session;
pub mod stats;
pub mod vaapi;
pub mod vasurface;
pub mod zerocopy;

pub use cache::{CachedFrame, FrameCache, Lookup, DEFAULT_CAPACITY};
pub use clock::{
    frame_at, frame_interval, frame_start, frame_time, is_late, nearest_frame_time, pace,
    ManualSource, MonotonicSource, Pacing, PlaybackClock, TimeSource, DEFAULT_READ_AHEAD,
};
pub use encoder::{
    encode_jpeg, encode_preview_jpeg, encode_preview_jpeg_dmabuf, hardware_available, Backend,
    BACKEND_ENV,
};
pub use error::{PreviewError, Result};
pub use ladder::{Ladder, Rung, MIN_QUALITY, RUNGS, STEP_DOWN_AFTER, STEP_UP_AFTER};
pub use probe::{Counts, Probe, PROBE};
pub use server::{
    frame_url, parse_frame_uri, EventSink, PreviewEvent, PreviewInfo, PreviewServer, PreviewStatus,
    FRAME_WAIT, SCHEME,
};
pub use session::{
    preview_size, proxy_long_edge, proxy_size, PreviewOptions, PreviewSession, Viewport,
    DEFAULT_JPEG_QUALITY, SCRUB_JPEG_QUALITY,
};
pub use stats::{
    decode_path, DecodePath, Histogram, PlaybackStats, Rendered, SeekKind, SeekWatch, SessionFacts,
    SlowSeek, Summary, SLOW_SEEK, SUMMARY_INTERVAL,
};
pub use vasurface::SurfaceRing;
pub use zerocopy::{Claim, PreviewRing};
