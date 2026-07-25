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

/// Quality for frames the user watches.
///
/// Around here is the knee of the curve: a 960x540 frame is ~120 KB, and the
/// artefacts that appear much below it are exactly the kind an editor would
/// mistake for a problem with their footage. A 1080x1920 frame at 88 is about
/// 1 MB, which the custom protocol carries at 30 fps without noticing.
pub const DEFAULT_JPEG_QUALITY: u8 = 88;

/// Quality for a frame rendered because the playhead moved — a scrub, a pause,
/// a seek, or the first frame of a session.
///
/// **Higher** than the playback quality, not lower, and the earlier reasoning
/// for the reverse was wrong. It assumed such a frame is on screen for one
/// refresh, which is true only while the pointer is moving. The moment the user
/// lets go, that exact frame is what they sit and look at — and at quality 60 a
/// dark shot shows visible JPEG blocking, which reads as the editor having
/// ruined the footage.
///
/// The cost is paid once per gesture rather than thirty times a second, so it
/// is close to free.
pub const SCRUB_JPEG_QUALITY: u8 = 94;

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
        0..=1920 => canvas_long_edge,
        _ => 1920,
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
    /// The resolution every frame is rendered at, playing or parked. Native
    /// up to 1920 on the long edge.
    ///
    /// There used to be a second, smaller size for playback, because the JPEG
    /// encode alone overran the frame budget at 1080x1920. It does not any
    /// more — the encoder moved onto the GPU and a 1080x1920 frame costs about
    /// 7 ms — so playback and stills render the same picture again. See
    /// `docs/research/vaapi-jpeg-preview.md`.
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
            // Deliberately not clamped down to `quality`: a still frame is
            // allowed to be better than a playback frame.
            scrub_quality: SCRUB_JPEG_QUALITY,
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
        // The table is keyed on the *long* edge, not on the height.
        //
        // Everything up to 1080 previews natively. That is the point of the
        // band: a 1080p project must not look softer in the editor than the
        // source file does in a player, because that is how the user finds out.
        assert_eq!(proxy_size((640, 480), None), (640, 480), "native");
        assert_eq!(proxy_size((1080, 1080), None), (1080, 1080), "native");
        assert_eq!(proxy_size((1920, 1080), None), (1920, 1080), "1080p landscape is native");
        assert_eq!(proxy_size((3840, 2160), None), (1920, 1080), "4K previews at 1080p");
        assert_eq!(proxy_size((7680, 4320), None), (1920, 1080), "8K previews at 1080p");
    }

    #[test]
    fn portrait_canvases_cap_the_long_edge_too() {
        // 9:16, the default canvas. The cap applies to height, not width, so a
        // 1080x1920 project is 1920 on its long edge and therefore *past* the
        // native band even though it is "1080p" in ordinary speech.
        // 1080x1920 is 1920 on its long edge, so it previews natively — the
        // case the whole band exists for.
        assert_eq!(proxy_size((1080, 1920), None), (1080, 1920));
        assert_eq!(proxy_size((2160, 3840), None), (1080, 1920));
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
