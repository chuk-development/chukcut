//! Walking a stretch of a video file frame by frame, small.
//!
//! Every picture analysis here reads the same way: decode forwards from the
//! start of a source range, at a reduced size (swscale scales during the
//! colour conversion it does anyway, so a small frame is close to free and
//! decode stays the bottleneck), one frame per step, until the end or until
//! the user cancels. The tracker's job (`tracking/job.rs`) reads the same way.

use super::jobs::{JobContext, CANCELLED};
use crate::modules::media::decoder::VideoDecoder;
use crate::modules::project::document::{Micros, TimeRange, SAMPLE_SLACK};

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
    pub path: String,
    /// The file's frame rate, for stepping one frame at a time.
    pub fps: f64,
    pub range: TimeRange,
    /// Height of the decoded frames, pixels.
    pub height: u32,
    /// At most this many frames per second are read; `None` reads them all.
    pub max_rate: Option<f64>,
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

/// Luma (BT.601) of an RGBA frame, `0..255`.
pub fn luma(rgba: &[u8]) -> Vec<f32> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect()
}
