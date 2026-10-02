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

/// The size of the widget the frames are painted into, in **device** pixels.
///
/// Device and not CSS pixels: the frontend multiplies by `devicePixelRatio`
/// before sending it, because a 700 px canvas on a 2× display really does show
/// 1400 columns and rendering 700 of them would be visibly soft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// Round down to an even number, never below 2.
///
/// Odd dimensions are not a rounding detail here: NV12 has no way to represent
/// them, so `vaapi::size_is_encodable` refuses them and every frame falls back
/// to libjpeg-turbo — 31 ms against 6.2 ms on a 1080x1920 frame, which is the
/// difference between fitting the budget and not. A preview that got one pixel
/// narrower and five times more expensive is the kind of regression nobody
/// thinks to look for.
fn even(value: u32) -> u32 {
    (value & !1).max(2)
}

/// What the preview actually renders at.
///
/// Three inputs, in order of precedence:
///
/// - `full_quality` — the settings toggle. Renders the canvas, whatever it
///   costs and whatever the panel is. This is the escape hatch, and the only
///   way to ask for pixels the screen cannot show.
/// - `cap` — `settings.preview_max_edge`, a hard cap on the long edge. `None`
///   uses the table in [`proxy_long_edge`].
/// - `viewport` — the size of the widget on screen. **The default, and the
///   whole point.** A 1920x1080 project shown in a 700 px panel was being
///   composited, read back and JPEG-encoded at 7.5× the pixels the screen could
///   display — measured at 3.1× the frame cost, since the decode is a fixed
///   charge that no render size reduces (`examples/preview_waste.rs`).
///
/// Never larger than the canvas: rendering above the source resolution invents
/// detail and costs the frame budget to do it.
pub fn preview_size(
    canvas: (u32, u32),
    viewport: Option<Viewport>,
    cap: Option<u32>,
    full_quality: bool,
) -> (u32, u32) {
    let canvas = (canvas.0.max(1), canvas.1.max(1));
    if full_quality {
        return canvas;
    }

    let base = proxy_size(canvas, cap);
    let Some(viewport) = viewport else {
        return base;
    };
    // A viewport that has not been laid out yet says nothing; a zero here would
    // otherwise scale the preview to one pixel until the first resize.
    if viewport.width == 0 || viewport.height == 0 {
        return base;
    }

    let scale = f64::min(
        viewport.width as f64 / base.0 as f64,
        viewport.height as f64 / base.1 as f64,
    );
    if scale >= 1.0 {
        return base;
    }
    (
        even((base.0 as f64 * scale).round() as u32),
        even((base.1 as f64 * scale).round() as u32),
    )
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
///
/// All four are persisted settings except `viewport`, which is measured from
/// the player panel on every layout change. The settings are overrides; the
/// viewport is the default that decides the size when nothing overrides it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOptions {
    /// Raise (or lower) the proxy cap on the long edge. `None` uses the table.
    /// `settings.preview_max_edge`.
    #[serde(default)]
    pub long_edge: Option<u32>,
    /// `settings.preview_quality`, the JPEG quality of a playback frame.
    #[serde(default)]
    pub quality: Option<u8>,
    /// The panel the frames are painted into, in device pixels.
    #[serde(default)]
    pub viewport: Option<Viewport>,
    /// `settings.preview_full_quality`: render the canvas, ignore the panel.
    #[serde(default)]
    pub full_quality: bool,
}

impl PreviewOptions {
    /// The same options looking at a different-sized panel.
    pub fn with_viewport(mut self, viewport: Option<Viewport>) -> Self {
        self.viewport = viewport;
        self
    }

    /// What a session opened with these options renders at.
    pub fn size_for(&self, canvas: (u32, u32)) -> (u32, u32) {
        preview_size(canvas, self.viewport, self.long_edge, self.full_quality)
    }
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
    /// What this session was opened with, kept so a panel resize can recompute
    /// the size without the frontend having to repeat the settings.
    pub options: PreviewOptions,
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
        let size = options.size_for((project.canvas.width, project.canvas.height));
        let quality = options
            .quality
            .unwrap_or(DEFAULT_JPEG_QUALITY)
            .clamp(1, 100);
        Self {
            id: next_session_id(),
            size,
            quality,
            // Deliberately never *below* `quality`: a still frame is allowed to
            // be better than a playback frame and must never be worse, which is
            // the whole of the owner's requirement that a paused frame look
            // like the project it is.
            scrub_quality: SCRUB_JPEG_QUALITY.max(quality),
            options,
            fps: sane_fps(project.fps),
            duration: project.duration(),
            project,
        }
    }

    /// The same session against a differently sized panel, with a fresh id.
    ///
    /// Returns `None` when the size does not change, which is most resize
    /// events: a window drag is hundreds of pixel-by-pixel layouts and only a
    /// few of them cross a rounding boundary. Restarting the pipeline for the
    /// rest would throw the ring away thirty times a second.
    pub fn with_viewport(&self, viewport: Option<Viewport>) -> Option<Self> {
        let options = self.options.with_viewport(viewport);
        let size = options.size_for((self.project.canvas.width, self.project.canvas.height));
        if size == self.size {
            return None;
        }
        Some(Self {
            id: next_session_id(),
            size,
            options,
            project: Arc::clone(&self.project),
            ..self.clone()
        })
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
        assert_eq!(
            proxy_size((1920, 1080), None),
            (1920, 1080),
            "1080p landscape is native"
        );
        assert_eq!(
            proxy_size((3840, 2160), None),
            (1920, 1080),
            "4K previews at 1080p"
        );
        assert_eq!(
            proxy_size((7680, 4320), None),
            (1920, 1080),
            "8K previews at 1080p"
        );
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

    // -----------------------------------------------------------------------
    // The panel decides the size
    // -----------------------------------------------------------------------

    #[test]
    fn a_1080p_project_in_a_700_pixel_panel_renders_at_the_panel() {
        // The measurement this whole change exists for: 1920x1080 shown in a
        // 700x394 CSS panel is 2.07 megapixels rendered for the 0.28 the screen
        // can show — 7.5× the work, on every stage of the frame.
        let panel = Viewport::new(700, 394);
        let size = preview_size((1920, 1080), Some(panel), None, false);
        assert!(
            size.0 <= 700 && size.1 <= 394,
            "{size:?} does not fit the panel"
        );
        assert_eq!(size, (700, 394));

        let full = 1920u64 * 1080;
        let fitted = size.0 as u64 * size.1 as u64;
        assert!(
            full / fitted >= 7,
            "{fitted} px against {full}: the saving is the point"
        );
    }

    #[test]
    fn a_high_dpi_panel_gets_the_pixels_it_really_has() {
        // 700 CSS pixels on a 2× display is 1400 columns of real pixels, and
        // rendering 700 of them would be visibly soft. The frontend multiplies
        // by devicePixelRatio, so this only has to not cap it.
        assert_eq!(
            preview_size((1920, 1080), Some(Viewport::new(1400, 788)), None, false),
            (1400, 788)
        );
    }

    #[test]
    fn the_preview_is_never_rendered_above_the_source() {
        // A panel bigger than the canvas — a maximised window on a 4K screen
        // showing a 720p project — must not upscale. Upscaling invents no
        // detail and costs the whole frame budget to do it.
        assert_eq!(
            preview_size((1280, 720), Some(Viewport::new(3840, 2160)), None, false),
            (1280, 720)
        );
        assert_eq!(
            preview_size((1920, 1080), Some(Viewport::new(1921, 1081)), None, false),
            (1920, 1080)
        );
    }

    #[test]
    fn the_fitted_size_keeps_the_aspect_ratio_and_stays_even() {
        // Odd dimensions cost five times the encode: NV12 cannot represent them
        // and every frame falls back to libjpeg-turbo.
        for canvas in [(1920, 1080), (1080, 1920), (3840, 2160), (2048, 858)] {
            for panel in [(701, 395), (333, 999), (1237, 601)] {
                let size = preview_size(canvas, Some(Viewport::new(panel.0, panel.1)), None, false);
                assert_eq!(size.0 % 2, 0, "{canvas:?} in {panel:?} -> {size:?} is odd");
                assert_eq!(size.1 % 2, 0, "{canvas:?} in {panel:?} -> {size:?} is odd");
                assert!(
                    size.0 <= panel.0 && size.1 <= panel.1,
                    "{size:?} overflows {panel:?}"
                );
                let want = canvas.0 as f64 / canvas.1 as f64;
                let got = size.0 as f64 / size.1 as f64;
                assert!(
                    (want - got).abs() / want < 0.02,
                    "{canvas:?} in {panel:?} -> {size:?}: aspect {got} not near {want}"
                );
            }
        }
    }

    #[test]
    fn the_settings_are_overrides_and_full_quality_wins() {
        let panel = Some(Viewport::new(700, 394));
        // `preview_max_edge` caps below the panel.
        assert_eq!(
            preview_size((1920, 1080), panel, Some(480), false),
            (480, 270)
        );
        // …and does not raise the preview above the panel, because the panel
        // still cannot show more than it has.
        assert_eq!(
            preview_size((1920, 1080), panel, Some(1920), false),
            (700, 394)
        );
        // "Full quality preview" is the escape hatch: the canvas, whatever the
        // panel is and whatever the cap says.
        assert_eq!(
            preview_size((1920, 1080), panel, Some(480), true),
            (1920, 1080)
        );
        assert_eq!(preview_size((3840, 2160), panel, None, true), (3840, 2160));
    }

    #[test]
    fn no_viewport_leaves_the_old_table_in_charge() {
        // Before the first layout, and for any caller that does not measure a
        // panel at all, nothing changes.
        assert_eq!(
            preview_size((1920, 1080), None, None, false),
            proxy_size((1920, 1080), None)
        );
        assert_eq!(preview_size((3840, 2160), None, None, false), (1920, 1080));
        // A panel that has not been laid out yet is not a request for a
        // one-pixel preview.
        assert_eq!(
            preview_size((1920, 1080), Some(Viewport::new(0, 0)), None, false),
            (1920, 1080)
        );
    }

    #[test]
    fn a_resize_that_does_not_change_the_size_does_not_supersede_the_session() {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.canvas.width = 1080;
        project.canvas.height = 1920;
        let session = PreviewSession::new(
            Arc::new(project),
            PreviewOptions::default().with_viewport(Some(Viewport::new(360, 640))),
        );
        assert_eq!(session.size, (360, 640));

        // A one-pixel drag that rounds to the same even size is not a reason to
        // throw the ring away.
        assert!(session
            .with_viewport(Some(Viewport::new(361, 641)))
            .is_none());
        let bigger = session
            .with_viewport(Some(Viewport::new(540, 960)))
            .expect("a real resize");
        assert_eq!(bigger.size, (540, 960));
        assert!(bigger.id > session.id, "a new size is a new session");
        assert_eq!(bigger.quality, session.quality);
        assert!(
            Arc::ptr_eq(&bigger.project, &session.project),
            "snapshot is shared"
        );
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
