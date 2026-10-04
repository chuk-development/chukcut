//! Baking a clip's remade frames: decode the source range in order, remove
//! the object, enhance the picture, write each frame into the cache.
//!
//! Runs on a thread of its own and holds no lock and no GPU: the worker
//! runs the models, this thread decodes, builds masks, blends and writes
//! JPEGs. A bake fills the cache directory of the provider it runs on.
//!
//! Removal carries state from frame to frame (`removal::Remover`), so a bake
//! walks consecutive frames, starting [`WARM_UP`] before the first missing
//! one, as a recurrent matte bake does. Frames already in the cache are run
//! (the state needs them) but not rewritten. A clip that is only enhanced
//! has no state and runs the missing frames alone.
//!
//! "Select object" removals first make sure the selection's mattes are
//! baked (`matting::bake`, shared with Remove background › Select).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;

use super::removal::{self, Plate, PlateBuilder, Remover};
use super::{cache, mask, Chain, ObjectRemoval};
use crate::modules::matting;
use crate::modules::media::decoder::VideoDecoder;
use crate::modules::ml::{inpaint, upscale};
use crate::modules::project::document::{Micros, SAMPLE_SLACK};

/// How far before the first missing frame a removal bake starts, so the
/// background memory and the smoothing have settled when it matters.
pub const WARM_UP: Micros = 500_000;

/// One bake: which file, which part of it, what is done to it.
#[derive(Debug, Clone)]
pub struct EnhanceJob {
    pub path: String,
    pub fps: f64,
    /// Display size of the source (rotation applied).
    pub source_size: (u32, u32),
    /// Source times, inclusive at both ends.
    pub range: (Micros, Micros),
    pub chain: Chain,
}

/// How far a bake is.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnhanceProgress {
    /// What runs now: "Selecting the object", "Removing the object",
    /// "Enhancing".
    pub stage: String,
    pub done: u32,
    pub total: u32,
    /// Where the models run (`CUDA`, `CPU`), once they do.
    pub provider: Option<String>,
    /// The time left at the pace so far, once a frame has been made.
    pub seconds_left: Option<f64>,
}

impl EnhanceProgress {
    /// The sentence a user sees while the bake runs on the CPU: how long
    /// the rest will take.
    pub fn cpu_warning(&self) -> Option<String> {
        if self.provider.as_deref() != Some("CPU") || self.done >= self.total {
            return None;
        }
        let left = self.total - self.done;
        let time = match self.seconds_left {
            Some(s) => format!("about {}", super::duration_words(s)),
            None => "a while".into(),
        };
        Some(format!(
            "The AI runs on the CPU here: {left} frames, {time}. A GPU bundle \
             (Settings › AI acceleration) makes it 10–50 times faster"
        ))
    }
}

/// What a bake did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnhanceOutcome {
    pub written: u32,
    /// Frames run, warm-up and already-baked included.
    pub frames: u32,
    pub cancelled: bool,
    pub provider: Option<String>,
    pub seconds: f64,
    /// The models' own time per frame, without decoding and writing.
    pub model_millis_per_frame: Option<f32>,
}

impl EnhanceJob {
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

    /// The frame times from `from` to `to` (inclusive), each from its index
    /// (adding a rounded period drifts).
    pub fn frame_times(&self, from: Micros, to: Micros) -> Vec<Micros> {
        let fps = self.fps();
        let first = (from as f64 * fps / 1_000_000.0).floor() as i64;
        (first..)
            .map(|k| (k as f64 * 1_000_000.0 / fps).round() as Micros)
            .skip_while(|&t| t + self.period() <= from)
            .take_while(|&t| t <= to)
            .collect()
    }

    /// The frames of the range with no made frame among `times`.
    pub fn missing(&self, times: &[Micros]) -> Vec<Micros> {
        let period = self.period();
        self.frame_times(self.range.0, self.range.1)
            .into_iter()
            .filter(|&t| {
                matting::cache::lookup(times, t.max(self.range.0) + period / 2, period).is_none()
            })
            .collect()
    }

    pub fn key(&self) -> Result<String, String> {
        cache::key_for(self.path.as_ref(), &self.chain)
    }

    /// The directory the compositor would draw from and its frames.
    pub fn best(&self) -> Result<Option<(PathBuf, Vec<Micros>)>, String> {
        Ok(cache::best(&self.key()?))
    }

    /// The decode size and the output size.
    pub fn sizes(&self) -> ((u32, u32), (u32, u32)) {
        let (w, h) = self.source_size;
        (self.chain.decode_size(w, h), self.chain.out_size(w, h))
    }

    /// Why this build cannot make the frames: a model or version it does
    /// not have, a source too large to enhance.
    pub fn refusal(&self) -> Option<String> {
        if let Some(r) = &self.chain.removal {
            if r.model != inpaint::MODEL || r.version != inpaint::model_version() {
                return Some(format!(
                    "this clip's object was removed with {} {}, which this build of chukcut \
                     does not have; remove the object again",
                    r.model, r.version
                ));
            }
            if let Some(why) = r.invalid() {
                return Some(why);
            }
        }
        if let Some(u) = &self.chain.upscale {
            if u.model != upscale::MODEL || u.version != upscale::model_version() {
                return Some(format!(
                    "this clip was enhanced with {} {}, which this build of chukcut does not \
                     have; switch Enhance quality off and on again",
                    u.model, u.version
                ));
            }
            if let Some(why) = super::upscale_refusal(self.source_size.0, self.source_size.1) {
                return Some(why);
            }
        }
        None
    }

    /// The bake of the selection's mattes, for a removal with clicks.
    fn matte_job(&self) -> Option<matting::bake::BakeJob> {
        let prompt = self.chain.removal.as_ref()?.prompt.as_ref()?;
        Some(matting::bake::BakeJob {
            path: self.path.clone(),
            fps: self.fps,
            source_size: self.source_size,
            range: self.range,
            setting: super::removal_matte_setting(prompt),
        })
    }

    fn open_decoder(&self) -> Result<VideoDecoder, String> {
        let ((_, h), _) = self.sizes();
        VideoDecoder::open_scaled(&self.path, h)
            .map_err(|e| format!("could not open {}: {e}", self.path))
    }
}

/// Make every model of `job` ready; returns the provider of the last one
/// (the one whose answer the frames are), which names the directory.
pub fn prepare(job: &EnhanceJob, cancel: &AtomicBool) -> Result<String, String> {
    let mut provider = None;
    if job.chain.removal.is_some() {
        provider = Some(inpaint::prepare(&|_, _| {}, cancel).map_err(|e| e.to_string())?);
    }
    if job.chain.upscale.is_some() {
        provider = Some(upscale::prepare(&|_, _| {}, cancel).map_err(|e| e.to_string())?);
    }
    provider.ok_or_else(|| "nothing to do to this clip".to_string())
}

/// Bake `job`'s missing frames. Blocking; run it on a thread of its own.
/// `report` is called after every frame. A cancel keeps what is written.
pub fn run(
    job: &EnhanceJob,
    cancel: &AtomicBool,
    mut report: impl FnMut(&EnhanceProgress),
) -> Result<EnhanceOutcome, String> {
    let started = Instant::now();
    if let Some(why) = job.refusal() {
        return Err(why);
    }
    let key = job.key()?;
    if cache::best(&key).is_some_and(|(_, times)| job.missing(&times).is_empty()) {
        return Ok(EnhanceOutcome {
            seconds: started.elapsed().as_secs_f64(),
            ..EnhanceOutcome::default()
        });
    }
    // The selection first: its mattes are the removal's masks.
    let mut mattes = None;
    if let Some(matte_job) = job.matte_job() {
        let outcome = matting::bake::run(&matte_job, cancel, |p| {
            report(&EnhanceProgress {
                stage: "Selecting the object".into(),
                done: p.done,
                total: p.total,
                ..EnhanceProgress::default()
            })
        })
        .map_err(|e| format!("the object could not be selected: {e}"))?;
        if outcome.cancelled {
            return Ok(EnhanceOutcome {
                cancelled: true,
                seconds: started.elapsed().as_secs_f64(),
                ..EnhanceOutcome::default()
            });
        }
        mattes = matte_job.best()?;
    }
    let provider = prepare(job, cancel)?;
    let ((_, _), (ow, oh)) = job.sizes();
    let dir = cache::dir_in(&key, &provider, ow.max(oh));
    let existing = cache::list(&dir);
    let missing = job.missing(&existing);
    let mut outcome = EnhanceOutcome {
        provider: Some(provider.clone()),
        ..EnhanceOutcome::default()
    };
    if missing.is_empty() {
        outcome.seconds = started.elapsed().as_secs_f64();
        return Ok(outcome);
    }
    let stateful = job.chain.removal.is_some();
    // A removal walks every frame from before the first missing one to the
    // last; an upscale only the missing ones.
    let grid = if stateful {
        let from = (missing[0] - WARM_UP).max(job.range.0);
        job.frame_times(from, missing[missing.len() - 1])
    } else {
        missing.clone()
    };
    let stage = match (stateful, job.chain.upscale.is_some()) {
        (true, true) => "Removing the object and enhancing",
        (true, false) => "Removing the object",
        _ => "Enhancing",
    };
    let mut progress = EnhanceProgress {
        stage: stage.into(),
        done: 0,
        total: grid.len() as u32,
        provider: Some(provider),
        seconds_left: None,
    };

    let period = job.period();
    let mut masks = job.chain.removal.as_ref().map(|removal| Masks {
        removal,
        mattes,
        painted: None,
        period,
    });
    // A selected object moves: what it covers in one frame is seen in
    // another. One pass over the whole clip first keeps that background.
    let plates = match &mut masks {
        Some(masks) if masks.removal.prompt.is_some() => {
            let plates = plates(job, masks, cancel, &mut report)?;
            if cancel.load(Ordering::Relaxed) {
                outcome.cancelled = true;
                outcome.seconds = started.elapsed().as_secs_f64();
                return Ok(outcome);
            }
            plates
        }
        _ => Vec::new(),
    };
    report(&progress);

    let mut decoder = job.open_decoder()?;
    let mut remover: Option<Remover> = None;
    let mut model_millis = 0.0f64;
    let mut last_pts = None;
    let baking = Instant::now();
    for (index, &t) in grid.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            break;
        }
        let frame = decoder
            .seek_and_decode(t + SAMPLE_SLACK)
            .map_err(|e| format!("decode failed at {t} µs: {e}"))?;
        progress.done = index as u32 + 1;
        // A variable-rate file can show one frame for two grid steps.
        if last_pts == Some(frame.pts) {
            continue;
        }
        last_pts = Some(frame.pts);
        let (w, h) = (frame.width as usize, frame.height as usize);
        let mut picture = frame.data;
        if let Some(masks) = &mut masks {
            let remover = remover.get_or_insert_with(|| Remover::new(w, h, masks.removal.grow));
            let object = masks.at(frame.pts, w, h)?;
            let plate = removal::plate_at(&plates, frame.pts);
            picture = match remover.next(&picture, &object, plate, Some(cancel)) {
                Err(e) if e == "cancelled" => {
                    outcome.cancelled = true;
                    break;
                }
                other => other?,
            };
        }
        let already = existing.binary_search(&frame.pts).is_ok();
        if !already {
            if job.chain.upscale.is_some() {
                let made = upscale::upscale(&picture, w, h, ow as usize, oh as usize, Some(cancel));
                let made = match made {
                    Err(crate::modules::ml::MlError::Cancelled) => {
                        outcome.cancelled = true;
                        break;
                    }
                    other => other.map_err(|e| e.to_string())?,
                };
                model_millis += made.millis as f64;
                cache::write(&dir, frame.pts, ow, oh, &made.rgba)?;
            } else {
                cache::write(&dir, frame.pts, w as u32, h as u32, &picture)?;
            }
            outcome.written += 1;
        }
        outcome.frames += 1;
        let per_frame = baking.elapsed().as_secs_f64() / progress.done.max(1) as f64;
        progress.seconds_left = Some(per_frame * (progress.total - progress.done) as f64);
        report(&progress);
    }
    if let Some(remover) = &remover {
        model_millis += remover.model_millis;
    }
    if outcome.frames > 0 && model_millis > 0.0 {
        outcome.model_millis_per_frame = Some((model_millis / outcome.frames as f64) as f32);
    }
    outcome.seconds = started.elapsed().as_secs_f64();
    Ok(outcome)
}

/// A removal's object mask per frame: the painted part (the same in every
/// frame, made once) and the selection's matte of that frame.
struct Masks<'a> {
    removal: &'a ObjectRemoval,
    mattes: Option<(PathBuf, Vec<Micros>)>,
    painted: Option<Vec<u8>>,
    period: Micros,
}

impl Masks<'_> {
    /// The mask of the frame shown from `pts`, `w × h`.
    fn at(&mut self, pts: Micros, w: usize, h: usize) -> Result<Vec<u8>, String> {
        let removal = self.removal;
        let mut object = self
            .painted
            .get_or_insert_with(|| mask::painted(removal, w, h))
            .clone();
        if let Some((dir, times)) = &self.mattes {
            if let Some(found) = matting::cache::lookup(times, pts, self.period) {
                let (mw, mh, matte) = matting::cache::read(dir, found)?;
                mask::add_matte(&mut object, w, h, &matte, mw as usize, mh as usize);
            }
        }
        Ok(object)
    }
}

/// The first pass of a removal of a selected object: decode the whole clip
/// and keep the background of each still stretch (`removal::PlateBuilder`).
fn plates(
    job: &EnhanceJob,
    masks: &mut Masks,
    cancel: &AtomicBool,
    report: &mut impl FnMut(&EnhanceProgress),
) -> Result<Vec<Plate>, String> {
    let grid = job.frame_times(job.range.0, job.range.1);
    let mut progress = EnhanceProgress {
        stage: "Looking at the background".into(),
        done: 0,
        total: grid.len() as u32,
        ..EnhanceProgress::default()
    };
    let mut decoder = job.open_decoder()?;
    let mut builder: Option<PlateBuilder> = None;
    let mut last_pts = None;
    for (index, &t) in grid.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
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
        let (w, h) = (frame.width as usize, frame.height as usize);
        let object = masks.at(frame.pts, w, h)?;
        builder
            .get_or_insert_with(|| PlateBuilder::new(w, h, masks.removal.grow))
            .add(frame.pts, &frame.data, &object);
        if index % 10 == 0 {
            report(&progress);
        }
    }
    Ok(builder.map(PlateBuilder::finish).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(range: (Micros, Micros)) -> EnhanceJob {
        EnhanceJob {
            path: "/nowhere.mp4".into(),
            fps: 30.0,
            source_size: (640, 360),
            range,
            chain: Chain {
                removal: None,
                upscale: Some(super::super::Upscale::new(2).unwrap()),
            },
        }
    }

    #[test]
    fn the_grid_covers_the_range_and_a_hole_is_missing() {
        let j = job((0, 999_999));
        let grid = j.frame_times(0, 999_999);
        assert_eq!(grid.len(), 30);
        assert_eq!(grid[1], 33_333);
        // A range starting inside frame 3 starts with frame 3.
        let mid = j.frame_times(110_000, 200_000);
        assert_eq!(mid[0], 100_000);
        let mut times = grid.clone();
        times.remove(10);
        assert_eq!(j.missing(&times), vec![grid[10]]);
        assert!(j.missing(&grid).is_empty());
        assert_eq!(j.sizes(), ((640, 360), (1280, 720)));
    }

    #[test]
    fn the_cpu_warning_says_how_long() {
        let p = EnhanceProgress {
            stage: "Enhancing".into(),
            done: 10,
            total: 100,
            provider: Some("CPU".into()),
            seconds_left: Some(600.0),
        };
        let w = p.cpu_warning().unwrap();
        assert!(w.contains("90 frames") && w.contains("10 min"), "{w}");
        let gpu = EnhanceProgress {
            provider: Some("CUDA".into()),
            ..p
        };
        assert!(gpu.cpu_warning().is_none());
    }
}
