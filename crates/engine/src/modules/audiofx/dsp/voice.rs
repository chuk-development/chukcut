//! The voice changer presets that are not plain pitch shifts: telephone,
//! megaphone and robot. Deep and chipmunk are [`super::pitch`] with fixed
//! settings; see [`super::chain`].
//!
//! Each takes an `intensity` from 0 (the clip unchanged) to 1 (the preset as
//! designed) and blends the processed signal with the dry one.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

use super::biquad::{Biquad, Coefficients};

/// Blend `wet` into `dry` by `intensity`, in place in `dry`.
fn blend(dry: &mut [f32], wet: &[f32], intensity: f32) {
    let k = intensity.clamp(0.0, 1.0);
    for (d, w) in dry.iter_mut().zip(wet) {
        *d = *d * (1.0 - k) + *w * k;
    }
}

/// Sum to mono and write it back to every channel: a phone line and a
/// loudhailer have one speaker.
fn to_mono(buffer: &mut [f32], channels: usize) {
    if channels < 2 {
        return;
    }
    for frame in buffer.chunks_exact_mut(channels) {
        let mono = frame.iter().sum::<f32>() / channels as f32;
        frame.iter_mut().for_each(|s| *s = mono);
    }
}

/// A band-limited, driven speaker: the shared body of telephone and
/// megaphone.
struct Horn {
    low_cut: f64,
    high_cut: f64,
    /// A bell for the honk, `(freq, gain_db)`.
    peak: Option<(f64, f64)>,
    /// How hard the speaker is driven: the small-signal gain is kept at
    /// `makeup`, and only peaks are squashed.
    drive: f32,
    /// Gain after the band-limiting, so switching the preset on does not
    /// drop the level by what the filters took away.
    makeup: f32,
}

impl Horn {
    fn process(&self, buffer: &mut [f32], channels: usize, rate: u32, intensity: f32) {
        let rate_f = rate as f64;
        let mut wet = buffer.to_vec();
        to_mono(&mut wet, channels);
        // Two of each, for 24 dB per octave: a phone line's band edges are
        // steep, and a 12 dB slope leaves too much body for it to read as one.
        let mut stages = vec![
            Biquad::new(
                Coefficients::high_pass(rate_f, self.low_cut, 0.707),
                channels,
            ),
            Biquad::new(
                Coefficients::high_pass(rate_f, self.low_cut, 0.707),
                channels,
            ),
            Biquad::new(
                Coefficients::low_pass(rate_f, self.high_cut, 0.707),
                channels,
            ),
            Biquad::new(
                Coefficients::low_pass(rate_f, self.high_cut, 0.707),
                channels,
            ),
        ];
        if let Some((freq, gain)) = self.peak {
            stages.push(Biquad::new(
                Coefficients::peaking(rate_f, freq, 1.2, gain),
                channels,
            ));
        }
        for stage in &mut stages {
            stage.process(&mut wet);
        }
        for s in wet.iter_mut() {
            *s = (*s * self.drive).tanh() / self.drive * self.makeup;
        }
        blend(buffer, &wet, intensity);
    }
}

pub fn telephone(buffer: &mut [f32], channels: usize, rate: u32, intensity: f32) {
    Horn {
        low_cut: 300.0,
        high_cut: 3_400.0,
        peak: None,
        drive: 1.5,
        makeup: 1.2,
    }
    .process(buffer, channels, rate, intensity);
}

pub fn megaphone(buffer: &mut [f32], channels: usize, rate: u32, intensity: f32) {
    Horn {
        low_cut: 500.0,
        high_cut: 4_000.0,
        peak: Some((1_800.0, 8.0)),
        drive: 4.0,
        makeup: 2.0,
    }
    .process(buffer, channels, rate, intensity);
}

/// The robot's monotone, in Hz: the hop of the resynthesis.
pub const ROBOT_PITCH: f64 = 100.0;
const ROBOT_FFT: usize = 1_024;

/// Robotisation (Zölzer, *DAFX*, ch. 7): each short-time spectrum keeps its
/// magnitudes and loses its phases, and the frames are laid down one pitch
/// period apart. Every frame then starts in phase, so the frame rate becomes
/// the only pitch left — a flat buzz with the voice's spectral envelope on
/// it, which is what a talking machine sounds like.
pub fn robot(buffer: &mut [f32], channels: usize, rate: u32, intensity: f32) {
    let channels = channels.max(1);
    let frames = buffer.len() / channels;
    if frames == 0 {
        return;
    }
    let n = ROBOT_FFT * (rate as usize).div_ceil(48_000).max(1);
    let hop = ((rate as f64 / ROBOT_PITCH).round() as usize).max(1);
    let mut planner = RealFftPlanner::<f32>::new();
    let forward: Arc<dyn RealToComplex<f32>> = planner.plan_fft_forward(n);
    let inverse = planner.plan_fft_inverse(n);
    let window: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect();
    // Hann analysis and synthesis at this hop sum to a constant; that sum is
    // what the overlap-add is divided by.
    let overlap: f32 = window.iter().map(|w| w * w).sum::<f32>() / hop as f32;

    let mut wet = vec![0.0f32; buffer.len()];
    let mut frame = vec![0.0f32; n];
    let mut spectrum = forward.make_output_vec();
    let mut scratch_out = vec![0.0f32; n];
    for channel in 0..channels {
        // Centre the first window on the first sample so the head is covered.
        let mut start = -(n as i64) / 2;
        while start < frames as i64 {
            for (i, value) in frame.iter_mut().enumerate() {
                let at = start + i as i64;
                let sample = if at >= 0 && (at as usize) < frames {
                    buffer[at as usize * channels + channel]
                } else {
                    0.0
                };
                *value = sample * window[i];
            }
            if forward.process(&mut frame, &mut spectrum).is_err() {
                return;
            }
            for bin in spectrum.iter_mut() {
                *bin = realfft::num_complex::Complex::new(bin.norm(), 0.0);
            }
            if inverse.process(&mut spectrum, &mut scratch_out).is_err() {
                return;
            }
            // The zero-phase frame is centred on index 0; rotate it to the
            // middle of the window so the window does not cut its peak.
            for i in 0..n {
                let at = start + i as i64;
                if at < 0 || at as usize >= frames {
                    continue;
                }
                let sample = scratch_out[(i + n / 2) % n] / n as f32;
                wet[at as usize * channels + channel] += sample * window[i] / overlap;
            }
            start += hop as i64;
        }
    }
    blend(buffer, &wet, intensity);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audiofx::dsp::test_signals::{dominant_frequency, rms, sine};

    fn band_gain(f: fn(&mut [f32], usize, u32, f32), freq: f64) -> f64 {
        let input = sine(freq, 0.1, 48_000, 2);
        let mut output = input.clone();
        f(&mut output, 2, 48_000, 1.0);
        20.0 * (rms(&output[48_000..]) / rms(&input[48_000..])).log10()
    }

    #[test]
    fn the_telephone_keeps_the_voice_band_and_drops_the_rest() {
        let inside = band_gain(telephone, 1_000.0);
        assert!(inside.abs() < 4.0, "1 kHz moved by {inside} dB");
        assert!(band_gain(telephone, 80.0) < inside - 25.0);
        assert!(band_gain(telephone, 12_000.0) < inside - 25.0);
    }

    #[test]
    fn the_megaphone_is_narrower_and_honks_in_the_middle() {
        let honk = band_gain(megaphone, 1_800.0);
        assert!(honk > band_gain(megaphone, 700.0));
        assert!(band_gain(megaphone, 150.0) < honk - 25.0);
    }

    #[test]
    fn an_intensity_of_zero_changes_nothing() {
        let input = sine(500.0, 0.3, 4_800, 2);
        for f in [telephone, megaphone, robot] {
            let mut output = input.clone();
            f(&mut output, 2, 48_000, 0.0);
            assert_eq!(input, output);
        }
    }

    #[test]
    fn the_robot_buzzes_at_its_own_pitch_whatever_it_is_fed() {
        // A gliding tone: no steady pitch of its own.
        let rate = 48_000.0;
        let input: Vec<f32> = (0..48_000)
            .map(|i| {
                let t = i as f64 / rate;
                (0.3 * (std::f64::consts::TAU * (300.0 * t + 400.0 * t * t)).sin()) as f32
            })
            .collect();
        let mut output = input.clone();
        robot(&mut output, 1, 48_000, 1.0);
        // The output is a comb at multiples of 100 Hz: its strongest
        // component sits on one.
        let f = dominant_frequency(&output[12_000..36_000], 1, 48_000);
        let harmonic = (f / ROBOT_PITCH).round() * ROBOT_PITCH;
        assert!(
            (f - harmonic).abs() < 4.0,
            "{f} Hz is not on the 100 Hz comb"
        );
        assert!(rms(&output) > 0.05, "and it is not silent");
    }
}
