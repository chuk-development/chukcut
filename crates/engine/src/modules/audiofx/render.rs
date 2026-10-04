//! One clip's sound, rendered: its source played through its time map (speed
//! or speed curve, pitch kept or moved) and then through its effect stack.
//!
//! A render is a pure function of a [`RenderSpec`] and the source file, so it
//! is the same samples wherever it runs. The export calls [`render`] for every
//! clip that needs it, at the export's rate; the preview reads a cached render
//! of the same spec at 48 kHz (`super::cache`). Both then mix the result like
//! any other clip at speed 1 — volume, keyframes and the clip's place on the
//! timeline are applied by the mixers, as before, because they are cheap and
//! must follow an edit instantly.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::modules::audio::clock::{frames_to_micros, micros_to_frames};
use crate::modules::export::audio::{frames_for, AudioRequest, AudioSource};
use crate::modules::project::document::{Micros, Project, Segment};
use crate::modules::project::SpeedPoint;

use super::dsp::apply_chain;
use super::model::{fx_or_default, AudioEffect};
use super::stretch::{self, Mode, SourceBuffer, SourceMap};

/// Rate of the cached renders the preview reads, and of every render when the
/// export runs at the usual 48 kHz.
pub const RATE: u32 = 48_000;
/// The mix bus is stereo.
pub const CHANNELS: usize = 2;
/// Bumped whenever the DSP changes what a spec sounds like, so stale cache
/// files are never read. 2: the stretcher's random engine has a fixed seed
/// (`vendor/signalsmith-stretch`), so slow pitch-preserving renders changed.
pub const VERSION: u32 = 2;

/// Everything a render depends on except the file's contents.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderSpec {
    /// The file decoded: the original, or its denoised cache.
    pub path: String,
    /// Microseconds into the file where the clip starts.
    pub source_start: Micros,
    /// The clip's length on the timeline: the render is exactly this long.
    pub duration: Micros,
    /// Constant speed, when there is no curve.
    pub speed: f64,
    /// The speed curve's points, in source microseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curve: Option<Vec<SpeedPoint>>,
    pub pitch_follows_speed: bool,
    /// Enabled effects of known kinds, in order.
    pub effects: Vec<AudioEffect>,
}

impl RenderSpec {
    /// The spec as the cache key reads it: stable JSON.
    pub fn key_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    fn mode(&self) -> Mode {
        if self.pitch_follows_speed {
            Mode::Resample
        } else {
            Mode::PreservePitch
        }
    }

    fn is_retimed(&self) -> bool {
        self.curve.is_some() || (self.speed - 1.0).abs() > 1e-9
    }
}

/// Bounds on a constant speed, as the preview mixer has them.
fn sane_speed(speed: f32) -> f64 {
    let speed = speed as f64;
    if speed.is_finite() && speed > 0.0 {
        speed.clamp(0.01, 16.0)
    } else {
        1.0
    }
}

/// What `segment` needs rendered, given the file its sound is decoded from
/// (`path`, after voice cleanup), or `None` when the mixers can play the file
/// directly: no effects, and either real time or a constant speed whose pitch
/// is allowed to move (the mixers' own resampling, unchanged since before
/// this module).
pub fn spec_for(project: &Project, segment: &Segment, path: &str) -> Option<RenderSpec> {
    let fx = fx_or_default(project, segment);
    let curve = project
        .materials
        .speed_curve_of(segment)
        .map(|c| c.points.clone());
    let spec = RenderSpec {
        path: path.to_string(),
        source_start: segment.source_range.start.max(0),
        duration: segment.target_range.duration,
        speed: sane_speed(segment.speed),
        curve,
        pitch_follows_speed: fx.pitch_follows_speed,
        effects: fx.active_effects(),
    };
    if spec.duration <= 0 {
        return None;
    }
    let plain = spec.effects.is_empty()
        && (!spec.is_retimed() || (spec.curve.is_none() && spec.pitch_follows_speed));
    (!plain).then_some(spec)
}

/// Decode `frames` sample frames of `path` from absolute frame `first` (may
/// be negative: silence before the file) through `source`.
fn read_frames(
    source: &dyn AudioSource,
    path: &str,
    first: i64,
    frames: usize,
    rate: u32,
) -> Result<Vec<f32>, String> {
    let mut out = vec![0.0f32; frames * CHANNELS];
    let skip = (-first).max(0) as usize;
    if skip >= frames {
        return Ok(out);
    }
    let start_frame = first.max(0) as u64;
    let want = frames - skip;
    // Microseconds that round back to exactly these frames: `frames_to_micros`
    // truncates by less than a microsecond, and the reader and `frames_for`
    // both round to nearest.
    let request = AudioRequest {
        material_id: "",
        path,
        start: frames_to_micros(start_frame, rate),
        duration: frames_to_micros(want as u64, rate),
        sample_rate: rate,
        channels: CHANNELS as u16,
    };
    debug_assert_eq!(
        micros_to_frames(request.start, rate),
        start_frame as i64,
        "start frame rounds back"
    );
    let decoded = source.samples(&request).map_err(|e| e.to_string())?;
    let n = decoded.len().min(want * CHANNELS);
    out[skip * CHANNELS..skip * CHANNELS + n].copy_from_slice(&decoded[..n]);
    Ok(out)
}

/// Render `spec` at `rate`: exactly `frames_for(spec.duration, rate)` stereo
/// sample frames, interleaved.
pub fn render(
    spec: &RenderSpec,
    source: &dyn AudioSource,
    rate: u32,
    cancel: &AtomicBool,
) -> Result<Vec<f32>, String> {
    let out_frames = frames_for(spec.duration, rate);
    let mut out = if !spec.is_retimed() {
        let first = micros_to_frames(spec.source_start, rate);
        read_frames(source, &spec.path, first, out_frames, rate)?
    } else {
        let map = match &spec.curve {
            Some(points) => SourceMap::curve(points, spec.source_start, out_frames, rate),
            None => SourceMap::linear(micros_to_frames(spec.source_start, rate) as f64, spec.speed),
        };
        let (lo, hi) = stretch::source_span(&map, out_frames, spec.mode(), rate);
        let samples = read_frames(source, &spec.path, lo, (hi - lo).max(0) as usize, rate)?;
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let buffer = SourceBuffer {
            samples: &samples,
            first: lo,
            channels: CHANNELS,
        };
        stretch::stretch(&buffer, &map, out_frames, spec.mode(), rate)
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    apply_chain(&mut out, CHANNELS, rate, &spec.effects);
    // A broken float would poison the mix; the mixers clamp later anyway.
    for s in out.iter_mut() {
        if !s.is_finite() {
            *s = 0.0;
        }
    }
    Ok(out)
}
