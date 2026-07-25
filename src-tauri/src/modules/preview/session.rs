//! A preview session: one agreement about what is being rendered and how.
//!
//! A session bundles the things every frame in a run shares — the project
//! snapshot, the proxy resolution, the JPEG quality — behind one monotonic id.
//!
//! The id is the point. Rendering is asynchronous and the playhead is not: by
//! the time a frame comes back the user has usually moved on, and a frame
//! addressed only by its number would happily paint the old position over the
//! new one. Every frame URL carries the session it was rendered for, the cache
//! holds frames for exactly one session, and anything addressed to a session
//! that is no longer live is refused rather than shown. That is the whole
//! mechanism for "a superseded seek cannot overwrite the current view".
//!
//! Sessions are cheap: the project is behind an `Arc`, so superseding one is an
//! id bump and a ring-buffer clear, not a copy of the document.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::modules::project::document::{Micros, Project};

/// Quality for frames the user watches. 80 is the knee of the curve: a 960x540
/// frame is ~120 KB, and the artefacts that appear below it are exactly the
/// kind an editor would mistake for a problem with their footage.
pub const DEFAULT_JPEG_QUALITY: u8 = 80;

/// Quality while dragging the playhead. Lower, because during a scrub the frame
/// is on screen for one refresh and latency is the only thing that matters.
pub const SCRUB_JPEG_QUALITY: u8 = 60;

/// Fallback when a project carries a nonsense frame rate.
pub const FALLBACK_FPS: f64 = 30.0;

/// Session ids never repeat within a process run. Starts at 1 so that 0 can
/// mean "no session" in the cache and in the IPC surface.
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_session_id() -> u64 {
    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
}

/// The cap on the preview's long edge for a given canvas long edge.
///
/// Straight from the table in `docs/architecture/preview-pipeline.md`. The
/// steps are deliberately coarse: the preview only has to be good enough to
/// make editing decisions on, and every pixel above that is throughput the
/// playback clock has to pay for on every single frame.
pub fn proxy_long_edge(canvas_long_edge: u32) -> u32 {
    match canvas_long_edge {
        0..=720 => canvas_long_edge,
        721..=1080 => 720,
        1081..=2160 => 960,
        _ => 1080,
    }
}

/// Proxy resolution for a canvas, aspect preserved.
///
/// The short edge is scaled by the same factor as the long one and rounded, so
/// the preview is the export scaled down rather than a differently framed
/// composition. `cap` overrides the table — that is the "full quality preview"
/// toggle, which raises the cap and accepts the frame rate hit.
pub fn proxy_size(canvas: (u32, u32), cap: Option<u32>) -> (u32, u32) {
    let width = canvas.0.max(1);
    let height = canvas.1.max(1);
    let long = width.max(height);
    let short = width.min(height);

    let cap = cap.unwrap_or_else(|| proxy_long_edge(long)).max(1);
    if cap >= long {
        return (width, height);
    }

    let scaled_short = ((short as f64 * cap as f64 / long as f64).round() as u32).max(1);
    if width >= height {
        (cap, scaled_short)
    } else {
        (scaled_short, cap)
    }
}

/// What the frontend may override about a session.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOptions {
    /// Raise (or lower) the proxy cap on the long edge. `None` uses the table.
    #[serde(default)]
    pub long_edge: Option<u32>,
    #[serde(default)]
    pub quality: Option<u8>,
}

/// One run of the preview pipeline.
#[derive(Debug, Clone)]
pub struct PreviewSession {
    pub id: u64,
    /// Proxy resolution, already clamped to what the device can allocate.
    pub size: (u32, u32),
    pub quality: u8,
    pub scrub_quality: u8,
    /// The document as it was when the session opened. Rendering from a
    /// snapshot rather than the live document is what lets the render thread
    /// run for tens of milliseconds without holding the project lock.
    pub project: Arc<Project>,
    /// Timeline frame rate, sanitized.
    pub fps: f64,
    pub duration: Micros,
}

impl PreviewSession {
    pub fn new(project: Arc<Project>, options: PreviewOptions) -> Self {
        let size = proxy_size(
            (project.canvas.width, project.canvas.height),
            options.long_edge,
        );
        let quality = options.quality.unwrap_or(DEFAULT_JPEG_QUALITY).clamp(1, 100);
        Self {
            id: next_session_id(),
            size,
            quality,
            scrub_quality: SCRUB_JPEG_QUALITY.min(quality),
            fps: sane_fps(project.fps),
            duration: project.duration(),
            project,
        }
    }

    /// The same session with a fresh id.
    ///
    /// This is what a seek does: the document and the proxy resolution have not
    /// changed, but every frame rendered for the old id is now about the wrong
    /// position and must not be servable.
    pub fn superseded(&self) -> Self {
        Self {
            id: next_session_id(),
            project: Arc::clone(&self.project),
            ..self.clone()
        }
    }

    /// Replace the proxy size with one the device will actually accept.
    pub fn with_size(mut self, size: (u32, u32)) -> Self {
        self.size = size;
        self
    }

    pub fn width(&self) -> u32 {
        self.size.0
    }

    pub fn height(&self) -> u32 {
        self.size.1
    }
}

/// A frame rate we are willing to divide by.
pub fn sane_fps(fps: f64) -> f64 {
    if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        FALLBACK_FPS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::CanvasConfig;

    #[test]
    fn proxy_resolution_follows_the_table() {
        // The table is keyed on the *long* edge, not on the height: a 720p
        // canvas is 1280 across, which is already past the native band.
        assert_eq!(proxy_size((640, 480), None), (640, 480), "<= 720: native");
        assert_eq!(proxy_size((720, 720), None), (720, 720), "<= 720: native");
        assert_eq!(proxy_size((1080, 1080), None), (720, 720), "<= 1080: 720");
        assert_eq!(proxy_size((1280, 720), None), (960, 540), "<= 2160: 960");
        assert_eq!(proxy_size((1920, 1080), None), (960, 540), "<= 2160: 960");
        assert_eq!(proxy_size((3840, 2160), None), (1080, 608), "> 2160: 1080");
        assert_eq!(proxy_size((7680, 4320), None), (1080, 608), "> 2160: 1080");
    }

    #[test]
    fn portrait_canvases_cap_the_long_edge_too() {
        // 9:16, the default canvas. The cap applies to height, not width.
        assert_eq!(proxy_size((1080, 1920), None), (540, 960));
        assert_eq!(proxy_size((2160, 3840), None), (608, 1080));
    }

    #[test]
    fn proxy_resolution_preserves_aspect_ratio() {
        for canvas in [(1920, 1080), (1080, 1920), (3840, 2160), (2048, 858)] {
            let (w, h) = proxy_size(canvas, None);
            let want = canvas.0 as f64 / canvas.1 as f64;
            let got = w as f64 / h as f64;
            assert!(
                (want - got).abs() / want < 0.01,
                "{canvas:?} -> {w}x{h}: aspect {got} not within 1% of {want}"
            );
        }
    }

    #[test]
    fn an_override_raises_the_cap() {
        // The "full quality preview" toggle: a cap at or above the long edge
        // renders native.
        assert_eq!(proxy_size((1920, 1080), Some(1920)), (1920, 1080));
        assert_eq!(proxy_size((1920, 1080), Some(4096)), (1920, 1080));
        assert_eq!(proxy_size((1920, 1080), Some(480)), (480, 270));
    }

    #[test]
    fn degenerate_canvases_do_not_produce_zero_sized_frames() {
        assert_eq!(proxy_size((0, 0), None), (1, 1));
        // 4000:1 scaled to a 1080 long edge would round the short edge to zero.
        let (w, h) = proxy_size((8000, 2), None);
        assert!(w >= 1 && h >= 1, "got {w}x{h}");
    }

    #[test]
    fn superseding_changes_only_the_id() {
        let project = Arc::new(Project::new("t", CanvasConfig::default(), 30.0));
        let a = PreviewSession::new(project, PreviewOptions::default());
        let b = a.superseded();
        assert_ne!(a.id, b.id);
        assert!(b.id > a.id, "ids are monotonic");
        assert_eq!(a.size, b.size);
        assert_eq!(a.quality, b.quality);
        assert!(Arc::ptr_eq(&a.project, &b.project), "snapshot is shared");
    }

    #[test]
    fn a_nonsense_frame_rate_falls_back() {
        let mut project = Project::new("t", CanvasConfig::default(), 0.0);
        project.fps = f64::NAN;
        let s = PreviewSession::new(Arc::new(project), PreviewOptions::default());
        assert_eq!(s.fps, FALLBACK_FPS);
    }
}
