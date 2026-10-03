//! Beat detection on music, in pure Rust: onset strength, tempo, beats.
//!
//! The research recommends Beat This! through ONNX Runtime once the ML worker
//! exists (`docs/research/ml-features.md` §3.6). Until then this is the
//! classic pipeline, which needs no model and no download and is good on music
//! with a clear pulse — the music people cut short-form video to:
//!
//! 1. **Onset strength**: log-compressed magnitude spectrum (1024-point Hann
//!    window, hop 256 at 22.05 kHz, ~11.6 ms), half-wave-rectified spectral
//!    flux, with its slow average removed.
//! 2. **Tempo**: the autocorrelation of the onset strength over the lags of
//!    60–200 BPM, weighted towards 120 BPM on a log scale (the prior of
//!    Ellis 2007, which keeps the answer off the half and double tempo).
//! 3. **Beats**: Ellis's dynamic programme — each frame's best score is its
//!    onset strength plus the best earlier beat about one period back, with a
//!    penalty for leaving the period — then a backtrack from the end.
//!
//! D. P. W. Ellis, "Beat Tracking by Dynamic Programming", JNMR 2007.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::modules::audio::decode::{AudioClipReader, ClipReader};
use crate::modules::project::document::{Micros, TimeRange};

/// Sample rate the sound is analysed at.
pub const RATE: u32 = 22_050;
const WINDOW: usize = 1024;
const HOP: usize = 256;
const MIN_BPM: f32 = 60.0;
const MAX_BPM: f32 = 200.0;
/// The tempo prior's centre and width (octaves).
const PRIOR_BPM: f32 = 120.0;
const PRIOR_OCTAVES: f32 = 1.0;
/// How strongly the beat tracker holds to the tempo.
const TIGHTNESS: f32 = 100.0;

/// What [`detect`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct BeatGrid {
    pub bpm: f32,
    /// Times of the beats, microseconds from the start of the analysed sound.
    pub beats: Vec<Micros>,
}

/// Decode `range` of the file at `path` as mono at [`RATE`]. Reports the
/// fraction done through `progress`.
pub fn decode(
    path: &str,
    range: TimeRange,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<Vec<f32>, String> {
    let mut reader = AudioClipReader::open(path, RATE, 1).map_err(|e| e.to_string())?;
    let first = (range.start.max(0) as i128 * RATE as i128 / 1_000_000) as i64;
    let total = (range.duration.max(0) as i128 * RATE as i128 / 1_000_000) as usize;
    let mut out = Vec::with_capacity(total);
    let mut chunk = vec![0.0f32; 1 << 15];
    while out.len() < total {
        if cancel.load(Ordering::Relaxed) {
            return Err(super::jobs::CANCELLED.into());
        }
        let frames = chunk.len().min(total - out.len());
        let buf = &mut chunk[..frames];
        reader
            .read(first + out.len() as i64, frames, buf)
            .map_err(|e| e.to_string())?;
        out.extend_from_slice(buf);
        progress(out.len() as f32 / total.max(1) as f32);
    }
    Ok(out)
}

/// In-place iterative radix-2 FFT of `re` + i·`im`; the length must be a
/// power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next;
            }
        }
        len <<= 1;
    }
}

/// Frames of onset strength per second.
pub fn frame_rate() -> f32 {
    RATE as f32 / HOP as f32
}

/// The onset strength of `mono`, one value per hop, normalised to unit
/// standard deviation.
pub fn onset_strength(mono: &[f32]) -> Vec<f32> {
    if mono.len() < WINDOW {
        return Vec::new();
    }
    let hann: Vec<f32> = (0..WINDOW)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos())
        .collect();
    // Up to ~8 kHz: above that there is little rhythm and much hiss.
    let bins = (8_000.0 / RATE as f32 * WINDOW as f32) as usize;
    let frames = (mono.len() - WINDOW) / HOP + 1;
    let mut previous = vec![0.0f32; bins];
    let mut flux = Vec::with_capacity(frames);
    let (mut re, mut im) = (vec![0.0f32; WINDOW], vec![0.0f32; WINDOW]);
    for f in 0..frames {
        let at = f * HOP;
        for i in 0..WINDOW {
            re[i] = mono[at + i] * hann[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        let mut sum = 0.0;
        for k in 1..bins {
            let level = (1.0 + 100.0 * (re[k] * re[k] + im[k] * im[k]).sqrt()).ln();
            if f > 0 {
                sum += (level - previous[k]).max(0.0);
            }
            previous[k] = level;
        }
        flux.push(sum);
    }
    // Remove the slow average (~0.4 s) so a crescendo is not a beat.
    let radius = (0.2 * frame_rate()) as usize;
    let mut prefix = vec![0.0f64; flux.len() + 1];
    for (i, v) in flux.iter().enumerate() {
        prefix[i + 1] = prefix[i] + *v as f64;
    }
    let mut out: Vec<f32> = (0..flux.len())
        .map(|i| {
            let lo = i.saturating_sub(radius);
            let hi = (i + radius + 1).min(flux.len());
            let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
            (flux[i] - mean as f32).max(0.0)
        })
        .collect();
    let n = out.len().max(1) as f32;
    let mean = out.iter().sum::<f32>() / n;
    let sd = (out.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n).sqrt();
    if sd > 1e-9 {
        for v in &mut out {
            *v /= sd;
        }
    }
    out
}

/// The beat period of `onset`, in onset frames, and the tempo it stands for.
pub fn tempo(onset: &[f32]) -> Option<(f32, f32)> {
    let rate = frame_rate();
    let min_lag = (60.0 / MAX_BPM * rate).floor() as usize;
    let max_lag = (60.0 / MIN_BPM * rate).ceil() as usize;
    if onset.len() < max_lag * 2 {
        return None;
    }
    // A beat period is rarely a whole number of frames, so a train of
    // one-frame onsets lines up badly with itself at one period and well at
    // two; a little blur first makes the two lags comparable.
    let blurred: Vec<f32> = (0..onset.len())
        .map(|i| {
            let (mut sum, mut weight) = (0.0, 0.0);
            for d in -3i32..=3 {
                let j = i as i32 + d;
                if j >= 0 && (j as usize) < onset.len() {
                    let w = (-(d * d) as f32 / (2.0 * 1.5 * 1.5)).exp();
                    sum += w * onset[j as usize];
                    weight += w;
                }
            }
            sum / weight
        })
        .collect();
    let mean = blurred.iter().sum::<f32>() / blurred.len() as f32;
    let centred: Vec<f32> = blurred.iter().map(|v| v - mean).collect();
    let longest = (2 * max_lag + 2).min(centred.len() - 1);
    let ac: Vec<f32> = (0..=longest)
        .map(|lag| {
            centred
                .iter()
                .zip(&centred[lag..])
                .map(|(a, b)| a * b)
                .sum::<f32>()
                / (centred.len() - lag) as f32
        })
        .collect();
    // A lag whose double also lines up is the beat; one whose double does
    // not is half of it.
    let at = |lag: usize| ac.get(lag).copied().unwrap_or(0.0).max(0.0);
    let weighted = |lag: usize| {
        let bpm = 60.0 * rate / lag as f32;
        let octaves = (bpm / PRIOR_BPM).log2() / PRIOR_OCTAVES;
        let double = at(2 * lag - 1).max(at(2 * lag)).max(at(2 * lag + 1));
        (at(lag) + 0.5 * double) * (-0.5 * octaves * octaves).exp()
    };
    let best = (min_lag.max(1)..=max_lag).max_by(|&a, &b| weighted(a).total_cmp(&weighted(b)))?;
    if ac[best] <= 0.0 {
        return None;
    }
    // Parabolic refinement on the raw autocorrelation.
    let (l, c, r) = (ac[best - 1], ac[best], ac[best + 1]);
    let denom = l - 2.0 * c + r;
    let shift = if denom.abs() > 1e-9 {
        (0.5 * (l - r) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let period = best as f32 + shift;
    Some((period, 60.0 * rate / period))
}

/// Beat frames in `onset` at `period` frames per beat.
pub fn track(onset: &[f32], period: f32) -> Vec<usize> {
    let n = onset.len();
    if n == 0 || period.is_nan() || period <= 1.0 {
        return Vec::new();
    }
    let mut score = vec![0.0f32; n];
    let mut back = vec![usize::MAX; n];
    let lo = (period * 0.5).round().max(1.0) as usize;
    let hi = (period * 2.0).round() as usize;
    for t in 0..n {
        let mut best = 0.0f32;
        let mut from = usize::MAX;
        if t >= lo {
            let first = t.saturating_sub(hi);
            for (tau, earlier) in score.iter().enumerate().take(t - lo + 1).skip(first) {
                let gap = (t - tau) as f32 / period;
                let s = earlier - TIGHTNESS * gap.ln().powi(2);
                if from == usize::MAX || s > best {
                    best = s;
                    from = tau;
                }
            }
        }
        // A frame with nothing a period back starts a chain of its own.
        if from != usize::MAX && best > 0.0 {
            score[t] = onset[t] + best;
            back[t] = from;
        } else {
            score[t] = onset[t];
        }
    }
    // The last beat: the best score in the final period.
    let tail = n.saturating_sub(period.ceil() as usize);
    let Some(mut t) = (tail..n).max_by(|&a, &b| score[a].total_cmp(&score[b])) else {
        return Vec::new();
    };
    let mut beats = vec![t];
    while back[t] != usize::MAX {
        t = back[t];
        beats.push(t);
    }
    beats.reverse();
    beats
}

/// Tempo and beats of `mono` (at [`RATE`]).
pub fn detect(mono: &[f32]) -> Option<BeatGrid> {
    let onset = onset_strength(mono);
    let (period, bpm) = tempo(&onset)?;
    let frames = track(&onset, period);
    // Each onset frame compares a window with the one before; the change it
    // measures sits at the centre of the window.
    let offset = WINDOW as f64 / 2.0 / RATE as f64;
    let beats = frames
        .into_iter()
        .map(|f| ((f as f64 * HOP as f64 / RATE as f64 + offset) * 1e6).round() as Micros)
        .collect();
    Some(BeatGrid { bpm, beats })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `seconds` of clicks every `60/bpm` s from `first`, over a quiet hum and
    /// noise, at [`RATE`].
    pub(crate) fn clicks(bpm: f32, first: f32, seconds: f32) -> Vec<f32> {
        let n = (seconds * RATE as f32) as usize;
        let mut out = vec![0.0f32; n];
        let mut state = 12_345u32;
        for (i, v) in out.iter_mut().enumerate() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let t = i as f32 / RATE as f32;
            *v = 0.05 * (std::f32::consts::TAU * 110.0 * t).sin()
                + 0.02 * ((state % 1000) as f32 / 500.0 - 1.0);
        }
        let period = 60.0 / bpm;
        let mut t = first;
        while t < seconds {
            let at = (t * RATE as f32) as usize;
            for k in 0..(0.03 * RATE as f32) as usize {
                if let Some(v) = out.get_mut(at + k) {
                    let decay = (-(k as f32) / (0.006 * RATE as f32)).exp();
                    *v += 0.8
                        * decay
                        * (std::f32::consts::TAU * 1500.0 * k as f32 / RATE as f32).sin();
                }
            }
            t += period;
        }
        out
    }

    fn check(bpm: f32, first: f32) {
        let grid = detect(&clicks(bpm, first, 12.0)).expect("a tempo");
        assert!((grid.bpm - bpm).abs() < 2.0, "found {} for {bpm}", grid.bpm);
        let period = 60.0 / bpm;
        let mut matched = 0;
        let truth: Vec<f32> = (0..)
            .map(|k| first + k as f32 * period)
            .take_while(|t| *t < 12.0)
            .collect();
        for t in &truth {
            if grid
                .beats
                .iter()
                .any(|b| (*b as f32 / 1e6 - t).abs() < 0.035)
            {
                matched += 1;
            }
        }
        assert!(
            matched as f32 >= truth.len() as f32 * 0.85,
            "{matched} of {} beats at {bpm} BPM; found {:?}",
            truth.len(),
            grid.beats
        );
    }

    #[test]
    fn a_click_track_at_120_is_found() {
        check(120.0, 0.25);
    }

    #[test]
    fn a_click_track_at_95_is_not_doubled() {
        check(95.0, 0.1);
    }

    #[test]
    fn a_click_track_at_150_is_not_halved() {
        check(150.0, 0.4);
    }

    #[test]
    fn the_fft_of_a_cosine_is_two_spikes() {
        let n = 64;
        let mut re: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 5.0 * i as f32 / n as f32).cos())
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        assert!((re[5] - 32.0).abs() < 1e-3 && (re[59] - 32.0).abs() < 1e-3);
        assert!(re[6].abs() < 1e-3);
    }

    #[test]
    fn silence_has_no_tempo() {
        assert!(detect(&vec![0.0; RATE as usize * 4]).is_none());
    }
}
