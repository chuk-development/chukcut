//! Baking optical-flow frames: decode each pair of source frames once, ask
//! the ML worker for every phase wanted between them, write the frames.
//!
//! Runs on a thread of its own and holds no lock and no GPU: the worker runs
//! the model, this thread decodes and writes JPEGs. A bake fills the cache
//! directory of the provider it runs on (`flow/mod.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;

use super::FlowSample;
use crate::modules::media::decoder::{DecodedFrame, VideoDecoder};
use crate::modules::ml::interpolate;
use crate::modules::project::document::{Micros, SAMPLE_SLACK};

/// One bake: which file, and which in-between frames of it.
#[derive(Debug, Clone)]
pub struct FlowJob {
    pub path: String,
    pub fps: f64,
    /// Display size of the source (rotation applied).
    pub source_size: (u32, u32),
    pub samples: BTreeSet<FlowSample>,
}

/// How far a bake is, in frames to make, and what it expects to take.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FlowProgress {
    pub done: u32,
    pub total: u32,
    /// Where the model runs (`CUDA`, `CPU`), once it does.
    pub provider: Option<String>,
    /// The time left at the pace so far, once a pair has been made.
    pub seconds_left: Option<f64>,
}

impl FlowProgress {
    /// The sentence a user sees when the bake runs on the CPU, which is
    /// slow enough to deserve one: how long the rest will take.
    pub fn cpu_warning(&self) -> Option<String> {
        if self.provider.as_deref() != Some("CPU") || self.done >= self.total {
            return None;
        }
        let left = self.total - self.done;
        let time = match self.seconds_left {
            Some(s) if s >= 90.0 => format!("about {} min", (s / 60.0).round() as u64),
            Some(s) => format!("about {} s", s.round().max(1.0) as u64),
            None => "a while".into(),
        };
        Some(format!(
            "Optical flow runs on the CPU here: {left} frames, {time}. A GPU bundle \
             (Settings › AI acceleration) makes it 10–30 times faster"
        ))
    }
}

/// What a bake did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FlowOutcome {
    pub written: u32,
    pub cancelled: bool,
    pub provider: Option<String>,
    pub seconds: f64,
    /// The worker's time per frame, without decoding and writing.
    pub model_millis_per_frame: Option<f32>,
}

impl FlowJob {
    fn fps(&self) -> f64 {
        if self.fps.is_finite() && self.fps > 1.0 {
            self.fps
        } else {
            30.0
        }
    }

    /// The middle of source frame `index`, which a decode asks for.
    fn middle(&self, index: i64) -> Micros {
        ((index as f64 + 0.5) * 1_000_000.0 / self.fps()).round() as Micros
    }

    /// The long side the frames are made at.
    pub fn long_side(&self) -> u32 {
        super::long_side(self.source_size.0, self.source_size.1)
    }

    pub fn key(&self) -> Result<String, String> {
        super::key_for(self.path.as_ref())
    }

    /// The directory the compositor would draw from and its frames.
    pub fn best(&self) -> Result<Option<(PathBuf, BTreeSet<FlowSample>)>, String> {
        Ok(super::best(&self.key()?))
    }

    /// The samples not among `present`.
    pub fn missing(&self, present: &BTreeSet<FlowSample>) -> BTreeSet<FlowSample> {
        self.samples.difference(present).copied().collect()
    }

    fn open_decoder(&self) -> Result<VideoDecoder, String> {
        let (w, h) = self.source_size;
        let long = w.max(h).max(1) as f32;
        let scale = (super::MAX_LONG_SIDE as f32 / long).min(1.0);
        let target_height = ((h as f32 * scale).round() as u32).max(16) & !1;
        VideoDecoder::open_scaled(&self.path, target_height)
            .map_err(|e| format!("could not open {}: {e}", self.path))
    }
}

/// Make the model ready in the worker and return the provider it runs on.
pub fn prepare(cancel: &AtomicBool) -> Result<String, String> {
    interpolate::prepare(&|_, _| {}, cancel).map_err(|e| e.to_string())
}

/// Bake `job`'s missing frames. Blocking; run it on a thread of its own.
/// `report` is called after every pair. A cancel keeps what is written.
pub fn run(
    job: &FlowJob,
    cancel: &AtomicBool,
    mut report: impl FnMut(&FlowProgress),
) -> Result<FlowOutcome, String> {
    let started = Instant::now();
    let key = job.key()?;
    if super::best(&key).is_some_and(|(_, present)| job.missing(&present).is_empty())
        || job.samples.is_empty()
    {
        return Ok(FlowOutcome {
            seconds: started.elapsed().as_secs_f64(),
            ..FlowOutcome::default()
        });
    }
    let provider = prepare(cancel)?;
    let dir = super::dir_in(&key, &provider, job.long_side());
    let missing = job.missing(&super::list(&dir));
    let mut outcome = FlowOutcome {
        provider: Some(provider.clone()),
        ..FlowOutcome::default()
    };
    if missing.is_empty() {
        outcome.seconds = started.elapsed().as_secs_f64();
        return Ok(outcome);
    }
    // Every phase of one pair goes in one request.
    let mut pairs: BTreeMap<i64, Vec<u32>> = BTreeMap::new();
    for sample in &missing {
        pairs.entry(sample.index).or_default().push(sample.step);
    }
    let mut progress = FlowProgress {
        done: 0,
        total: missing.len() as u32,
        provider: Some(provider),
        seconds_left: None,
    };
    report(&progress);
    let mut decoder = job.open_decoder()?;
    // The later frame of the last pair, which is the earlier one of the next
    // pair when the pairs are consecutive (as they are in slow motion).
    let mut held: Option<(i64, DecodedFrame)> = None;
    let mut model_millis = 0.0f64;
    let baking = Instant::now();
    for (&index, steps) in &pairs {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            break;
        }
        let first = match held.take() {
            Some((i, frame)) if i == index => frame,
            _ => decode(&mut decoder, job.middle(index))?,
        };
        let second = decode(&mut decoder, job.middle(index + 1))?;
        if (first.width, first.height) != (second.width, second.height) {
            return Err(format!(
                "frames {index} and {} of {} differ in size",
                index + 1,
                job.path
            ));
        }
        let phases: Vec<f32> = steps
            .iter()
            .map(|&step| FlowSample { index, step }.phase())
            .collect();
        let made = interpolate::interpolate(
            &first.data,
            &second.data,
            first.width as usize,
            first.height as usize,
            &phases,
            Some(cancel),
        )
        .map_err(|e| match e {
            crate::modules::ml::MlError::Cancelled => "cancelled".to_string(),
            other => other.to_string(),
        });
        let made = match made {
            Err(e) if e == "cancelled" => {
                outcome.cancelled = true;
                break;
            }
            other => other?,
        };
        model_millis += made.millis as f64;
        for (&step, frame) in steps.iter().zip(&made.frames) {
            super::write(
                &dir,
                FlowSample { index, step },
                first.width,
                first.height,
                frame,
            )?;
            outcome.written += 1;
        }
        progress.done += steps.len() as u32;
        let per_frame = baking.elapsed().as_secs_f64() / progress.done.max(1) as f64;
        progress.seconds_left = Some(per_frame * (progress.total - progress.done) as f64);
        report(&progress);
        held = Some((index + 1, second));
    }
    if outcome.written > 0 {
        outcome.model_millis_per_frame = Some((model_millis / outcome.written as f64) as f32);
    }
    outcome.seconds = started.elapsed().as_secs_f64();
    Ok(outcome)
}

fn decode(decoder: &mut VideoDecoder, at: Micros) -> Result<DecodedFrame, String> {
    decoder
        .seek_and_decode(at + SAMPLE_SLACK)
        .map_err(|e| format!("decode failed at {at} µs: {e}"))
}
