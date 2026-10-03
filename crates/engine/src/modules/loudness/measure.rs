//! EBU R128 measurement: integrated loudness, loudness range, true peak.

use std::sync::atomic::{AtomicBool, Ordering};

use ebur128::{EbuR128, Mode};
use serde::{Deserialize, Serialize};

use crate::modules::audio::decode::{AudioClipReader, ClipReader};
use crate::modules::export::audio::frames_for;
use crate::modules::project::document::TimeRange;

/// Rate and layout every measurement here decodes to.
pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

/// What a loudness meter reports for a stretch of audio.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Loudness {
    /// Integrated (programme) loudness in LUFS. `None` for silence, which has
    /// no loudness — the gate removes every block.
    pub integrated: Option<f64>,
    /// Loudness range in LU.
    pub range: Option<f64>,
    /// The highest true peak over all channels, in dBTP.
    pub true_peak_db: f64,
}

/// A running meter. Feed it interleaved `f32` blocks of any size.
pub struct Meter {
    inner: EbuR128,
    channels: u32,
}

impl Meter {
    pub fn new(channels: u32, rate: u32) -> Result<Self, String> {
        let inner = EbuR128::new(channels, rate, Mode::I | Mode::LRA | Mode::TRUE_PEAK)
            .map_err(|e| format!("cannot start a loudness meter: {e}"))?;
        Ok(Self { inner, channels })
    }

    pub fn add(&mut self, interleaved: &[f32]) -> Result<(), String> {
        self.inner
            .add_frames_f32(interleaved)
            .map_err(|e| format!("loudness meter: {e}"))
    }

    pub fn finish(&self) -> Loudness {
        let finite = |v: Result<f64, ebur128::Error>| v.ok().filter(|v| v.is_finite());
        let peak = (0..self.channels)
            .filter_map(|c| self.inner.true_peak(c).ok())
            .fold(0.0f64, f64::max);
        Loudness {
            integrated: finite(self.inner.loudness_global()),
            range: finite(self.inner.loudness_range()),
            // Floored so the value survives JSON, which has no infinity.
            true_peak_db: amplitude_to_db(peak).max(-150.0),
        }
    }
}

pub fn amplitude_to_db(amplitude: f64) -> f64 {
    if amplitude > 0.0 {
        20.0 * amplitude.log10()
    } else {
        -f64::INFINITY
    }
}

/// Measure interleaved samples.
pub fn measure(interleaved: &[f32], channels: u32, rate: u32) -> Result<Loudness, String> {
    let mut meter = Meter::new(channels, rate)?;
    meter.add(interleaved)?;
    Ok(meter.finish())
}

/// Measure `range` (source time) of the file at `path`, streaming.
pub fn measure_file(path: &str, range: TimeRange, cancel: &AtomicBool) -> Result<Loudness, String> {
    let mut reader = AudioClipReader::open(path, RATE, CHANNELS).map_err(|e| e.to_string())?;
    let mut meter = Meter::new(CHANNELS as u32, RATE)?;
    let first = frames_for(range.start.max(0), RATE) as i64;
    let total = frames_for(range.duration, RATE);
    const CHUNK: usize = 48_000 * 5;
    let mut buffer = vec![0.0f32; CHUNK * CHANNELS as usize];
    let mut done = 0usize;
    while done < total {
        if cancel.load(Ordering::Relaxed) {
            return Err("the measurement was cancelled".into());
        }
        let frames = CHUNK.min(total - done);
        let chunk = &mut buffer[..frames * CHANNELS as usize];
        reader
            .read(first + done as i64, frames, chunk)
            .map_err(|e| e.to_string())?;
        meter.add(chunk)?;
        done += frames;
    }
    Ok(meter.finish())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A 1 kHz stereo sine at `amplitude`, `seconds` long.
    pub(crate) fn sine(amplitude: f32, seconds: f32) -> Vec<f32> {
        let frames = (48_000.0 * seconds) as usize;
        (0..frames)
            .flat_map(|i| {
                let v = amplitude * (i as f32 * 1_000.0 * std::f32::consts::TAU / 48_000.0).sin();
                [v, v]
            })
            .collect()
    }

    #[test]
    fn a_full_scale_sine_on_both_channels_reads_zero_lufs() {
        // EBU Tech 3341's calibration: a 1 kHz sine at −23 dBFS on both
        // channels reads −23 LUFS; at full scale, 0. (1 kHz passes the
        // K-weighting at +0.69 dB, which the −0.691 in the formula cancels.)
        let l = measure(&sine(1.0, 5.0), 2, 48_000).unwrap();
        let i = l.integrated.unwrap();
        assert!(i.abs() < 0.1, "got {i}");
        assert!(l.true_peak_db.abs() < 0.3);
    }

    #[test]
    fn halving_the_amplitude_is_six_lu_quieter() {
        let a = measure(&sine(0.5, 5.0), 2, 48_000)
            .unwrap()
            .integrated
            .unwrap();
        let b = measure(&sine(0.25, 5.0), 2, 48_000)
            .unwrap()
            .integrated
            .unwrap();
        assert!(((a - b) - 6.02).abs() < 0.1, "{a} vs {b}");
    }

    #[test]
    fn silence_has_no_loudness() {
        let l = measure(&vec![0.0; 48_000 * 4], 2, 48_000).unwrap();
        assert!(l.integrated.is_none());
        assert!(l.true_peak_db <= -150.0);
    }
}
