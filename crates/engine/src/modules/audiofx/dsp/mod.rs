//! The signal processing behind the audio effects.
//!
//! Everything here works **offline, on a whole buffer**: a clip's effects are
//! rendered once into the cache and both mixers read the result
//! (`super::render`). That is what lets a reverb tail, a compressor's
//! envelope and a pitch shifter's latency be exactly the same in the preview
//! and in the export — no effect is ever started cold at a seek point.

pub mod biquad;
pub mod delay;
pub mod dynamics;
pub mod pitch;
pub mod reverb;
pub mod voice;

use super::catalog::{self, descriptor};
use super::model::AudioEffect;

/// Run `effects` over `buffer` (interleaved, `channels` wide, at `rate`), in
/// order. Disabled effects and kinds this build does not know are skipped.
pub fn apply_chain(buffer: &mut [f32], channels: usize, rate: u32, effects: &[AudioEffect]) {
    for effect in effects.iter().filter(|e| e.enabled) {
        apply(buffer, channels, rate, effect);
    }
}

/// One effect, in place.
pub fn apply(buffer: &mut [f32], channels: usize, rate: u32, effect: &AudioEffect) {
    let Some(desc) = descriptor(&effect.kind) else {
        return;
    };
    let get = |id: &str| -> f32 {
        let spec = desc.param(id).expect("catalog parameter");
        effect
            .params
            .get(id)
            .copied()
            .map(|v| spec.clamp(v))
            .unwrap_or(spec.default)
    };
    let rate_f = rate as f64;
    match desc.id {
        catalog::EQ3 => {
            use biquad::{Biquad, Coefficients};
            let mut stages = [
                Biquad::new(
                    Coefficients::low_shelf(rate_f, 120.0, 0.707, get("low") as f64),
                    channels,
                ),
                Biquad::new(
                    Coefficients::peaking(rate_f, get("mid_freq") as f64, 0.8, get("mid") as f64),
                    channels,
                ),
                Biquad::new(
                    Coefficients::high_shelf(rate_f, 8_000.0, 0.707, get("high") as f64),
                    channels,
                ),
            ];
            for stage in &mut stages {
                stage.process(buffer);
            }
        }
        catalog::EQ5 => {
            use biquad::{Biquad, Coefficients};
            for band in 1..=5 {
                let freq = get(&format!("b{band}_freq")) as f64;
                let gain = get(&format!("b{band}_gain")) as f64;
                let q = get(&format!("b{band}_q")) as f64;
                let c = match band {
                    1 => Coefficients::low_shelf(rate_f, freq, q, gain),
                    5 => Coefficients::high_shelf(rate_f, freq, q, gain),
                    _ => Coefficients::peaking(rate_f, freq, q, gain),
                };
                Biquad::new(c, channels).process(buffer);
            }
        }
        catalog::COMPRESSOR => dynamics::Compressor::new(
            dynamics::CompressorParams {
                threshold_db: get("threshold"),
                ratio: get("ratio"),
                attack_ms: get("attack"),
                release_ms: get("release"),
                knee_db: get("knee"),
                makeup_db: get("makeup"),
            },
            rate,
            channels,
        )
        .process(buffer),
        catalog::REVERB => reverb::Reverb::new(
            reverb::ReverbParams {
                room: get("room"),
                damping: get("damping"),
                width: get("width"),
                mix: get("mix"),
            },
            rate,
            channels,
        )
        .process(buffer),
        catalog::DELAY => delay::Delay::new(
            delay::DelayParams {
                time_ms: get("time"),
                feedback: get("feedback"),
                mix: get("mix"),
                tone_hz: get("tone"),
            },
            rate,
            channels,
        )
        .process(buffer),
        catalog::PITCH => pitch::shift(
            buffer,
            channels,
            rate,
            pitch::PitchParams {
                semitones: get("semitones"),
                keep_formants: get("formant") >= 0.5,
                formant_semitones: 0.0,
            },
        ),
        catalog::VOICE_DEEP => pitch::shift(
            buffer,
            channels,
            rate,
            pitch::PitchParams {
                semitones: -5.0 * get("intensity"),
                keep_formants: true,
                // Resonances a little lower than the voice's own: a larger
                // throat, not a slowed-down tape.
                formant_semitones: -3.0 * get("intensity"),
            },
        ),
        catalog::VOICE_CHIPMUNK => pitch::shift(
            buffer,
            channels,
            rate,
            pitch::PitchParams {
                semitones: 7.0 * get("intensity"),
                keep_formants: false,
                formant_semitones: 0.0,
            },
        ),
        catalog::VOICE_ROBOT => voice::robot(buffer, channels, rate, get("intensity")),
        catalog::VOICE_TELEPHONE => voice::telephone(buffer, channels, rate, get("intensity")),
        catalog::VOICE_MEGAPHONE => voice::megaphone(buffer, channels, rate, get("intensity")),
        _ => {}
    }
}

#[cfg(test)]
pub(crate) mod test_signals {
    /// `frames` of a sine at `freq`, `channels` wide, at 48 kHz.
    pub fn sine(freq: f64, amplitude: f32, frames: usize, channels: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            let v = amplitude * (std::f64::consts::TAU * freq * i as f64 / 48_000.0).sin() as f32;
            for _ in 0..channels {
                out.push(v);
            }
        }
        out
    }

    pub fn rms(v: &[f32]) -> f64 {
        (v.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt()
    }

    /// The frequency of the strongest component of the first channel, by a
    /// zero-padded DFT peak with parabolic interpolation. Accurate to about a
    /// hertz over half a second, which is all a pitch test needs.
    pub fn dominant_frequency(v: &[f32], channels: usize, rate: u32) -> f64 {
        use realfft::RealFftPlanner;
        let mono: Vec<f32> = v.chunks_exact(channels).map(|f| f[0]).collect();
        let n = (mono.len() * 4).next_power_of_two();
        let mut input = vec![0.0f32; n];
        for (i, s) in mono.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / mono.len() as f32).cos();
            input[i] = s * w;
        }
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
        let mut spectrum = fft.make_output_vec();
        fft.process(&mut input, &mut spectrum).unwrap();
        let mags: Vec<f64> = spectrum.iter().map(|c| c.norm() as f64).collect();
        let (peak, _) = mags
            .iter()
            .enumerate()
            .skip(1)
            .fold(
                (0, 0.0),
                |best, (i, m)| if *m > best.1 { (i, *m) } else { best },
            );
        let (a, b, c) = (
            mags[peak.saturating_sub(1)],
            mags[peak],
            mags[(peak + 1).min(mags.len() - 1)],
        );
        let offset = 0.5 * (a - c) / (a - 2.0 * b + c);
        (peak as f64 + if offset.is_finite() { offset } else { 0.0 }) * rate as f64 / n as f64
    }
}
