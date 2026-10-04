//! Baking: decode a clip's source range, run each frame through a model in
//! the ML worker, write the mattes into the cache.
//!
//! Three kinds of model, three ways to walk the frames:
//!
//! - **People** (RVM) carries state from frame to frame, so a bake runs a
//!   stretch of consecutive frames and starts [`WARM_UP`] before the first
//!   missing frame so that frame's matte is as good as the ones around it.
//!   Frames already in the cache are run (the state needs them) but not
//!   rewritten.
//! - **Objects** (BiRefNet) has no state: only the missing frames are run.
//! - **Select object** (MobileSAM) propagates the user's clicks from the
//!   frame they were made on, forwards and backwards (`object.rs`).
//!
//! A bake fills the cache directory of the provider it runs on
//! (`cache.rs`, "The provider is part of the key"). It holds no lock and no
//! GPU: the worker runs the model, this thread only decodes and writes PNGs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use super::{cache, object};
use crate::modules::media::decoder::VideoDecoder;
use crate::modules::ml::matte::{self, Matter};
use crate::modules::ml::segment;
use crate::modules::project::compositing::BackgroundRemoval;
use crate::modules::project::document::{Micros, SAMPLE_SLACK};

/// How far before the first missing frame a recurrent bake starts, so the
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
    /// The clip's setting: model, version and, for "Select object", the
    /// clicks.
    pub setting: BackgroundRemoval,
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

/// The model kinds, by how a bake walks the frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Recurrent: consecutive frames with a warm-up (RVM).
    Recurrent,
    /// Frame by frame (BiRefNet).
    PerFrame,
    /// Propagated from the clicked frame (MobileSAM).
    Prompted,
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

    /// The frames of the range with no matte among `times`, as the
    /// frame-grid times the compositor will ask for. Empty when the bake is
    /// complete.
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

    /// The cache key of this job's mattes (without the provider).
    pub fn key(&self) -> Result<String, String> {
        cache::key_for(self.path.as_ref(), &self.setting)
    }

    /// The directory the compositor would draw from and its frames.
    pub fn best(&self) -> Result<Option<(PathBuf, Vec<Micros>)>, String> {
        Ok(cache::best(&self.key()?))
    }

    /// How the bake walks the frames, or why this build cannot make the
    /// matte at all: a model or a version it does not have, or a selection
    /// without clicks.
    pub fn kind(&self) -> Result<Kind, String> {
        let model = self.setting.model.as_str();
        let Some(version) = matte::version_of(model) else {
            return Err(format!(
                "this clip's background was removed with {model}, which this build of \
                 chukcut does not have; turn Remove background off and on again"
            ));
        };
        if version != self.setting.version {
            return Err(format!(
                "this clip's background was removed with {model} {}, which this build of \
                 chukcut does not have ({model} {version} is); turn Remove background off \
                 and on again to use it",
                self.setting.version
            ));
        }
        match model {
            matte::MODEL => Ok(Kind::Recurrent),
            m if m == segment::MODEL => {
                if self.setting.prompt.is_none() {
                    return Err("Select object has no clicks on this clip yet".into());
                }
                Ok(Kind::Prompted)
            }
            m if matte::is_stateless(m) => Ok(Kind::PerFrame),
            other => Err(format!("{other} does not make mattes")),
        }
    }

    /// The decoder at the matte size.
    pub(super) fn open_decoder(&self) -> Result<VideoDecoder, String> {
        let (w, h) = self.source_size;
        let long = w.max(h).max(1) as f32;
        let scale = (cache::MATTE_LONG_SIDE as f32 / long).min(1.0);
        let target_height = ((h as f32 * scale).round() as u32).max(16) & !1;
        VideoDecoder::open_scaled(&self.path, target_height)
            .map_err(|e| format!("could not open {}: {e}", self.path))
    }
}

/// Make the job's model ready in the worker and return the provider it runs
/// on. Downloads the model (and ONNX Runtime) on first use.
pub fn prepare(job: &BakeJob, cancel: &AtomicBool) -> Result<String, String> {
    let provider = match job.kind()? {
        Kind::Prompted => segment::prepare(&|_, _| {}, cancel).inspect(|_| {
            // The propagation follows the object with VitTrack, and does
            // without it (from the last mask's box) when it cannot run.
            if let Err(error) = crate::modules::ml::tracker::VitTracker::prepare(&|_, _| {}, cancel)
            {
                tracing::info!(%error, "VitTrack unavailable; propagating from the masks alone");
            }
        }),
        _ => Matter::prepare_model(&job.setting.model, &|_, _| {}, cancel),
    };
    provider.map_err(|e| e.to_string())
}

/// Bake `job`'s missing mattes. Blocking; run it on a thread of its own.
/// Makes the model ready first ([`prepare`]). `report` is called after
/// every frame. A cancel keeps every matte written so far.
pub fn run(
    job: &BakeJob,
    cancel: &AtomicBool,
    mut report: impl FnMut(&BakeProgress),
) -> Result<BakeOutcome, String> {
    let started = std::time::Instant::now();
    let kind = job.kind()?;
    let key = job.key()?;
    // Complete in some provider's directory: nothing to do, and nothing to
    // download or start.
    if cache::best(&key).is_some_and(|(_, times)| job.missing(&times).is_empty()) {
        return Ok(BakeOutcome {
            seconds: started.elapsed().as_secs_f64(),
            ..BakeOutcome::default()
        });
    }
    let provider = prepare(job, cancel)?;
    let dir = cache::dir_in(&key, &provider);
    let existing = cache::list(&dir);
    let missing = job.missing(&existing);
    if missing.is_empty() {
        return Ok(BakeOutcome {
            seconds: started.elapsed().as_secs_f64(),
            ..BakeOutcome::default()
        });
    }
    let mut outcome = match kind {
        Kind::Recurrent => run_recurrent(job, &dir, &existing, &missing, cancel, &mut report)?,
        Kind::PerFrame => run_per_frame(job, &dir, &missing, cancel, &mut report)?,
        Kind::Prompted => object::run(job, &dir, &existing, &missing, cancel, &mut report)?,
    };
    outcome.provider.get_or_insert(provider);
    outcome.seconds = started.elapsed().as_secs_f64();
    Ok(outcome)
}

fn run_recurrent(
    job: &BakeJob,
    dir: &std::path::Path,
    existing: &[Micros],
    missing: &[Micros],
    cancel: &AtomicBool,
    report: &mut dyn FnMut(&BakeProgress),
) -> Result<BakeOutcome, String> {
    let (first, last) = (missing[0], missing[missing.len() - 1]);
    let from = (first - WARM_UP).max(0);
    let mut decoder = job.open_decoder()?;
    let grid = job.frame_times(from, last);
    let mut progress = BakeProgress {
        done: 0,
        total: grid.len() as u32,
    };
    report(&progress);
    let mut matter = Matter::with_model(&job.setting.model);
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
            cache::write(dir, frame.pts, frame.width, frame.height, alpha)?;
            outcome.written += 1;
        }
        report(&progress);
    }
    outcome.provider = matter.provider.take();
    Ok(outcome)
}

fn run_per_frame(
    job: &BakeJob,
    dir: &std::path::Path,
    missing: &[Micros],
    cancel: &AtomicBool,
    report: &mut dyn FnMut(&BakeProgress),
) -> Result<BakeOutcome, String> {
    let mut decoder = job.open_decoder()?;
    let mut progress = BakeProgress {
        done: 0,
        total: missing.len() as u32,
    };
    report(&progress);
    let mut matter = Matter::with_model(&job.setting.model);
    let mut outcome = BakeOutcome::default();
    let mut last_pts = None;
    for (index, &t) in missing.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            break;
        }
        let frame = decoder
            .seek_and_decode(t + SAMPLE_SLACK)
            .map_err(|e| format!("decode failed at {t} µs: {e}"))?;
        progress.done = index as u32 + 1;
        if last_pts == Some(frame.pts) {
            continue;
        }
        last_pts = Some(frame.pts);
        let alpha = matter
            .next(&frame.data, frame.width as usize, frame.height as usize)
            .map_err(|e| e.to_string())?;
        outcome.frames += 1;
        cache::write(dir, frame.pts, frame.width, frame.height, alpha)?;
        outcome.written += 1;
        report(&progress);
    }
    outcome.provider = matter.provider.take();
    Ok(outcome)
}
