//! Baking: decode a clip's source range in order, run each frame through the
//! matting model in the ML worker, write the mattes into the cache.
//!
//! RVM carries state from frame to frame, so a bake always runs a stretch of
//! consecutive frames, and starts [`WARM_UP`] before the first missing frame
//! so that frame's matte is as good as the ones around it. Frames already in
//! the cache are run (the state needs them) but not rewritten. A bake holds
//! no lock and no GPU: the worker runs the model, this thread only decodes
//! and writes PNGs.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use super::cache;
use crate::modules::media::decoder::VideoDecoder;
use crate::modules::ml::matte::{self, Matter};
use crate::modules::project::document::{Micros, SAMPLE_SLACK};

/// How far before the first missing frame a bake starts, so the recurrent
/// state has settled by the time it matters. RVM's mattes stop changing
/// after about ten frames on a cold start.
pub const WARM_UP: Micros = 500_000;

/// One bake: which file, which part of it, with which model.
#[derive(Debug, Clone)]
pub struct BakeJob {
    pub path: String,
    pub fps: f64,
    /// Display size of the source (rotation applied).
    pub source_size: (u32, u32),
    /// Source times, inclusive at both ends.
    pub range: (Micros, Micros),
    pub model: String,
    pub version: String,
}

/// How far a bake is, in frames of the stretch it runs.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BakeProgress {
    pub done: u32,
    pub total: u32,
}

/// What a bake did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BakeOutcome {
    /// Mattes written now.
    pub written: u32,
    /// Frames run through the model, warm-up and already-baked included.
    pub frames: u32,
    pub cancelled: bool,
    /// Where the model ran (`CUDA`, `OpenVINO`, `CPU`); `None` when nothing
    /// needed baking.
    pub provider: Option<String>,
    pub seconds: f64,
}

impl BakeJob {
    fn fps(&self) -> f64 {
        if self.fps.is_finite() && self.fps > 1.0 {
            self.fps
        } else {
            30.0
        }
    }

    /// Frame period of the source, never zero.
    pub fn period(&self) -> Micros {
        ((1_000_000.0 / self.fps()).round() as Micros).max(1)
    }

    /// The frame times from `from` to `to` (inclusive), each one computed
    /// from its index rather than by adding a rounded period, which drifts:
    /// at 30 fps thirty rounded periods are 999 990 µs, and a one-second
    /// clip seemed to have a 31st frame.
    pub fn frame_times(&self, from: Micros, to: Micros) -> Vec<Micros> {
        let fps = self.fps();
        (0..)
            .map(|k: i64| from + (k as f64 * 1_000_000.0 / fps).round() as Micros)
            .take_while(|&t| t <= to)
            .collect()
    }

    /// The frames of the range with no matte in the cache, as the frame-grid
    /// times the compositor will ask for. Empty when the bake is complete.
    pub fn missing(&self, times: &[Micros]) -> Vec<Micros> {
        let period = self.period();
        // Asked in the middle of the frame's time, where the gap to the
        // previous frame's matte is half a period when this frame has one
        // and a period and a half when it does not.
        self.frame_times(self.range.0, self.range.1)
            .into_iter()
            .filter(|&t| cache::lookup(times, t + period / 2, period).is_none())
            .collect()
    }
}

/// Bake `job`'s missing mattes. Blocking; run it on a thread of its own.
/// The ML worker must be ready (`Matter::prepare`). `report` is called after
/// every frame. A cancel keeps every matte written so far.
pub fn run(
    job: &BakeJob,
    cancel: &AtomicBool,
    mut report: impl FnMut(&BakeProgress),
) -> Result<BakeOutcome, String> {
    let started = std::time::Instant::now();
    if job.model != matte::MODEL || job.version != matte::model_version() {
        return Err(format!(
            "this clip's background was removed with {} {}, which this build of chukcut does \
             not have ({} {} is); turn Remove background off and on again to use it",
            job.model,
            job.version,
            matte::MODEL,
            matte::model_version()
        ));
    }
    let dir = cache::dir_for(job.path.as_ref(), &job.model, &job.version)?;
    let existing = cache::list(&dir);
    let missing = job.missing(&existing);
    let (Some(&first), Some(&last)) = (missing.first(), missing.last()) else {
        return Ok(BakeOutcome {
            seconds: started.elapsed().as_secs_f64(),
            ..BakeOutcome::default()
        });
    };
    let from = (first - WARM_UP).max(0);
    let to = last;

    let (w, h) = job.source_size;
    let long = w.max(h).max(1) as f32;
    let scale = (cache::MATTE_LONG_SIDE as f32 / long).min(1.0);
    let target_height = ((h as f32 * scale).round() as u32).max(16) & !1;
    let mut decoder = VideoDecoder::open_scaled(&job.path, target_height)
        .map_err(|e| format!("could not open {}: {e}", job.path))?;

    let grid = job.frame_times(from, to);
    let mut progress = BakeProgress {
        done: 0,
        total: grid.len() as u32,
    };
    report(&progress);
    let mut matter = Matter::new();
    let mut outcome = BakeOutcome::default();
    let mut last_pts = None;
    for (index, &t) in grid.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            break;
        }
        let frame = decoder
            .seek_and_decode(t + SAMPLE_SLACK)
            .map_err(|e| format!("decode failed at {t} µs: {e}"))?;
        progress.done = index as u32 + 1;
        // A variable-rate file can show one frame for two grid steps; run
        // the model once per real frame, or the state sees a frozen picture.
        if last_pts == Some(frame.pts) {
            continue;
        }
        last_pts = Some(frame.pts);
        let alpha = matter
            .next(&frame.data, frame.width as usize, frame.height as usize)
            .map_err(|e| e.to_string())?;
        outcome.frames += 1;
        if existing.binary_search(&frame.pts).is_err() {
            cache::write(&dir, frame.pts, frame.width, frame.height, alpha)?;
            outcome.written += 1;
        }
        report(&progress);
    }
    outcome.provider = matter.provider.take();
    outcome.seconds = started.elapsed().as_secs_f64();
    Ok(outcome)
}
