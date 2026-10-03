//! A feed-forward compressor.
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
}
