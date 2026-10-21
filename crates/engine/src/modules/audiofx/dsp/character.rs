// Adapted from FilmCraft (https://github.com/storytold/filmcraft):
// crates/audio-dsp/src/effects/special.rs (Distortion, `shape`) and
// crates/audio-dsp/src/effects/essential.rs (StereoWidth).
// Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
// Licensed under the Apache License, Version 2.0 or the MIT License.
// Changes for chukcut (offline whole-buffer processing on interleaved audio)
// are under the GPL-3.0 like the rest of this repository. See NOTICE.md.

//! Effects that change the character of a sound: distortion and stereo
//! width.

use std::f64::consts::PI;

/// The distortion curves, by the index the catalog stores.
pub const CURVES: [&str; 4] = ["Soft", "Hard", "Tube", "Fold"];

/// A memoryless transfer curve with unity slope for small signals.
pub fn shape(curve: usize, v: f32) -> f32 {
    match curve {
        0 => v.tanh(),
        1 => v.clamp(-1.0, 1.0),
        // Asymmetric: the negative half clips harder, which adds the even
        // harmonics a valve amplifier is liked for.
        2 => {
            if v >= 0.0 {
                v.tanh()
            } else {
                (1.6 * v).tanh() / 1.6
            }
        }
        _ => v.sin(),
    }
}

/// The distortion's settings.
#[derive(Debug, Clone, Copy)]
pub struct DistortionParams {
    pub drive_db: f32,
    /// An index into [`CURVES`].
    pub curve: usize,
    /// `-1..=1`: a bias before the curve; the DC it adds is removed after.
    pub asymmetry: f32,
    /// A low-pass after the curve; 20 kHz is off.
    pub tone_hz: f32,
    pub output_db: f32,
    pub mix: f32,
}

/// Waveshaping distortion: drive into a curve, DC blocked, darkened by the
/// tone filter, mixed with the dry sound.
pub fn distortion(buffer: &mut [f32], channels: usize, rate: u32, p: DistortionParams) {
    let channels = channels.max(1);
    let sr = rate as f32;
    let drive = 10f32.powf(p.drive_db / 20.0);
    let out = 10f32.powf(p.output_db / 20.0);
    let bias = 0.5 * p.asymmetry.clamp(-1.0, 1.0);
    let mix = p.mix.clamp(0.0, 1.0);
    let lp_on = p.tone_hz < 19_999.0;
    let alpha = 1.0 - (-2.0 * PI * (p.tone_hz.clamp(1.0, sr * 0.49) as f64) / sr as f64).exp();
    // DC blocker at about 20 Hz: y = x − x₁ + R·y₁.
    let r = 1.0 - 2.0 * PI * 20.0 / sr as f64;
    let mut dc = vec![(0.0f64, 0.0f64); channels];
    let mut lp = vec![0.0f64; channels];
    let offset = shape(p.curve, bias);
    for frame in buffer.chunks_exact_mut(channels) {
        for (ch, sample) in frame.iter_mut().enumerate() {
            let x = *sample;
            let shaped = (shape(p.curve, drive * x + bias) - offset) as f64;
            let (x1, y1) = &mut dc[ch];
            let mut y = shaped - *x1 + r * *y1;
            *x1 = shaped;
            *y1 = y;
            if lp_on {
                lp[ch] += alpha * (y - lp[ch]);
                y = lp[ch];
            }
            *sample = x + mix * (y as f32 * out - x);
        }
    }
}

/// Make a stereo sound wider or narrower: the side signal (left minus right)
/// is scaled by `width`, `0` is mono, `1` unchanged, `2` twice as wide. A
/// mono clip is left alone.
pub fn stereo_width(buffer: &mut [f32], channels: usize, width: f32) {
    // `m + s` is not bit-exact `l` in floating point; unchanged means
    // unchanged.
    if channels < 2 || width == 1.0 {
        return;
    }
    for frame in buffer.chunks_exact_mut(channels) {
        let (l, r) = (frame[0], frame[1]);
        let m = 0.5 * (l + r);
        let s = 0.5 * (l - r) * width;
        frame[0] = m + s;
        frame[1] = m - s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audiofx::dsp::test_signals::{rms, sine};

    fn params() -> DistortionParams {
        DistortionParams {
            drive_db: 24.0,
            curve: 0,
            asymmetry: 0.0,
            tone_hz: 20_000.0,
            output_db: 0.0,
            mix: 1.0,
        }
    }

    #[test]
    fn every_curve_is_unity_for_a_whisper_and_bounded_for_a_shout() {
        for (c, name) in CURVES.iter().enumerate() {
            assert!((shape(c, 0.001) - 0.001).abs() < 1e-5, "{name}");
            assert!(shape(c, 50.0).abs() <= 1.0 + 1e-6);
        }
    }

    #[test]
    fn heavy_drive_flattens_a_sine_and_the_tone_darkens_it() {
        let input = sine(220.0, 0.5, 48_000, 2);
        let mut hot = input.clone();
        distortion(&mut hot, 2, 48_000, params());
        // A sine driven 24 dB into tanh is nearly square: its RMS approaches
        // its peak.
        let peak = hot[4_800..].iter().fold(0.0f32, |m, s| m.max(s.abs())) as f64;
        assert!(rms(&hot[4_800..]) / peak > 0.85);
        // No DC left, even with asymmetry.
        let mut asym = input.clone();
        distortion(
            &mut asym,
            2,
            48_000,
            DistortionParams {
                asymmetry: 0.8,
                ..params()
            },
        );
        let mean = asym[24_000..].iter().map(|&s| s as f64).sum::<f64>() / 24_000.0;
        assert!(mean.abs() < 0.01, "{mean}");
        let mut dark = input.clone();
        distortion(
            &mut dark,
            2,
            48_000,
            DistortionParams {
                tone_hz: 800.0,
                ..params()
            },
        );
        assert!(rms(&dark[4_800..]) < rms(&hot[4_800..]));
        // Mix 0 is the dry signal.
        let mut dry = input.clone();
        distortion(
            &mut dry,
            2,
            48_000,
            DistortionParams {
                mix: 0.0,
                ..params()
            },
        );
        assert_eq!(dry, input);
    }

    #[test]
    fn width_zero_is_mono_and_one_is_unchanged() {
        let mut stereo: Vec<f32> = (0..2_000).map(|i| (i as f32 * 0.37).sin() * 0.5).collect();
        let original = stereo.clone();
        stereo_width(&mut stereo, 2, 1.0);
        assert_eq!(stereo, original);
        stereo_width(&mut stereo, 2, 0.0);
        assert!(stereo
            .as_chunks::<2>()
            .0
            .iter()
            .all(|f| (f[0] - f[1]).abs() < 1e-7));
        let mut mono = original.clone();
        stereo_width(&mut mono, 1, 2.0);
        assert_eq!(mono, original);
    }
}
