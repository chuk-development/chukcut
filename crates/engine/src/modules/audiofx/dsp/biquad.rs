//! Second-order filters from Robert Bristow-Johnson's "Audio EQ Cookbook".
//!
//! The coefficients are computed in `f64` and the state is `f64` as well: a
//! low shelf at 60 Hz has poles a hair inside the unit circle at 48 kHz, and in
//! `f32` the rounding of the recursion is audible as a low rumble on a long
//! render. Samples go in and out as `f32`.

use std::f64::consts::PI;

/// One biquad's normalised coefficients (`a0` divided out).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefficients {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefficients {
    /// Passes everything unchanged.
    pub const IDENTITY: Coefficients = Coefficients {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalised(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// `ω0` and `α` for a corner at `freq` with quality `q`, the corner held
    /// below Nyquist so a slider at 20 kHz on a 44.1 kHz render stays stable.
    fn omega(rate: f64, freq: f64, q: f64) -> (f64, f64, f64) {
        let freq = freq.clamp(1.0, rate * 0.49);
        let w0 = 2.0 * PI * freq / rate;
        let alpha = w0.sin() / (2.0 * q.max(0.05));
        (w0.cos(), w0.sin(), alpha)
    }

    /// A bell: `gain_db` at `freq`, `q` wide.
    pub fn peaking(rate: f64, freq: f64, q: f64, gain_db: f64) -> Self {
        if gain_db.abs() < 1e-6 {
            return Self::IDENTITY;
        }
        let a = 10f64.powf(gain_db / 40.0);
        let (cos, _, alpha) = Self::omega(rate, freq, q);
        Self::normalised(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        )
    }

    /// Everything below `freq` raised or lowered by `gain_db`.
    pub fn low_shelf(rate: f64, freq: f64, q: f64, gain_db: f64) -> Self {
        if gain_db.abs() < 1e-6 {
            return Self::IDENTITY;
        }
        let a = 10f64.powf(gain_db / 40.0);
        let (cos, _, alpha) = Self::omega(rate, freq, q);
        let s = 2.0 * a.sqrt() * alpha;
        Self::normalised(
            a * ((a + 1.0) - (a - 1.0) * cos + s),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - s),
            (a + 1.0) + (a - 1.0) * cos + s,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - s,
        )
    }

    /// Everything above `freq` raised or lowered by `gain_db`.
    pub fn high_shelf(rate: f64, freq: f64, q: f64, gain_db: f64) -> Self {
        if gain_db.abs() < 1e-6 {
            return Self::IDENTITY;
        }
        let a = 10f64.powf(gain_db / 40.0);
        let (cos, _, alpha) = Self::omega(rate, freq, q);
        let s = 2.0 * a.sqrt() * alpha;
        Self::normalised(
            a * ((a + 1.0) + (a - 1.0) * cos + s),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - s),
            (a + 1.0) - (a - 1.0) * cos + s,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - s,
        )
    }

    pub fn low_pass(rate: f64, freq: f64, q: f64) -> Self {
        let (cos, _, alpha) = Self::omega(rate, freq, q);
        Self::normalised(
            (1.0 - cos) / 2.0,
            1.0 - cos,
            (1.0 - cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    pub fn high_pass(rate: f64, freq: f64, q: f64) -> Self {
        let (cos, _, alpha) = Self::omega(rate, freq, q);
        Self::normalised(
            (1.0 + cos) / 2.0,
            -(1.0 + cos),
            (1.0 + cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// The filter's gain at `freq`, in dB. What the tests compare a measured
    /// response against.
    pub fn magnitude_db(&self, rate: f64, freq: f64) -> f64 {
        let w = 2.0 * PI * freq / rate;
        // H(e^jw) = (b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = self.b1 * s1 + self.b2 * s2;
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = self.a1 * s1 + self.a2 * s2;
        let num = (num_re * num_re + num_im * num_im).sqrt();
        let den = (den_re * den_re + den_im * den_im).sqrt();
        20.0 * (num / den).log10()
    }
}

/// One biquad over interleaved audio, with its own state per channel.
#[derive(Debug, Clone)]
pub struct Biquad {
    c: Coefficients,
    /// Transposed direct form II state, two values per channel.
    z: Vec<[f64; 2]>,
}

impl Biquad {
    pub fn new(c: Coefficients, channels: usize) -> Self {
        Self {
            c,
            z: vec![[0.0; 2]; channels.max(1)],
        }
    }

    pub fn is_identity(&self) -> bool {
        self.c == Coefficients::IDENTITY
    }

    #[inline]
    pub fn tick(&mut self, channel: usize, x: f64) -> f64 {
        let c = self.c;
        let z = &mut self.z[channel];
        let y = c.b0 * x + z[0];
        z[0] = c.b1 * x - c.a1 * y + z[1];
        z[1] = c.b2 * x - c.a2 * y;
        y
    }

    /// Filter `buffer` (interleaved, as many channels as this was made for) in
    /// place.
    pub fn process(&mut self, buffer: &mut [f32]) {
        if self.is_identity() {
            return;
        }
        let channels = self.z.len();
        for frame in buffer.chunks_exact_mut(channels) {
            for (channel, sample) in frame.iter_mut().enumerate() {
                *sample = self.tick(channel, *sample as f64) as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audiofx::dsp::test_signals::{rms, sine};

    const RATE: f64 = 48_000.0;

    /// Gain of a filter at `freq`, measured on a steady sine after it settles.
    fn measured_db(c: Coefficients, freq: f64) -> f64 {
        let input = sine(freq, 0.5, 48_000, 1);
        let mut output = input.clone();
        Biquad::new(c, 1).process(&mut output);
        20.0 * (rms(&output[24_000..]) / rms(&input[24_000..])).log10()
    }

    #[test]
    fn a_peaking_band_has_its_gain_at_its_centre_and_none_far_away() {
        let c = Coefficients::peaking(RATE, 1_000.0, 1.0, 9.0);
        assert!((c.magnitude_db(RATE, 1_000.0) - 9.0).abs() < 0.01);
        assert!((measured_db(c, 1_000.0) - 9.0).abs() < 0.1);
        assert!(measured_db(c, 50.0).abs() < 0.3);
        assert!(measured_db(c, 15_000.0).abs() < 0.3);
    }

    #[test]
    fn shelves_lift_their_own_side_only() {
        let low = Coefficients::low_shelf(RATE, 200.0, 0.707, -12.0);
        assert!((measured_db(low, 30.0) + 12.0).abs() < 0.5);
        assert!(measured_db(low, 5_000.0).abs() < 0.3);
        let high = Coefficients::high_shelf(RATE, 4_000.0, 0.707, 6.0);
        assert!((measured_db(high, 16_000.0) - 6.0).abs() < 0.5);
        assert!(measured_db(high, 100.0).abs() < 0.3);
    }

    #[test]
    fn the_analytic_response_matches_the_measured_one() {
        let c = Coefficients::high_pass(RATE, 300.0, 0.707);
        for freq in [100.0, 300.0, 1_000.0] {
            let expected = c.magnitude_db(RATE, freq);
            let got = measured_db(c, freq);
            assert!(
                (expected - got).abs() < 0.2,
                "{freq} Hz: {expected} vs {got}"
            );
        }
        assert!((c.magnitude_db(RATE, 300.0) + 3.01).abs() < 0.05);
    }

    #[test]
    fn zero_gain_is_exactly_transparent() {
        let c = Coefficients::peaking(RATE, 1_000.0, 1.0, 0.0);
        let input = sine(440.0, 0.5, 4_800, 2);
        let mut output = input.clone();
        Biquad::new(c, 2).process(&mut output);
        assert_eq!(input, output);
    }
}
