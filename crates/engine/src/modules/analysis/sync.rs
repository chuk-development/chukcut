// Adapted from FilmCraft (https://github.com/storytold/filmcraft),
// crates/audio-dsp/src/sync.rs: the GCC-PHAT-β cross-correlation, the
// valid-lag window, and the confidence and distinctness figures.
// Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
// Licensed under the Apache License, Version 2.0 or the MIT License.
// Changes for chukcut (realfft, one pass at the analysis rate, the clip
// edit) are under the GPL-3.0 like the rest of this repository. See
// NOTICE.md.

//! Lining up two recordings of the same moment by their sound: a camera's
//! own microphone and a separate recorder, two cameras at a wedding.
//!
//! [`find_offset`] returns the lag `L` (in samples) for which
//! `b[n] ≈ g · a[n + L]`: when `b` started recording two seconds after `a`,
//! `L = 2 · rate`. Both signals are made zero-mean and unit-RMS, then the
//! generalised cross-correlation with partial phase-transform weighting
//! (GCC-PHAT-β: `R = A·B* / |A·B*|^β`) is taken through one FFT each. The
//! weighting whitens the spectra, so the peak is sharp whatever the two
//! microphones sound like; β < 1 keeps bins of pure noise from dominating.
//!
//! The analysis runs at [`SYNC_RATE`]: 125 µs per sample, far finer than a
//! frame, so FilmCraft's second full-rate pass buys nothing a video editor
//! can see and is left out.
//!
//! Two figures say how far to trust the answer: `confidence`, the peak's
//! height over the RMS of the correlation, and `distinct`, its height over
//! the highest peak more than 50 ms away — music and steady hum can match
//! at several lags.
//!
//! Knapp & Carter, "The generalized correlation method for estimation of
//! time delay", IEEE Trans. ASSP 24(4), 1976.

use realfft::RealFftPlanner;

/// The rate both recordings are decoded at for the analysis.
pub const SYNC_RATE: u32 = 8_000;

/// The phase-transform weighting exponent: 1 whitens fully, 0 is a plain
/// cross-correlation. FilmCraft's value.
const PHAT_BETA: f32 = 0.75;

/// The offset found between two recordings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncResult {
    /// `b[n]` lines up with `a[n + lag]`, in samples.
    pub lag: i64,
    /// Peak height over the RMS of the correlation.
    pub confidence: f32,
    /// Peak height over the next-highest peak more than 50 ms away.
    pub distinct: f32,
    /// The two recordings correlate negatively: one has its polarity
    /// inverted. The offset is right all the same.
    pub inverted: bool,
}

impl SyncResult {
    /// Whether the match is clear enough to move clips by.
    pub fn reliable(&self) -> bool {
        self.confidence >= 8.0 && self.distinct >= 1.5
    }
}

/// Zero mean, unit RMS; silence stays silence.
fn normalise(x: &mut [f32]) {
    if x.is_empty() {
        return;
    }
    let mean = (x.iter().map(|&v| v as f64).sum::<f64>() / x.len() as f64) as f32;
    x.iter_mut().for_each(|v| *v -= mean);
    let rms = (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
    if rms > 1e-12 {
        let k = (1.0 / rms) as f32;
        x.iter_mut().for_each(|v| *v *= k);
    }
}

/// `r[L mod n] = Σ a[i + L]·b[i]` with whitened spectra, both zero-padded to
/// `n`. Bins below 40 Hz (rumble, wind) do not vote.
fn gcc(a: &[f32], b: &[f32], n: usize, rate: u32) -> Vec<f32> {
    let mut planner = RealFftPlanner::<f32>::new();
    let forward = planner.plan_fft_forward(n);
    let inverse = planner.plan_fft_inverse(n);
    let spectrum = |x: &[f32]| {
        let mut input = vec![0.0f32; n];
        input[..x.len()].copy_from_slice(x);
        let mut out = forward.make_output_vec();
        forward
            .process(&mut input, &mut out)
            .expect("buffers sized by the plan");
        out
    };
    let (sa, sb) = (spectrum(a), spectrum(b));
    let low = (40.0 * n as f32 / rate as f32).ceil() as usize;
    let mut r: Vec<_> = sa
        .iter()
        .zip(&sb)
        .enumerate()
        .map(|(k, (x, y))| {
            let p = x * y.conj();
            let mag = p.norm();
            if k < low || mag <= 1e-30 {
                realfft::num_complex::Complex32::new(0.0, 0.0)
            } else {
                p * mag.powf(-PHAT_BETA)
            }
        })
        .collect();
    // A real signal's spectrum has real DC and Nyquist bins.
    r[0].im = 0.0;
    if let Some(last) = r.last_mut() {
        last.im = 0.0;
    }
    let mut out = inverse.make_output_vec();
    inverse
        .process(&mut r, &mut out)
        .expect("buffers sized by the plan");
    out
}

fn corr_at(r: &[f32], lag: i64) -> f32 {
    r[lag.rem_euclid(r.len() as i64) as usize]
}

/// The lag `L` with `b[n] ≈ g·a[n + L]`, both at `rate`. `None` when either
/// is shorter than a tenth of a second, silent, or no lag leaves them
/// overlapping by half a second (or a quarter of the shorter one).
pub fn find_offset(a: &[f32], b: &[f32], rate: u32) -> Option<SyncResult> {
    let sr = rate.max(1) as i64;
    let min_len = (sr / 10).max(64) as usize;
    if a.len() < min_len || b.len() < min_len {
        return None;
    }
    let energy = |x: &[f32]| x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
    if energy(a) < 1e-12 || energy(b) < 1e-12 {
        return None;
    }
    let (mut ca, mut cb) = (a.to_vec(), b.to_vec());
    normalise(&mut ca);
    normalise(&mut cb);
    let (la, lb) = (ca.len() as i64, cb.len() as i64);
    let n = ((la + lb) as usize).next_power_of_two();
    let r = gcc(&ca, &cb, n, rate);
    let min_overlap = (sr / 2).min(la.min(lb) / 4).max(8);
    let (lo, hi) = (-(lb - min_overlap), la - min_overlap);
    if hi < lo {
        return None;
    }
    let mut best = (0i64, 0.0f32);
    let mut sum2 = 0.0f64;
    for l in lo..=hi {
        let v = corr_at(&r, l);
        sum2 += (v as f64).powi(2);
        if v.abs() > best.1.abs() {
            best = (l, v);
        }
    }
    if best.1 == 0.0 {
        return None;
    }
    let count = (hi - lo + 1) as f64;
    let rms = (sum2 / count).sqrt().max(1e-30) as f32;
    let guard = (sr / 20).max(2);
    let second = (lo..=hi)
        .filter(|l| (l - best.0).abs() > guard)
        .map(|l| corr_at(&r, l).abs())
        .fold(0.0f32, f32::max);
    Some(SyncResult {
        lag: best.0,
        confidence: best.1.abs() / rms,
        distinct: if second > 0.0 {
            best.1.abs() / second
        } else {
            f32::INFINITY
        },
        inverted: best.1 < 0.0,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Speech-like sound: syllables of gliding harmonics, noise bursts and
    /// pauses, from a fixed seed — no two stretches alike, like a voice.
    pub(crate) fn speechy(rate: u32, seconds: f64, seed: u64) -> Vec<f32> {
        let n = (rate as f64 * seconds) as usize;
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut rnd = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut out = vec![0.0f32; n];
        let mut i = 0;
        while i < n {
            let len = ((0.08 + 0.2 * rnd()) * rate as f64) as usize;
            let kind = rnd();
            let f0 = 110.0 + 120.0 * rnd();
            let amp = 0.2 + 0.3 * rnd();
            for k in 0..len.min(n - i) {
                let t = k as f64 / rate as f64;
                let env = (std::f64::consts::PI * k as f64 / len as f64).sin();
                let v = if kind > 0.35 {
                    (1..6)
                        .map(|h| (std::f64::consts::TAU * f0 * h as f64 * t).sin() / h as f64)
                        .sum::<f64>()
                } else if kind > 0.15 {
                    rnd() * 2.0 - 1.0
                } else {
                    0.0
                };
                out[i + k] = (amp * env * v) as f32;
            }
            i += len;
        }
        out
    }

    #[test]
    fn a_late_recorder_is_found_to_the_sample() {
        let a = speechy(SYNC_RATE, 20.0, 7);
        // b started 3.25 s later, at a third of the level, with some hiss.
        let shift = (3.25 * SYNC_RATE as f64) as usize;
        let b: Vec<f32> = a[shift..shift + 8 * SYNC_RATE as usize]
            .iter()
            .enumerate()
            .map(|(i, v)| v * 0.33 + 0.01 * ((i * 7919 % 101) as f32 / 50.0 - 1.0))
            .collect();
        let r = find_offset(&a, &b, SYNC_RATE).unwrap();
        assert_eq!(r.lag, shift as i64);
        assert!(r.reliable(), "{r:?}");
        // The other way round: a started before b, so the lag is negative.
        let back = find_offset(&b, &a, SYNC_RATE).unwrap();
        assert_eq!(back.lag, -(shift as i64));
    }

    #[test]
    fn inverted_polarity_still_lines_up_and_unrelated_sound_is_not_trusted() {
        let a = speechy(SYNC_RATE, 10.0, 3);
        let b: Vec<f32> = a[4_000..60_000].iter().map(|v| -v).collect();
        let r = find_offset(&a, &b, SYNC_RATE).unwrap();
        assert_eq!(r.lag, 4_000);
        assert!(r.inverted);
        let other = speechy(SYNC_RATE, 10.0, 99);
        let none = find_offset(&a, &other, SYNC_RATE).unwrap();
        assert!(!none.reliable(), "{none:?}");
        assert!(find_offset(&a, &vec![0.0; 40_000], SYNC_RATE).is_none());
    }
}
