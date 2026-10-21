//! A feed-forward compressor, a noise gate and a look-ahead limiter.
//!
//! Stereo-linked (one gain for both channels, from the louder of the two), so
//! a voice panned slightly left does not wander right when it gets loud. The
//! gain computer works in dB with a soft knee, and the reduction it asks for
//! goes through the "smooth decoupled peak" detector of Giannoulis, Massberg
//! and Reiss ("Digital Dynamic Range Compressor Design", JAES 2012): a peak
//! hold that decays at the release time, then a one-pole at the attack time.
//! A plain one-pole on the reduction would follow the shape of every cycle of
//! a low note and compress the sine's average rather than its peaks.

/// The parameters, in the units the catalog stores.
#[derive(Debug, Clone, Copy)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee_db: f32,
    pub makeup_db: f32,
}

pub struct Compressor {
    p: CompressorParams,
    attack: f64,
    release: f64,
    /// The peak-held reduction and the smoothed one, in dB, both `>= 0`.
    held: f64,
    reduction: f64,
    channels: usize,
}

impl Compressor {
    pub fn new(p: CompressorParams, rate: u32, channels: usize) -> Self {
        let coefficient = |ms: f32| {
            let samples = (ms.max(0.01) as f64 / 1_000.0) * rate as f64;
            (-1.0 / samples).exp()
        };
        Self {
            attack: coefficient(p.attack_ms),
            release: coefficient(p.release_ms),
            p,
            held: 0.0,
            reduction: 0.0,
            channels: channels.max(1),
        }
    }

    /// The static curve: output level for an input level, both in dB.
    pub fn curve(&self, x: f64) -> f64 {
        let t = self.p.threshold_db as f64;
        let r = (self.p.ratio as f64).max(1.0);
        let w = (self.p.knee_db as f64).max(0.0);
        let over = x - t;
        if 2.0 * over < -w {
            x
        } else if w > 0.0 && 2.0 * over.abs() <= w {
            x + (1.0 / r - 1.0) * (over + w / 2.0).powi(2) / (2.0 * w)
        } else {
            t + over / r
        }
    }

    pub fn process(&mut self, buffer: &mut [f32]) {
        let makeup = self.p.makeup_db as f64;
        for frame in buffer.chunks_exact_mut(self.channels) {
            let peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs())) as f64;
            let level = if peak > 1e-9 {
                20.0 * peak.log10()
            } else {
                -180.0
            };
            let wanted = level - self.curve(level);
            self.held = wanted.max(self.release * self.held + (1.0 - self.release) * wanted);
            self.reduction = self.attack * self.reduction + (1.0 - self.attack) * self.held;
            let gain = 10f64.powf((makeup - self.reduction) / 20.0) as f32;
            for sample in frame.iter_mut() {
                *sample *= gain;
            }
        }
    }
}

// The gate and the limiter below are adapted from FilmCraft
// (https://github.com/storytold/filmcraft), crates/audio-dsp/src/effects/
// dynamics.rs. Copyright (c) 2026 ArtCraft Team and the FilmCraft
// contributors, licensed under the Apache License, Version 2.0 or the MIT
// License. The offline rewrite for chukcut is under the GPL-3.0; NOTICE.md.

/// The noise gate's parameters.
#[derive(Debug, Clone, Copy)]
pub struct GateParams {
    /// Below this the sound is turned down.
    pub threshold_db: f32,
    /// How hard: every dB under the threshold becomes `ratio` dB. At 10 and
    /// up it behaves as a gate, below as a gentle expander.
    pub ratio: f32,
    /// The most it turns down, in dB: 60 is silence for practical purposes,
    /// 12 keeps a little room tone so the cuts do not sound like holes.
    pub range_db: f32,
    pub attack_ms: f32,
    /// How long it stays open after the sound falls under the threshold, so
    /// the gaps between words do not chatter.
    pub hold_ms: f32,
    pub release_ms: f32,
}

/// The gate's static curve: the output level for an input level, in dB.
pub fn expander_curve(x: f32, threshold: f32, ratio: f32, range: f32) -> f32 {
    if x >= threshold {
        x
    } else {
        (threshold + (x - threshold) * ratio.max(1.0)).max(x - range.max(0.0))
    }
}

fn one_pole(ms: f32, rate: u32) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * rate as f32)).exp()
    }
}

/// Turn down what is under the threshold: noise between the words of a
/// voice, the hiss of a quiet room. Stereo-linked like the compressor.
pub fn gate(buffer: &mut [f32], channels: usize, rate: u32, p: GateParams) {
    let channels = channels.max(1);
    let att = one_pole(p.attack_ms, rate);
    let rel = one_pole(p.release_ms, rate);
    let hold = (p.hold_ms.max(0.0) * 0.001 * rate as f32) as u32;
    let (mut env, mut hold_left) = (0.0f32, 0u32);
    for frame in buffer.chunks_exact_mut(channels) {
        let peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let level = 20.0 * peak.max(1e-10).log10();
        let target = expander_curve(level, p.threshold_db, p.ratio, p.range_db) - level;
        if target >= env {
            env = att * env + (1.0 - att) * target;
            if target >= 0.0 {
                hold_left = hold;
            }
        } else if hold_left > 0 {
            hold_left -= 1;
        } else {
            env = rel * env + (1.0 - rel) * target;
        }
        let g = 10f32.powf(env / 20.0);
        for sample in frame.iter_mut() {
            *sample *= g;
        }
    }
}

/// The limiter's parameters.
#[derive(Debug, Clone, Copy)]
pub struct LimiterParams {
    /// Gain before limiting: makes a quiet mix louder up to the ceiling.
    pub input_db: f32,
    /// No sample goes over this.
    pub ceiling_db: f32,
    pub release_ms: f32,
}

/// How far the limiter looks ahead. FilmCraft's value: long enough that the
/// gain is down before a drum hit arrives, short enough not to pump.
pub const LIMITER_LOOKAHEAD_MS: f32 = 5.0;

/// A look-ahead brick-wall limiter.
///
/// Offline, the look-ahead costs no latency: the gain each frame needs is
/// held over the look-ahead window with a sliding minimum, released with a
/// one-pole, and smoothed with a moving average exactly as long as the
/// window, reading forward — so the gain has reached its target when the
/// peak arrives. A final clamp at the ceiling makes the promise exact.
pub fn limit(buffer: &mut [f32], channels: usize, rate: u32, p: LimiterParams) {
    use std::collections::VecDeque;
    let channels = channels.max(1);
    let frames = buffer.len() / channels;
    if frames == 0 {
        return;
    }
    let input = 10f32.powf(p.input_db / 20.0);
    let ceiling = 10f32.powf(p.ceiling_db.min(0.0) / 20.0);
    let look = ((LIMITER_LOOKAHEAD_MS * 0.001 * rate as f32).round() as usize).max(1);
    let rel = one_pole(p.release_ms, rate);
    // The gain each frame needs on its own.
    let need: Vec<f32> = buffer
        .chunks_exact(channels)
        .map(|f| {
            let peak = f.iter().fold(0.0f32, |m, s| m.max(s.abs())) * input;
            if peak > ceiling {
                ceiling / peak
            } else {
                1.0
            }
        })
        .collect();
    // held[k] = min(need[k-look..=k]), then released upwards slowly.
    let mut held = vec![1.0f32; frames];
    let mut window: VecDeque<(usize, f32)> = VecDeque::with_capacity(look + 2);
    let mut released = 1.0f32;
    for (k, &n) in need.iter().enumerate() {
        while window.back().is_some_and(|&(_, v)| v >= n) {
            window.pop_back();
        }
        window.push_back((k, n));
        while window.front().is_some_and(|&(i, _)| i + look < k) {
            window.pop_front();
        }
        let m = window.front().map_or(1.0, |&(_, v)| v);
        released = if m < released {
            m
        } else {
            rel * released + (1.0 - rel) * m
        };
        held[k] = released;
    }
    // gain[j] = mean(held[j..=j+look]): every value in that window already
    // includes the peak at j, so the mean is at most what j needs.
    let mut sum: f64 = held[..(look + 1).min(frames)]
        .iter()
        .map(|&v| v as f64)
        .sum();
    let mut count = (look + 1).min(frames);
    for (j, frame) in buffer.chunks_exact_mut(channels).enumerate() {
        let g = (sum / count as f64) as f32 * input;
        for sample in frame.iter_mut() {
            *sample = (*sample * g).clamp(-ceiling, ceiling);
        }
        sum -= held[j] as f64;
        count -= 1;
        if let Some(&next) = held.get(j + look + 1) {
            sum += next as f64;
            count += 1;
        }
        if count == 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audiofx::dsp::test_signals::{rms, sine};

    fn params() -> CompressorParams {
        CompressorParams {
            threshold_db: -20.0,
            ratio: 4.0,
            attack_ms: 5.0,
            release_ms: 50.0,
            knee_db: 0.0,
            makeup_db: 0.0,
        }
    }

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    #[test]
    fn the_static_curve_follows_the_ratio_above_the_threshold() {
        let c = Compressor::new(params(), 48_000, 2);
        assert_eq!(c.curve(-30.0), -30.0);
        assert!((c.curve(-20.0) + 20.0).abs() < 1e-9);
        assert!(
            (c.curve(0.0) + 15.0).abs() < 1e-9,
            "20 dB over at 4:1 is 5 over"
        );
        let soft = Compressor::new(
            CompressorParams {
                knee_db: 10.0,
                ..params()
            },
            48_000,
            2,
        );
        // Continuous at both ends of the knee.
        assert!((soft.curve(-25.0) + 25.0).abs() < 1e-9);
        assert!((soft.curve(-15.0) - c.curve(-15.0)).abs() < 1e-9);
        assert!(soft.curve(-20.0) < -20.0);
    }

    #[test]
    fn a_loud_tone_settles_where_the_curve_says() {
        // A sine's peak is 3 dB over its RMS; the detector reads peaks.
        let input = sine(1_000.0, 0.5, 48_000, 2);
        let mut output = input.clone();
        Compressor::new(params(), 48_000, 2).process(&mut output);
        let peak_in = db(0.5);
        let expected_gain = (-20.0 + (peak_in + 20.0) / 4.0) - peak_in;
        let got = db(rms(&output[48_000..]) / rms(&input[48_000..]));
        assert!(
            (got - expected_gain).abs() < 0.6,
            "{got} dB, expected {expected_gain}"
        );
    }

    #[test]
    fn a_quiet_tone_passes_and_makeup_adds_gain() {
        let input = sine(1_000.0, 0.01, 9_600, 2);
        let mut output = input.clone();
        Compressor::new(
            CompressorParams {
                makeup_db: 6.0,
                ..params()
            },
            48_000,
            2,
        )
        .process(&mut output);
        let got = db(rms(&output[4_800..]) / rms(&input[4_800..]));
        assert!((got - 6.0).abs() < 0.05, "{got}");
    }

    #[test]
    fn the_gate_silences_quiet_noise_and_passes_a_voice() {
        let p = GateParams {
            threshold_db: -40.0,
            ratio: 10.0,
            range_db: 60.0,
            attack_ms: 1.0,
            hold_ms: 50.0,
            release_ms: 150.0,
        };
        let loud = sine(440.0, 0.5, 48_000, 2);
        let mut out = loud.clone();
        gate(&mut out, 2, 48_000, p);
        assert!(db(rms(&out[24_000..]) / rms(&loud[24_000..])).abs() < 0.1);
        // −60 dBFS noise floor: 20 dB under the threshold, ×10 = −200,
        // limited by the range to −60 dB more.
        // Two seconds, the last one measured: the gate starts open and
        // closes at the release time.
        let quiet = sine(440.0, 0.001, 96_000, 2);
        let mut out = quiet.clone();
        gate(&mut out, 2, 48_000, p);
        let g = db(rms(&out[96_000..]) / rms(&quiet[96_000..]));
        assert!((g + 60.0).abs() < 1.0, "{g}");
        assert_eq!(expander_curve(-30.0, -40.0, 10.0, 60.0), -30.0);
        assert_eq!(expander_curve(-42.0, -40.0, 2.0, 60.0), -44.0);
    }

    #[test]
    fn the_limiter_never_lets_a_sample_over_the_ceiling() {
        // A burst at full scale in the middle of a quiet tone.
        let mut buffer = sine(200.0, 0.1, 48_000, 2);
        for s in &mut buffer[40_000..44_000] {
            *s *= 10.0;
        }
        let quiet_before = rms(&buffer[2_000..20_000]);
        limit(
            &mut buffer,
            2,
            48_000,
            LimiterParams {
                input_db: 6.0,
                ceiling_db: -1.0,
                release_ms: 60.0,
            },
        );
        let ceiling = 10f32.powf(-1.0 / 20.0);
        let peak = buffer.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= ceiling + 1e-6, "{peak}");
        // The burst is turned down to just under the ceiling, not clipped:
        // its peak reaches the ceiling within half a dB.
        let burst_peak = buffer[40_000..44_000]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(db((burst_peak / ceiling) as f64) > -0.5);
        // Far from the burst the input gain is all that happened.
        let quiet_after = rms(&buffer[2_000..20_000]);
        assert!((db(quiet_after / quiet_before) - 6.0).abs() < 0.05);
    }
}
