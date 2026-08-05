//! GPU compositor.
//!
//! Turns "project + timeline position" into one composited RGBA frame using
//! wgpu. Owns the device, the texture pool, and the per-frame render graph:
//! for each visible segment, upload/lookup its source texture, apply transform,
//! crop, opacity and blend it onto the canvas target.
//!
//! Deliberately knows nothing about playback, encoding or IPC — it renders one
//! frame when asked. `preview` and `export` both drive it.
//!
//! ## The shape of it
//!
//! ```text
//!   Project + Micros + (w, h) ──► Compositor::render_frame ──► Vec<u8> RGBA8
//!                                        │
//!                                        │ "material X at source time T"
//!                                        ▼
//!                                  SourceProvider   ← implemented by `media`
//! ```
//!
//! One `RenderContext` holds the device. One `Compositor` holds the pipeline,
//! the sampler and the [`TexturePool`]. Both are `Sync`, so the exporter's
//! worker and the preview's frame server can share a single instance instead of
//! opening two GPU devices.
//!
//! ## What it owns
//!
//! - **The device.** Adapter selection, headless. Vulkan where it exists, GL
//!   where it does not, software last. There is no surface anywhere in here.
//! - **Texture recycling.** Frames are the same size for minutes at a time, so
//!   nearly every allocation is a repeat; the pool turns them into hits.
//! - **Geometry.** Normalized transforms, source-space crops and keyframes
//!   become one 4x4 matrix and one UV rectangle per segment, on the CPU, in
//!   [`layout`] — pure functions that are tested without a GPU because that
//!   maths is where a compositor is most likely to be quietly wrong.
//! - **Compositing.** Painter's algorithm in `render_index` order with
//!   straight-alpha source-over blending, onto a background from
//!   `project.canvas`.
//!
//! ## What it deliberately does not own
//!
//! - **Decoding.** It never opens a file. Pixels arrive through
//!   [`SourceProvider`], which `media` implements; this module ships a
//!   solid-colour implementation so the compositor is testable standalone.
//! - **Playback.** No clock, no ring buffer, no read-ahead, no notion of "the
//!   next frame". It renders the instant it was handed.
//! - **Encoding and delivery.** It hands back bytes. JPEG for the preview and
//!   H.264 for the export happen elsewhere; see
//!   `docs/architecture/preview-pipeline.md`.
//! - **Effects.** `Segment::extras` is not read yet. Effects are a shader
//!   runtime in the `effects` module and will hook in as extra passes around
//!   this one, not as special cases inside it.
//! - **Audio.** Audio-kind tracks and the `Volume` keyframe property are
//!   skipped. A muted track still draws: muting silences a lane, `hidden` is
//!   what conceals it.

pub mod compositor;
pub mod context;
/// DMA-BUF export. Linux only: everything in it is a DRM concept.
#[cfg(target_os = "linux")]
pub mod dmabuf;
pub mod error;
pub mod layout;
pub mod lut;
pub mod nv12;
pub mod source;
pub mod texture_pool;

pub use compositor::{Compositor, CompositorConfig, Frame, RenderStats};
pub use context::RenderContext;
pub use error::{RenderError, Result};
pub use layout::{
    animated_transform, crop_uv, fit_size, place_quad, track_is_visible, visible_segments,
    QuadPlacement,
};
pub use nv12::{Nv12Converter, Nv12Frame, Nv12Layout, Nv12PlaneWriter};
pub use source::{
    EmptySourceProvider, FrameGuard, SolidColorProvider, SolidSource, SourceFrame, SourceProvider,
    SourceRequest, YuvMatrix, YuvRange,
};
pub use texture_pool::{PoolStats, PooledTexture, TextureKey, TexturePool};

/// The one GPU device, or `None` on a machine with no adapter.
///
/// A name kept for the tests that read well with it; the device itself is
/// [`crate::modules::gpu::render_context`]'s, which is the same one the
/// application runs on. Tests that each opened their own had a dozen devices
/// alive at once under `cargo test`'s default parallelism, and creating and
/// tearing down that many concurrently segfaults inside the Mesa driver often
/// enough to make the suite unreliable — a failure that says nothing about the
/// code under test.
#[cfg(test)]
pub(crate) fn test_context() -> Option<std::sync::Arc<RenderContext>> {
    crate::modules::gpu::render_context()
}
