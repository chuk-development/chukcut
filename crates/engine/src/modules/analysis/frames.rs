//! Walking a stretch of a video file frame by frame, small.
//!
//! Every picture analysis here reads the same way: decode forwards from the
//! start of a source range, at a reduced size (swscale scales during the
//! colour conversion it does anyway, so a small frame is close to free and
//! decode stays the bottleneck), one frame per step, until the end or until
//! the user cancels. The tracker's job (`tracking/job.rs`) reads the same way.
//!
//! A compound clip has no file: its picture is its sequence rendered
//! (decision 0024). A walk over one renders the nested sequence at the
//! reduced size instead of decoding, one frame per step of the project's
//! frame rate, and reports the sequence's own time as the frame's `pts` —
//! the compound clip's source time, which is what its results are stored in.

use std::sync::Arc;

use super::jobs::{JobContext, CANCELLED};
use crate::modules::media::decoder::VideoDecoder;
use crate::modules::project::document::{Micros, Project, TimeRange, SAMPLE_SLACK};

/// One decoded frame.
pub struct Frame {
    /// Presentation time in the file.
    pub pts: Micros,
    pub width: usize,
    pub height: usize,
    /// Tightly packed RGBA8, rotation applied.
    pub rgba: Vec<u8>,
}

/// What to read.
#[derive(Debug, Clone)]
pub struct Walk {
    /// The file decoded; for a sequence, its cache identity
    /// (`sequence:<id>#<digest>`), which names no file.
    pub path: String,
    /// The file's frame rate, for stepping one frame at a time.
    pub fps: f64,
    pub range: TimeRange,
    /// Height of the decoded frames, pixels.
    pub height: u32,
    /// At most this many frames per second are read; `None` reads them all.
    pub max_rate: Option<f64>,
    /// A compound clip's contents, as the document to render
    /// (`sequence::nested`), in place of decoding `path`. Frames are as wide
    /// as its canvas's shape at `height`.
    pub sequence: Option<Arc<Project>>,
}

/// Call `visit` for each frame of `walk`, in time order, reporting progress
/// through `ctx` (scaled into `progress_span`). Frames a variable-rate file
/// repeats are skipped. Stops with [`CANCELLED`] when the user cancels.
pub fn walk(
    walk: &Walk,
    ctx: Option<&JobContext>,
    progress_span: (f32, f32),
    mut visit: impl FnMut(Frame) -> Result<(), String>,
) -> Result<(), String> {
    if let Some(view) = &walk.sequence {
        return walk_sequence(walk, view, ctx, progress_span, visit);
    }
    let mut decoder = VideoDecoder::open_scaled(&walk.path, walk.height.max(16) & !1)
        .map_err(|e| format!("could not open {}: {e}", walk.path))?;
    let fps = if walk.fps.is_finite() && walk.fps > 1.0 {
        walk.fps
    } else {
        30.0
    };
    let rate = walk.max_rate.map_or(fps, |r| r.min(fps).max(0.5));
    let period = ((1_000_000.0 / rate).round() as Micros).max(1);
    let (start, end) = (walk.range.start.max(0), walk.range.end());
    let span = (end - start).max(1) as f32;
    let mut last_pts: Option<Micros> = None;
    let mut t = start;
    while t < end {
        if ctx.is_some_and(JobContext::cancelled) {
            return Err(CANCELLED.into());
        }
        let frame = match decoder.seek_and_decode(t + SAMPLE_SLACK) {
            Ok(frame) => frame,
            // A frame past the last one the file really has: the range was
            // taken from the container's duration, which can be a frame long.
            Err(_) if last_pts.is_some() => break,
            Err(e) => return Err(format!("decode failed at {t} µs: {e}")),
        };
        t += period;
        if last_pts.is_some_and(|p| frame.pts <= p) {
            continue;
        }
        last_pts = Some(frame.pts);
        if let Some(ctx) = ctx {
            let done = (t - start) as f32 / span;
            ctx.progress(progress_span.0 + (progress_span.1 - progress_span.0) * done);
        }
        visit(Frame {
            pts: frame.pts,
            width: frame.width as usize,
            height: frame.height as usize,
            rgba: frame.data,
        })?;
    }
    Ok(())
}

/// [`walk`] over a sequence: each step renders it, whole, at the step's time.
fn walk_sequence(
    walk: &Walk,
    view: &Project,
    ctx: Option<&JobContext>,
    progress_span: (f32, f32),
    mut visit: impl FnMut(Frame) -> Result<(), String>,
) -> Result<(), String> {
    use crate::modules::media::MediaSourceProvider;
    let compositor = crate::modules::sequence::thumbs::compositor()?;
    let height = walk.height.max(16) & !1;
    let aspect = view.canvas.width.max(1) as f64 / view.canvas.height.max(1) as f64;
    let width = ((height as f64 * aspect).round() as u32).max(2) & !1;
    let fps = if walk.fps.is_finite() && walk.fps > 1.0 {
        walk.fps
    } else {
        30.0
    };
    let rate = walk.max_rate.map_or(fps, |r| r.min(fps).max(0.5));
    let period = ((1_000_000.0 / rate).round() as Micros).max(1);
    let (start, end) = (
        walk.range.start.max(0),
        walk.range.end().min(view.duration()),
    );
    let span = (end - start).max(1) as f32;
    let sources = MediaSourceProvider::from_project(view);
    let mut t = start;
    let result = (|| {
        while t < end {
            if ctx.is_some_and(JobContext::cancelled) {
                return Err(CANCELLED.to_string());
            }
            // Just inside the frame, like the export and the decoder walk.
            let frame = compositor
                .render(view, t + SAMPLE_SLACK, (width, height), &sources)
                .map_err(|e| format!("could not render the compound clip at {t} µs: {e}"))?;
            let pts = t;
            t += period;
            if let Some(ctx) = ctx {
                let done = (t - start) as f32 / span;
                ctx.progress(progress_span.0 + (progress_span.1 - progress_span.0) * done);
            }
            visit(Frame {
                pts,
                width: frame.width as usize,
                height: frame.height as usize,
                rgba: frame.data,
            })?;
        }
        Ok(())
    })();
    sources.clear();
    result
}

/// Luma (BT.601) of an RGBA frame, `0..255`.
pub fn luma(rgba: &[u8]) -> Vec<f32> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect()
}
