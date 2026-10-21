// Adapted from FilmCraft (https://github.com/storytold/filmcraft):
// crates/audio-dsp/src/effects/restoration.rs (DeHum) and
// crates/audio-dsp/src/effects/essential.rs (DeEsser).
// Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
// Licensed under the Apache License, Version 2.0 or the MIT License.
// Changes for chukcut (offline whole-buffer processing, chukcut's biquads,
// the high- and low-pass filters) are under the GPL-3.0 like the rest of
// this repository. See NOTICE.md.

//! Clean-up filters for recorded sound: high-pass and low-pass, the mains
//! hum remover and the de-esser.
//!
//! Like everything in `dsp`, each function runs once over a whole clip, so
//! there is no parameter smoothing: a value is constant for the render.

use super::biquad::{Biquad, Coefficients};

/// The Q of each section of a 4th-order Butterworth filter (two cascaded
/// biquads). One 0.707 section gives 12 dB per octave; these two give 24 dB
/// with the same flat passband.
const BUTTERWORTH4_Q: [f64; 2] = [0.541_196_1, 1.306_563];

/// Cut everything below `freq`. `steep` doubles the slope from 12 to 24 dB
/// per octave.
pub fn high_pass(buffer: &mut [f32], channels: usize, rate: u32, freq: f32, steep: bool) {
    pass(buffer, channels, rate, freq, steep, Coefficients::high_pass);
}

/// Cut everything above `freq`.
pub fn low_pass(buffer: &mut [f32], channels: usize, rate: u32, freq: f32, steep: bool) {
    pass(buffer, channels, rate, freq, steep, Coefficients::low_pass);
}

fn pass(
    buffer: &mut [f32],
    channels: usize,
    rate: u32,
    freq: f32,
    steep: bool,
    design: fn(f64, f64, f64) -> Coefficients,
) {
    let rate = rate as f64;
    if steep {
        for q in BUTTERWORTH4_Q {
            Biquad::new(design(rate, freq as f64, q), channels).process(buffer);
        }
    } else {
        Biquad::new(
            design(rate, freq as f64, std::f64::consts::FRAC_1_SQRT_2),
            channels,
        )
        .process(buffer);
    }
}

/// The most harmonics of the mains frequency the hum remover notches out.
pub const MAX_HUM_HARMONICS: usize = 10;

/// The hum remover's parameters.
#[derive(Debug, Clone, Copy)]
pub struct DeHumParams {
    /// 50 Hz (Europe, most of Asia and Africa) or 60 Hz (the Americas).
    pub sixty_hz: bool,
    /// How many of the fundamental's multiples get a notch, 1 to 10.
    pub harmonics: usize,
    /// The notches' quality: higher is narrower, so less of the voice goes
    /// with the hum.
    pub q: f32,
    /// 0 is the clip unchanged, 1 the full cut.
    pub amount: f32,
}

/// Remove mains hum: a cascade of notches at the fundamental and its
/// harmonics, mixed with the dry signal by `amount`.
pub fn dehum(buffer: &mut [f32], channels: usize, rate: u32, p: DeHumParams) {
    let f0 = if p.sixty_hz { 60.0 } else { 50.0 };
    let amount = p.amount.clamp(0.0, 1.0);
    if amount == 0.0 {
        return;
    }
    let dry = (amount < 1.0).then(|| buffer.to_vec());
    let rate_f = rate as f64;
    for k in 1..=p.harmonics.clamp(1, MAX_HUM_HARMONICS) {
        let freq = f0 * k as f64;
        // A notch near Nyquist would cut a band, not a line.
        if freq >= rate_f * 0.45 {
            break;
        }
        Biquad::new(Coefficients::notch(rate_f, freq, p.q as f64), channels).process(buffer);
    }
    if let Some(dry) = dry {
        for (wet, dry) in buffer.iter_mut().zip(dry) {
            *wet = dry + amount * (*wet - dry);
        }
    }
}

/// The de-esser's parameters.
#[derive(Debug, Clone, Copy)]
pub struct DeEsserParams {
    /// Where the sibilance starts: the corner of the detector's high-pass and
    /// of the shelf that turns it down.
    pub freq_hz: f32,
    /// How far the band may rise over the whole signal's level before it is
    /// turned down, in dB (negative: the band is normally quieter).
    pub threshold_db: f32,
    /// The most the band is turned down, in dB.
    pub reduction_db: f32,
}

/// Power in dB, with a floor far below anything audible.
fn pow_db(p: f64) -> f64 {
    10.0 * (p + 1e-20).log10()
}

/// One-pole coefficient for a time constant of `ms`.
fn coef(ms: f64, rate: f64) -> f64 {
    (-1.0 / (ms * 0.001 * rate)).exp()
}

/// Turn down "s" and "sh" sounds. The detector compares the power above
/// `freq_hz` with the power of the whole signal, so a loud voice and a quiet
/// one are treated alike; when the band stands out by more than the
/// threshold, a high shelf at the same frequency comes down by twice the
/// excess, at most `reduction_db`.
pub fn deess(buffer: &mut [f32], channels: usize, rate: u32, p: DeEsserParams) {
    let channels = channels.max(1);
    let rate_f = rate as f64;
    let freq = p.freq_hz as f64;
    let mut detector = Biquad::new(
        Coefficients::high_pass(rate_f, freq, std::f64::consts::FRAC_1_SQRT_2),
        channels,
    );
    let mut shelf = Biquad::new(Coefficients::IDENTITY, channels);
    let (det, att, rel) = (coef(2.0, rate_f), coef(1.0, rate_f), coef(60.0, rate_f));
    let (mut env_band, mut env_full, mut gr, mut shelf_gr) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let threshold = p.threshold_db as f64;
    let reduction = p.reduction_db.max(0.0) as f64;
    for frame in buffer.chunks_exact_mut(channels) {
        let (mut band, mut full) = (0.0f64, 0.0f64);
        for (channel, sample) in frame.iter().enumerate() {
            let x = *sample as f64;
            let b = detector.tick(channel, x);
            band += b * b;
            full += x * x;
        }
        env_band = det * env_band + (1.0 - det) * band;
        env_full = det * env_full + (1.0 - det) * full;
        let band_db = pow_db(env_band);
        let excess = band_db - pow_db(env_full) - threshold;
        let target = if band_db > -70.0 && excess > 0.0 {
            -(excess * 2.0).min(reduction)
        } else {
            0.0
        };
        let c = if target < gr { att } else { rel };
        gr = c * gr + (1.0 - c) * target;
        if target == 0.0 && gr > -1e-4 {
            gr = 0.0;
        }
        // Redesigning the shelf costs a few transcendental calls; doing it
        // only when the reduction moved by a twentieth of a dB keeps a long
        // clip cheap without an audible step.
        if (gr - shelf_gr).abs() > 0.05 || (gr == 0.0) != (shelf_gr == 0.0) {
            let c = if gr == 0.0 {
                Coefficients::IDENTITY
            } else {
                Coefficients::high_shelf(rate_f, freq, std::f64::consts::FRAC_1_SQRT_2, gr)
            };
            shelf.set_coefficients(c);
            shelf_gr = gr;
        }
        for (channel, sample) in frame.iter_mut().enumerate() {
            *sample = shelf.tick(channel, *sample as f64) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audiofx::dsp::test_signals::{rms, sine};

    fn gain_db(freq: f64, f: impl Fn(&mut [f32])) -> f64 {
        let input = sine(freq, 0.5, 48_000, 2);
        let mut output = input.clone();
        f(&mut output);
        20.0 * (rms(&output[48_000..]) / rms(&input[48_000..])).log10()
    }

    /// The same over three seconds, the last one measured: a notch of Q 30
    /// at 50 Hz takes a few hundred milliseconds to settle.
    fn settled_db(freq: f64, f: impl Fn(&mut [f32])) -> f64 {
        let input = sine(freq, 0.5, 144_000, 2);
        let mut output = input.clone();
        f(&mut output);
        20.0 * (rms(&output[192_000..]) / rms(&input[192_000..])).log10()
    }

    #[test]
    fn the_high_pass_cuts_below_its_corner_and_the_steep_one_cuts_more() {
        let gentle = |b: &mut [f32]| high_pass(b, 2, 48_000, 200.0, false);
        let steep = |b: &mut [f32]| high_pass(b, 2, 48_000, 200.0, true);
        assert!(gain_db(2_000.0, gentle).abs() < 0.2);
        assert!(gain_db(2_000.0, steep).abs() < 0.2);
        // One octave below: about −12 dB for 2nd order, −24 dB for 4th.
        let g = gain_db(100.0, gentle);
        let s = gain_db(100.0, steep);
        assert!((-14.0..-10.0).contains(&g), "{g}");
        assert!((-26.0..-21.0).contains(&s), "{s}");
        // Both are −3 dB at the corner: Butterworth.
        assert!((gain_db(200.0, steep) + 3.0).abs() < 0.3);
    }

    #[test]
    fn the_low_pass_cuts_above_its_corner() {
        let lp = |b: &mut [f32]| low_pass(b, 2, 48_000, 1_000.0, true);
        assert!(gain_db(200.0, lp).abs() < 0.2);
        assert!(gain_db(4_000.0, lp) < -40.0);
    }

    #[test]
    fn the_hum_remover_kills_the_hum_and_keeps_the_voice() {
        let p = DeHumParams {
            sixty_hz: false,
            harmonics: 5,
            q: 30.0,
            amount: 1.0,
        };
        let f = |b: &mut [f32]| dehum(b, 2, 48_000, p);
        assert!(settled_db(50.0, f) < -30.0, "fundamental");
        assert!(settled_db(150.0, f) < -30.0, "third harmonic");
        // Between the notches, a voice's fundamental passes.
        assert!(settled_db(175.0, f).abs() < 0.5);
        assert!(settled_db(1_000.0, f).abs() < 0.1);
        // 60 Hz mains leaves 50 Hz alone.
        let us = |b: &mut [f32]| {
            dehum(
                b,
                2,
                48_000,
                DeHumParams {
                    sixty_hz: true,
                    ..p
                },
            )
        };
        assert!(settled_db(60.0, us) < -30.0);
        assert!(settled_db(50.0, us) > -3.0);
        // Half the amount is half the cut, in linear terms: about −6 dB.
        let half = |b: &mut [f32]| dehum(b, 2, 48_000, DeHumParams { amount: 0.5, ..p });
        assert!((settled_db(50.0, half) + 6.0).abs() < 0.5);
    }

    #[test]
    fn the_de_esser_turns_down_a_hiss_and_leaves_a_low_voice() {
        let p = DeEsserParams {
            freq_hz: 6_000.0,
            threshold_db: -12.0,
            reduction_db: 8.0,
        };
        let f = |b: &mut [f32]| deess(b, 2, 48_000, p);
        let hiss = gain_db(9_000.0, f);
        assert!((-8.5..-5.0).contains(&hiss), "{hiss}");
        assert!(gain_db(300.0, f).abs() < 0.1);
    }
}
