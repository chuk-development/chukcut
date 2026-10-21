// Adapted from FilmCraft (https://github.com/storytold/filmcraft):
// crates/audio-dsp/src/effects/modulation.rs (ChorusFlanger, Phaser) and
// crates/audio-dsp/src/effects/util.rs (DelayLine, Lfo, AllPass1).
// Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
// Licensed under the Apache License, Version 2.0 or the MIT License.
// Changes for chukcut (offline whole-buffer processing on interleaved audio)
// are under the GPL-3.0 like the rest of this repository. See NOTICE.md.

//! Modulation effects: chorus, flanger and phaser. Each moves a short delay
//! or an all-pass chain with a slow oscillator and mixes it with the dry
//! sound. Feedback is bounded below 1, so no setting can run away.

use std::f64::consts::PI;

/// A circular delay line with linear-interpolated fractional reads.
struct DelayLine {
    buf: Vec<f32>,
    w: usize,
}

impl DelayLine {
    fn new(max: usize) -> Self {
        Self {
            buf: vec![0.0; max + 3],
            w: 0,
        }
    }

    /// The sample written `d` samples ago (0 is the last `push`).
    fn read(&self, d: f32) -> f32 {
        let len = self.buf.len();
        let d = d.clamp(0.0, (len - 3) as f32);
        let di = d.floor();
        let frac = d - di;
        let i0 = (self.w + len - 1 - di as usize) % len;
        let i1 = (i0 + len - 1) % len;
        self.buf[i0] + (self.buf[i1] - self.buf[i0]) * frac
    }

    fn push(&mut self, x: f32) {
        // Denormals in a feedback loop cost a hundred times the cycles.
        self.buf[self.w] = if x.abs() < 1e-30 { 0.0 } else { x };
        self.w = (self.w + 1) % self.buf.len();
    }
}

/// A raised sine of a phase in cycles, 0..1.
fn uni_sine(phase: f64) -> f32 {
    (0.5 - 0.5 * (2.0 * PI * phase).cos()) as f32
}

/// A triangle of a phase in cycles, 0..1.
fn uni_tri(phase: f64) -> f32 {
    (if phase < 0.5 {
        2.0 * phase
    } else {
        2.0 - 2.0 * phase
    }) as f32
}

/// Chorus and flanger settings. Percentages are `0..=1`.
#[derive(Debug, Clone, Copy)]
pub struct ChorusParams {
    /// How fast the delay moves, in Hz.
    pub speed_hz: f32,
    /// How far it moves.
    pub width: f32,
    /// Feedback: a chorus gets thicker, a flanger more metallic.
    pub intensity: f32,
    pub mix: f32,
}

/// Three voices whose delays drift between 15 and 35 ms: one sound becomes a
/// small ensemble.
pub fn chorus(buffer: &mut [f32], channels: usize, rate: u32, p: ChorusParams) {
    modulated_delay(buffer, channels, rate, p, false);
}

/// One voice sweeping between 0.3 and 5 ms with strong feedback: the jet
/// whoosh.
pub fn flanger(buffer: &mut [f32], channels: usize, rate: u32, p: ChorusParams) {
    modulated_delay(buffer, channels, rate, p, true);
}

fn modulated_delay(buffer: &mut [f32], channels: usize, rate: u32, p: ChorusParams, flange: bool) {
    let channels = channels.max(1);
    let sr = rate as f32;
    let ms = sr * 0.001;
    let mut lines: Vec<DelayLine> = (0..channels)
        .map(|_| DelayLine::new((0.05 * sr) as usize))
        .collect();
    let mut fb = vec![0.0f32; channels];
    let mut phase = 0.0f64;
    let step = p.speed_hz as f64 / rate as f64;
    let g = if flange { 0.95 } else { 0.6 } * p.intensity.clamp(0.0, 1.0);
    let (width, mix) = (p.width.clamp(0.0, 1.0), p.mix.clamp(0.0, 1.0));
    for frame in buffer.chunks_exact_mut(channels) {
        for (ch, sample) in frame.iter_mut().enumerate() {
            let x = *sample;
            // A quarter cycle between left and right spreads the voices.
            let off = ch as f64 * 0.25;
            let line = &mut lines[ch];
            let wet = if flange {
                let d = (0.3 + 4.7 * width * uni_tri((phase + off).rem_euclid(1.0))) * ms;
                line.read(d)
            } else {
                let mut acc = 0.0;
                for v in 0..3 {
                    let ph = (phase + off + v as f64 / 3.0).rem_euclid(1.0);
                    acc += line.read((15.0 + 20.0 * width * uni_sine(ph)) * ms);
                }
                acc / 3.0
            };
            fb[ch] = wet;
            line.push(x + g * fb[ch]);
            *sample = x + mix * (wet - x);
        }
        phase = (phase + step).fract();
    }
}

/// The phaser's settings. Percentages are `0..=1`; feedback is `-1..=1`.
#[derive(Debug, Clone, Copy)]
pub struct PhaserParams {
    /// All-pass stages, even, 2 to 12: more stages, more notches.
    pub stages: usize,
    pub rate_hz: f32,
    /// How many of the six octaves below the top frequency the sweep covers.
    pub depth: f32,
    pub upper_hz: f32,
    pub feedback: f32,
    pub mix: f32,
}

/// The coefficient of a first-order all-pass whose 90° point is at `hz`.
fn allpass_coef(hz: f32, sr: f32) -> f64 {
    let t = (PI * (hz.clamp(1.0, sr * 0.49) as f64) / sr as f64).tan();
    (t - 1.0) / (t + 1.0)
}

/// A swept cascade of first-order all-pass stages, mixed with the dry
/// signal: moving notches, the sound of 1970s keyboards.
pub fn phaser(buffer: &mut [f32], channels: usize, rate: u32, p: PhaserParams) {
    const MAX_STAGES: usize = 12;
    let channels = channels.max(1);
    let sr = rate as f32;
    let stages = ((p.stages / 2) * 2).clamp(2, MAX_STAGES);
    let mut state = vec![[(0.0f64, 0.0f64); MAX_STAGES]; channels];
    let mut fbs = vec![0.0f64; channels];
    let mut coefs = vec![0.0f64; channels];
    let mut phase = 0.0f64;
    let step = p.rate_hz as f64 / rate as f64;
    let fb = 0.9 * p.feedback.clamp(-1.0, 1.0) as f64;
    let (depth, mix) = (p.depth.clamp(0.0, 1.0), p.mix.clamp(0.0, 1.0));
    for (i, frame) in buffer.chunks_exact_mut(channels).enumerate() {
        // The sweep is slow; a new coefficient every 8 samples is inaudible
        // and saves a `tan` per sample.
        if i % 8 == 0 {
            for (ch, c) in coefs.iter_mut().enumerate() {
                // Right lags left by a quarter cycle: the sweep moves across.
                let ph = (phase + if ch == 1 { 0.25 } else { 0.0 }).rem_euclid(1.0);
                let f = p.upper_hz * 2f32.powf(-6.0 * depth * uni_sine(ph));
                *c = allpass_coef(f, sr);
            }
        }
        for (ch, sample) in frame.iter_mut().enumerate() {
            let x = *sample;
            let a = coefs[ch];
            let mut y = x as f64 + fb * fbs[ch];
            for (x1, y1) in state[ch].iter_mut().take(stages) {
                let out = a * y + *x1 - a * *y1;
                *x1 = y;
                *y1 = if out.abs() < 1e-30 { 0.0 } else { out };
                y = out;
            }
            fbs[ch] = y;
            *sample = x + mix * (y as f32 - x);
        }
        phase = (phase + step).fract();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audiofx::dsp::test_signals::{rms, sine};

    fn finite_and_bounded(v: &[f32], bound: f32) -> bool {
        v.iter().all(|s| s.is_finite() && s.abs() <= bound)
    }

    #[test]
    fn a_dry_mix_changes_nothing() {
        let input = sine(440.0, 0.5, 9_600, 2);
        let mut c = input.clone();
        chorus(
            &mut c,
            2,
            48_000,
            ChorusParams {
                speed_hz: 0.8,
                width: 0.5,
                intensity: 0.3,
                mix: 0.0,
            },
        );
        assert_eq!(c, input);
        let mut ph = input.clone();
        phaser(
            &mut ph,
            2,
            48_000,
            PhaserParams {
                stages: 4,
                rate_hz: 0.5,
                depth: 0.7,
                upper_hz: 2_500.0,
                feedback: 0.0,
                mix: 0.0,
            },
        );
        assert_eq!(ph, input);
    }

    #[test]
    fn full_feedback_stays_bounded_and_the_sound_changes() {
        let input = sine(300.0, 0.5, 96_000, 2);
        let p = ChorusParams {
            speed_hz: 2.0,
            width: 1.0,
            intensity: 1.0,
            mix: 0.5,
        };
        let mut f = input.clone();
        flanger(&mut f, 2, 48_000, p);
        assert!(finite_and_bounded(&f, 12.0));
        assert!(f != input);
        let mut c = input.clone();
        chorus(&mut c, 2, 48_000, p);
        assert!(finite_and_bounded(&c, 4.0));
        let mut ph = input.clone();
        phaser(
            &mut ph,
            2,
            48_000,
            PhaserParams {
                stages: 12,
                rate_hz: 1.0,
                depth: 1.0,
                upper_hz: 5_000.0,
                feedback: 1.0,
                mix: 1.0,
            },
        );
        assert!(finite_and_bounded(&ph, 12.0));
        // An all-pass keeps the energy; the swept notch takes some of it.
        assert!(rms(&ph) > 0.01);
    }

    #[test]
    fn the_left_and_right_voices_differ() {
        let input = sine(500.0, 0.5, 48_000, 2);
        let mut c = input.clone();
        chorus(
            &mut c,
            2,
            48_000,
            ChorusParams {
                speed_hz: 1.0,
                width: 1.0,
                intensity: 0.0,
                mix: 1.0,
            },
        );
        let differs = c
            .as_chunks::<2>()
            .0
            .iter()
            .skip(4_800)
            .any(|f| (f[0] - f[1]).abs() > 1e-3);
        assert!(differs, "the quarter-cycle offset spreads the voices");
    }
}
