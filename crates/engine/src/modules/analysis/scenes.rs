//! Scene (shot) detection: where one shot ends and the next begins.
//!
//! The classic detector `docs/research/ml-features.md` §3.5 recommends for
//! now, done in our own code rather than through FFmpeg's `scdet` because the
//! score per frame is what makes a sensitivity slider instant (it is cached,
//! and re-thresholding needs no decode), and because both halves of the score
//! are a few lines on frames we decode anyway:
//!
//! - a **colour histogram** distance (16 bins per RGB channel), which ignores
//!   motion inside a shot but jumps when the palette changes, and
//! - a **block difference** on a 16×9 grid of luma means, which catches a cut
//!   between two shots of similar colour.
//!
//! A frame is a cut when its score stands out from the frames around it — the
//! median of a window either side is subtracted — so a fast pan, which raises
//! every score for a while, is not a string of cuts. Peaks closer than a
//! minimum shot length keep only the strongest, which also stops a single
//! flash frame from counting twice. Hard cuts only; a dissolve is the job of
//! TransNetV2 once the ML runtime exists.

use serde::{Deserialize, Serialize};

use super::cache;
use super::frames::{walk, Frame, Walk};
use super::jobs::JobContext;
use crate::modules::project::document::Micros;

/// Height of the frames the detector looks at.
pub const ANALYSIS_HEIGHT: u32 = 90;
const BINS: usize = 16;
const GRID: (usize, usize) = (16, 9);
/// Frames either side whose median is the "normal" score at a frame.
const WINDOW: usize = 8;
/// The shortest shot reported, seconds.
const MIN_SHOT: f64 = 0.4;
/// Bump when the score changes, so cached scores are not reused.
const ALGORITHM: &str = "scenes-v1";

/// What one frame looks like, reduced to what the score compares.
#[derive(Debug, Clone)]
pub struct Signature {
    histogram: [[f32; BINS]; 3],
    grid: Vec<f32>,
}

impl Signature {
    pub fn of(rgba: &[u8], width: usize, height: usize) -> Self {
        let mut histogram = [[0.0f32; BINS]; 3];
        let (gw, gh) = GRID;
        let mut grid = vec![0.0f32; gw * gh];
        let mut counts = vec![0u32; gw * gh];
        let total = (width * height).max(1) as f32;
        for y in 0..height {
            let gy = (y * gh / height.max(1)).min(gh - 1);
            for x in 0..width {
                let i = (y * width + x) * 4;
                let Some(p) = rgba.get(i..i + 4) else {
                    continue;
                };
                for c in 0..3 {
                    histogram[c][p[c] as usize * BINS / 256] += 1.0;
                }
                let gx = (x * gw / width.max(1)).min(gw - 1);
                grid[gy * gw + gx] +=
                    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
                counts[gy * gw + gx] += 1;
            }
        }
        for channel in &mut histogram {
            for bin in channel.iter_mut() {
                *bin /= total;
            }
        }
        for (cell, n) in grid.iter_mut().zip(&counts) {
            *cell /= (*n).max(1) as f32;
        }
        Self { histogram, grid }
    }

    /// How different `self` is from `previous`, `0..=1`.
    pub fn distance(&self, previous: &Signature) -> f32 {
        let mut hist = 0.0;
        for c in 0..3 {
            for b in 0..BINS {
                hist += (self.histogram[c][b] - previous.histogram[c][b]).abs();
            }
        }
        // Each channel's histogram sums to 1, so its L1 distance is 0..2.
        let hist = hist / 6.0;
        let blocks = self
            .grid
            .iter()
            .zip(&previous.grid)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / self.grid.len().max(1) as f32
            / 255.0;
        // A cut between unrelated shots moves the block means by a tenth to
        // a third of full scale; the factor brings that up to the histogram's
        // range so neither half drowns the other.
        0.5 * hist + 0.5 * (blocks * 3.0).min(1.0)
    }
}

/// The score of every analysed frame against the one before it. The first
/// frame scores 0.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scores {
    pub times: Vec<Micros>,
    pub scores: Vec<f32>,
    pub fps: f64,
}

/// Decode `walk` and score every frame. Cached by file, range and algorithm.
pub fn score(job: &Walk, ctx: Option<&JobContext>) -> Result<Scores, String> {
    let key = cache::key(ALGORITHM, &job.path, job.range);
    if let Some(cached) = key.as_deref().and_then(cache::load::<Scores>) {
        return Ok(cached);
    }
    let mut out = Scores {
        fps: job.max_rate.unwrap_or(job.fps).min(job.fps),
        ..Default::default()
    };
    let mut previous: Option<Signature> = None;
    walk(job, ctx, (0.0, 1.0), |frame: Frame| {
        let signature = Signature::of(&frame.rgba, frame.width, frame.height);
        let score = previous.as_ref().map_or(0.0, |p| signature.distance(p));
        out.times.push(frame.pts);
        out.scores.push(score);
        previous = Some(signature);
        Ok(())
    })?;
    if let Some(key) = key {
        cache::store(&key, &out);
    }
    Ok(out)
}

/// The threshold a sensitivity `0..=1` stands for: how far above its
/// neighbours a frame's score must stand.
pub fn threshold(sensitivity: f32) -> f32 {
    0.5 - 0.38 * sensitivity.clamp(0.0, 1.0)
}

/// The indices of the frames that start a new shot.
pub fn detect(scores: &Scores, sensitivity: f32) -> Vec<usize> {
    let n = scores.scores.len();
    if n < 2 {
        return Vec::new();
    }
    let fps = if scores.fps.is_finite() && scores.fps > 1.0 {
        scores.fps
    } else {
        30.0
    };
    let min_gap = ((MIN_SHOT * fps).round() as usize).max(2);
    let threshold = threshold(sensitivity);
    let mut window = Vec::with_capacity(2 * WINDOW);
    let mut candidates: Vec<(usize, f32)> = Vec::new();
    for i in 1..n {
        window.clear();
        let lo = i.saturating_sub(WINDOW).max(1);
        let hi = (i + WINDOW).min(n - 1);
        window.extend((lo..=hi).filter(|&j| j != i).map(|j| scores.scores[j]));
        window.sort_by(f32::total_cmp);
        let baseline = window.get(window.len() / 2).copied().unwrap_or(0.0);
        let lift = scores.scores[i] - baseline;
        if lift >= threshold {
            candidates.push((i, lift));
        }
    }
    // Strongest first; a candidate too close to a stronger one is dropped.
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut kept: Vec<usize> = Vec::new();
    for (i, _) in candidates {
        if kept.iter().all(|&k| k.abs_diff(i) >= min_gap) {
            kept.push(i);
        }
    }
    kept.sort_unstable();
    kept
}

/// Source times of the cuts in `scores`.
pub fn cut_times(scores: &Scores, sensitivity: f32) -> Vec<Micros> {
    detect(scores, sensitivity)
        .into_iter()
        .map(|i| scores.times[i])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(rgb: [u8; 3], noise: u8, seed: u32) -> Vec<u8> {
        let mut state = seed.wrapping_mul(2_654_435_761).max(1);
        let mut out = Vec::with_capacity(160 * 90 * 4);
        for _ in 0..160 * 90 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let n = (state % (noise as u32 + 1)) as u8;
            out.extend_from_slice(&[
                rgb[0].saturating_add(n),
                rgb[1].saturating_add(n),
                rgb[2].saturating_add(n),
                255,
            ]);
        }
        out
    }

    fn scores_of(frames: &[Vec<u8>]) -> Scores {
        let mut scores = Scores {
            fps: 30.0,
            ..Default::default()
        };
        let mut previous: Option<Signature> = None;
        for (i, f) in frames.iter().enumerate() {
            let s = Signature::of(f, 160, 90);
            scores.times.push(i as Micros * 33_333);
            scores
                .scores
                .push(previous.as_ref().map_or(0.0, |p| s.distance(p)));
            previous = Some(s);
        }
        scores
    }

    #[test]
    fn a_change_of_shot_is_a_cut_and_noise_is_not() {
        let mut frames = Vec::new();
        for i in 0..30 {
            frames.push(solid([200, 40, 40], 20, i));
        }
        for i in 30..60 {
            frames.push(solid([30, 60, 200], 20, i));
        }
        for i in 60..90 {
            frames.push(solid([30, 180, 60], 20, i));
        }
        let scores = scores_of(&frames);
        assert_eq!(detect(&scores, 0.5), vec![30, 60]);
        assert_eq!(cut_times(&scores, 0.5)[0], 30 * 33_333);
    }

    #[test]
    fn a_flash_frame_counts_once() {
        let mut frames: Vec<Vec<u8>> = (0..40).map(|i| solid([80, 80, 80], 10, i)).collect();
        frames[20] = solid([255, 255, 255], 0, 99);
        let cuts = detect(&scores_of(&frames), 0.5);
        assert!(cuts.len() <= 1, "{cuts:?}");
    }

    #[test]
    fn lower_sensitivity_never_finds_more() {
        let frames: Vec<Vec<u8>> = (0..60)
            .map(|i| solid([(i * 4) as u8, 100, 255 - (i * 4) as u8], 30, i as u32))
            .collect();
        let scores = scores_of(&frames);
        assert!(detect(&scores, 0.1).len() <= detect(&scores, 0.9).len());
        // A slow fade is not a string of cuts at the default.
        assert!(detect(&scores, 0.5).is_empty());
    }
}
