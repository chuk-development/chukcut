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
//! - **The proxy resolution.** Derived from the project canvas so the aspect
//!   ratio matches the export exactly, capped on the long edge
//!   ([`session::proxy_size`]). Full resolution is an export concern; the
//!   preview only has to be good enough to make editing decisions on.
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
//!   late; time is never stretched to let the renderer catch up.
//! - **JPEG encoding.** The compositor already hands back sRGB-encoded bytes,
//!   so [`encoder::encode_jpeg`] is a pure re-encode with no colour conversion.
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
pub mod server;
pub mod session;

pub use cache::{CachedFrame, FrameCache, Lookup, DEFAULT_CAPACITY};
pub use clock::{
    frame_at, frame_interval, frame_time, is_late, pace, ManualSource, MonotonicSource, Pacing,
    PlaybackClock, TimeSource, DEFAULT_READ_AHEAD,
};
pub use encoder::encode_jpeg;
pub use error::{PreviewError, Result};
pub use server::{
    frame_protocol, frame_protocol_async, frame_url, parse_frame_uri, PreviewEvent, PreviewInfo,
    PreviewServer, PreviewStatus, FRAME_WAIT, SCHEME,
};
pub use session::{
    proxy_long_edge, proxy_size, PreviewOptions, PreviewSession, DEFAULT_JPEG_QUALITY,
    SCRUB_JPEG_QUALITY,
};
