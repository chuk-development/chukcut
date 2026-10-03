//! Bringing a finished mix to a loudness target without overshooting peaks.
//!
//! Gain alone gets the integrated loudness right and the peaks wrong: a quiet
//! voice-over raised to −14 LUFS has transients well over full scale. So the
//! gain is followed by a **true-peak limiter** — look-ahead, so the gain is
//! already down when the peak arrives, with a release slow enough not to pump
//! — and because limiting removes a little loudness, the measurement and
//! correction are run a second time. Two passes land within a few tenths of
//! a LU on anything that is not limited into a brick; the export test holds
//! the result to ±1 LU with FFmpeg's own `ebur128` as the referee.
//!
//! "True peak" is the peak of the reconstructed waveform, which can sit
//! between samples and above every one of them. The limiter estimates it by
//! 4× oversampling with cubic (Catmull-Rom) interpolation — cheaper than the
//! 49-tap polyphase filter a meter uses, close enough to keep inter-sample
//! overs well under the margin between the ceiling and full scale.

use serde::{Deserialize, Serialize};

use super::measure::{measure, Loudness};

/// What normalising a mix did, for the export log and the panel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormalizeReport {
    pub before: Loudness,
    pub after: Loudness,
    /// Total gain applied before limiting, in dB.
    pub gain_db: f64,
}

/// The largest boost a loudness target may apply. Past this the mix is mostly
/// noise floor and the result is a hiss at −14 LUFS.
pub const MAX_GAIN_DB: f64 = 30.0;

/// Normalise `samples` (interleaved) in place to `target_lufs` with true
/// peaks at or under `ceiling_db` dBTP. Silence is left alone.
pub fn normalize_in_place(
    samples: &mut [f32],
    channels: usize,
    rate: u32,
    target_lufs: f64,
    ceiling_db: f64,
) -> Result<NormalizeReport, String> {
    let channels = channels.max(1);
    let before = measure(samples, channels as u32, rate)?;
    let Some(measured) = before.integrated else {
        return Ok(NormalizeReport {
            before,
            after: before,
            gain_db: 0.0,
        });
    };

    let mut total_gain = 0.0;
    let mut current = measured;
    let mut after = before;
    for _ in 0..3 {
        let gain_db = (target_lufs - current).clamp(-MAX_GAIN_DB, MAX_GAIN_DB - total_gain);
        if gain_db.abs() < 0.05 {
            break;
        }
        apply_gain(samples, db_to_gain(gain_db) as f32);
        total_gain += gain_db;
        limit_true_peak(samples, channels, rate, ceiling_db);
        after = measure(samples, channels as u32, rate)?;
        match after.integrated {
            Some(i) => current = i,
            None => break,
        }
        if (current - target_lufs).abs() < 0.2 {
            break;
        }
    }
    Ok(NormalizeReport {
        before,
        after,
        gain_db: total_gain,
    })
}

pub fn db_to_gain(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

fn apply_gain(samples: &mut [f32], gain: f32) {
    for s in samples.iter_mut() {
        *s *= gain;
    }
}

/// Look-ahead and release of the limiter.
const LOOKAHEAD_SECONDS: f64 = 0.005;
const RELEASE_SECONDS: f64 = 0.080;

/// Keep every (estimated) true peak of `samples` at or under `ceiling_db`.
pub fn limit_true_peak(samples: &mut [f32], channels: usize, rate: u32, ceiling_db: f64) {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    if frames == 0 {
        return;
    }
    let ceiling = db_to_gain(ceiling_db) as f32;

    // 1. The gain each frame needs on its own.
    let mut need = vec![1.0f32; frames];
    for (f, need) in need.iter_mut().enumerate() {
        let peak = (0..channels)
            .map(|c| true_peak_at(samples, channels, frames, f, c))
            .fold(0.0f32, f32::max);
        if peak > ceiling {
            *need = ceiling / peak;
        }
    }
    if need.iter().all(|g| *g >= 1.0) {
        return;
    }

    // 2. Hold each requirement over the look-ahead on both sides, then average
    //    over half that window. Every value averaged at a peak frame is a
    //    hold that saw the peak, so the average never exceeds what the peak
    //    needs — the guarantee — while the ramp into it is smooth.
    let look = ((LOOKAHEAD_SECONDS * rate as f64) as usize).max(1);
    let held = min_filter(&need, look);
    let half = (look / 2).max(1);
    let smoothed = box_average(&held, half);

    // 3. Release: recover slowly, never faster than the smoothed need allows.
    let release = (-1.0 / (RELEASE_SECONDS * rate as f64)).exp() as f32;
    let mut gain = 1.0f32;
    for f in 0..frames {
        let target = smoothed[f];
        gain = if target < gain {
            target
        } else {
            target - (target - gain) * release
        };
        for c in 0..channels {
            samples[f * channels + c] *= gain;
        }
    }

    // 4. Whatever a rounding left over the ceiling is clipped there: the
    //    limiter's job is that nothing passes, and a clip of a few thousandths
    //    of a dB is inaudible where an over is not.
    for s in samples.iter_mut() {
        *s = s.clamp(-ceiling, ceiling);
    }
}

/// The highest absolute value of channel `c` between frame `f` and `f + 1`,
/// at 4× oversampling.
fn true_peak_at(samples: &[f32], channels: usize, frames: usize, f: usize, c: usize) -> f32 {
    let at = |i: isize| -> f32 {
        let i = i.clamp(0, frames as isize - 1) as usize;
        samples[i * channels + c]
    };
    let f = f as isize;
    let (p0, p1, p2, p3) = (at(f - 1), at(f), at(f + 1), at(f + 2));
    let mut peak = p1.abs();
    for step in 1..4 {
        let t = step as f32 / 4.0;
        // Catmull-Rom between p1 and p2.
        let v = 0.5
            * ((2.0 * p1)
                + (-p0 + p2) * t
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
                + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t);
        peak = peak.max(v.abs());
    }
    peak
}

/// Sliding minimum over `[i - radius, i + radius]`, O(n) with a deque.
fn min_filter(values: &[f32], radius: usize) -> Vec<f32> {
    let n = values.len();
    let mut out = vec![0.0; n];
    let mut deque: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let mut next = 0usize;
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = (i + radius).min(n - 1);
        while next <= hi {
            while deque.back().is_some_and(|&b| values[b] >= values[next]) {
                deque.pop_back();
            }
            deque.push_back(next);
            next += 1;
        }
        let lo = i.saturating_sub(radius);
        while deque.front().is_some_and(|&f| f < lo) {
            deque.pop_front();
        }
        *slot = values[*deque.front().expect("window is never empty")];
    }
    out
}

/// Mean over `[i - radius, i + radius]`, clamped at the edges.
fn box_average(values: &[f32], radius: usize) -> Vec<f32> {
    let n = values.len();
    let mut prefix = vec![0.0f64; n + 1];
    for (i, v) in values.iter().enumerate() {
        prefix[i + 1] = prefix[i] + *v as f64;
    }
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(radius);
            let hi = (i + radius + 1).min(n);
            ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::measure::tests::sine;
    use super::*;

    #[test]
    fn a_quiet_sine_is_raised_to_the_target() {
        let mut s = sine(0.05, 6.0);
        let report = normalize_in_place(&mut s, 2, 48_000, -14.0, -1.0).unwrap();
        let after = report.after.integrated.unwrap();
        assert!((after + 14.0).abs() < 0.3, "got {after}");
        assert!(
            report.after.true_peak_db <= -0.9,
            "peak {}",
            report.after.true_peak_db
        );
    }

    #[test]
    fn a_loud_mix_is_brought_down() {
        let mut s = sine(0.9, 6.0);
        let report = normalize_in_place(&mut s, 2, 48_000, -23.0, -1.0).unwrap();
        assert!((report.after.integrated.unwrap() + 23.0).abs() < 0.3);
        assert!(report.gain_db < -15.0);
    }

    #[test]
    fn transients_over_the_ceiling_are_limited_and_the_target_still_holds() {
        // Speech-like: a quiet bed with short loud bursts, the case where gain
        // alone overshoots.
        let mut s = sine(0.03, 8.0);
        for burst in 0..8 {
            let at = (burst * 48_000 + 10_000) * 2;
            for i in 0..2_400 {
                s[at + i] *= 25.0;
            }
        }
        let report = normalize_in_place(&mut s, 2, 48_000, -14.0, -1.0).unwrap();
        assert!((report.after.integrated.unwrap() + 14.0).abs() < 1.0);
        assert!(
            report.after.true_peak_db <= -0.9,
            "peak {}",
            report.after.true_peak_db
        );
        assert!(s.iter().all(|v| v.abs() <= db_to_gain(-1.0) as f32 + 1e-6));
    }

    #[test]
    fn silence_is_left_alone() {
        let mut s = vec![0.0f32; 48_000 * 2];
        let report = normalize_in_place(&mut s, 2, 48_000, -14.0, -1.0).unwrap();
        assert_eq!(report.gain_db, 0.0);
        assert!(s.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_sliding_minimum_matches_the_naive_one() {
        let values: Vec<f32> = (0..50).map(|i| ((i * 37) % 11) as f32).collect();
        let fast = min_filter(&values, 3);
        for (i, got) in fast.iter().enumerate() {
            let lo = i.saturating_sub(3);
            let hi = (i + 3).min(values.len() - 1);
            let naive = values[lo..=hi].iter().copied().fold(f32::MAX, f32::min);
            assert_eq!(*got, naive);
        }
    }
}
