//! From a clip's sound to a level envelope: one RMS value per 10 ms window.
//!
//! Detection runs on the envelope, never on the samples. Decoding is the
//! expensive half (seconds for a long take); thresholding a few thousand
//! numbers is microseconds. So the review panel decodes once when it opens and
//! re-runs detection on every slider move without touching the file again.
//!
//! The window is 10 ms at 48 kHz — 480 samples — because that is RNNoise's
//! frame. The optional voice probability comes out of the same frames, so the
//! two series line up index for index without any resampling.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::modules::audio::decode::{AudioClipReader, ClipReader};
use crate::modules::export::audio::frames_for;
use crate::modules::project::document::{Micros, TimeRange};

/// The rate everything here is decoded at. RNNoise only runs at 48 kHz.
pub const ANALYSIS_RATE: u32 = 48_000;
/// One envelope window, in microseconds.
pub const WINDOW: Micros = 10_000;
/// One envelope window, in sample frames at [`ANALYSIS_RATE`].
pub const WINDOW_FRAMES: usize = 480;
/// What an all-zero window reads as. Low enough to be below any threshold a
/// slider offers, finite so it serialises.
pub const FLOOR_DB: f32 = -100.0;

/// How much is decoded per read: ten seconds. Bounded so a one-hour take does
/// not need its whole PCM in memory at once.
const CHUNK_FRAMES: usize = WINDOW_FRAMES * 1_000;

/// The level of a stretch of source audio, window by window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// Source time of window 0, in microseconds from the start of the file.
    pub start: Micros,
    /// Window length in microseconds. Always [`WINDOW`] today; stored so a
    /// consumer never has to know that.
    pub window: Micros,
    /// RMS per window in dBFS, at least [`FLOOR_DB`].
    pub db: Vec<f32>,
    /// RNNoise's voice probability per window, `0..=1`, when asked for.
    #[serde(default)]
    pub voice: Option<Vec<f32>>,
}

impl Envelope {
    /// Source time just past the last window.
    pub fn end(&self) -> Micros {
        self.start + self.db.len() as Micros * self.window
    }

    /// The windows that overlap `range`, as an index range.
    pub fn windows_in(&self, range: TimeRange) -> std::ops::Range<usize> {
        if self.window <= 0 || self.db.is_empty() {
            return 0..0;
        }
        let first = ((range.start - self.start).max(0) / self.window) as usize;
        let last = ((range.end() - self.start).max(0) + self.window - 1) / self.window;
        first.min(self.db.len())..(last as usize).min(self.db.len())
    }

    /// The loudest window inside `range`, in dBFS.
    pub fn peak_db(&self, range: TimeRange) -> f32 {
        self.db[self.windows_in(range)]
            .iter()
            .copied()
            .fold(FLOOR_DB, f32::max)
    }

    /// `count` bars of linear amplitude (`0..=1`) covering `range`: what a mini
    /// waveform draws. Each bar is the loudest window under it, so a short
    /// word inside a long gap is still visible.
    pub fn bars(&self, range: TimeRange, count: usize) -> Vec<f32> {
        if count == 0 || range.duration <= 0 {
            return Vec::new();
        }
        (0..count)
            .map(|i| {
                let from = range.start + range.duration * i as Micros / count as Micros;
                let to = range.start + range.duration * (i as Micros + 1) / count as Micros;
                let db = self.peak_db(TimeRange::new(from, (to - from).max(1)));
                db_to_amplitude(db)
            })
            .collect()
    }
}

/// A dBFS value as a `0..=1` bar height, on a 60 dB scale: quieter than
/// −60 dBFS draws as nothing, which is where room tone usually sits.
pub fn db_to_amplitude(db: f32) -> f32 {
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Builds an [`Envelope`] from mono samples pushed in pieces of any size.
pub struct EnvelopeBuilder {
    start: Micros,
    pending: Vec<f32>,
    db: Vec<f32>,
    voice: Option<(Box<nnnoiseless::DenoiseState<'static>>, Vec<f32>)>,
}

impl EnvelopeBuilder {
    pub fn new(start: Micros, with_voice: bool) -> Self {
        Self {
            start,
            pending: Vec::with_capacity(WINDOW_FRAMES),
            db: Vec::new(),
            voice: with_voice.then(|| (nnnoiseless::DenoiseState::new(), Vec::new())),
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (WINDOW_FRAMES - self.pending.len()).min(rest.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() == WINDOW_FRAMES {
                self.window_done();
            }
        }
    }

    fn window_done(&mut self) {
        let window = std::mem::take(&mut self.pending);
        let energy: f64 = window.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        let rms = (energy / window.len().max(1) as f64).sqrt();
        let db = if rms > 0.0 {
            (20.0 * rms.log10()) as f32
        } else {
            FLOOR_DB
        };
        self.db.push(db.max(FLOOR_DB));

        if let Some((state, probabilities)) = &mut self.voice {
            // RNNoise wants 16-bit scale in an f32.
            let input: Vec<f32> = window.iter().map(|s| s * 32_768.0).collect();
            let mut output = [0.0f32; WINDOW_FRAMES];
            probabilities.push(state.process_frame(&mut output, &input).clamp(0.0, 1.0));
        }
        self.pending = window;
        self.pending.clear();
    }

    /// Close the envelope. A trailing partial window is measured as it is
    /// rather than dropped, so the envelope covers the whole range.
    pub fn finish(mut self) -> Envelope {
        if !self.pending.is_empty() {
            self.pending.resize(WINDOW_FRAMES, 0.0);
            self.window_done();
        }
        Envelope {
            start: self.start,
            window: WINDOW,
            db: self.db,
            voice: self.voice.map(|(_, v)| v),
        }
    }
}

/// The envelope of `mono` samples at [`ANALYSIS_RATE`] that start at source
/// time `start`.
pub fn envelope_from_pcm(mono: &[f32], start: Micros, with_voice: bool) -> Envelope {
    let mut builder = EnvelopeBuilder::new(start, with_voice);
    builder.push(mono);
    builder.finish()
}

/// Decode `range` of the file at `path` and measure it.
///
/// Mono, because a pause is a pause on every channel and summing first halves
/// the work. Blocking and slow — seconds for a long take — so callers run it
/// off the UI thread; `cancel` is checked between chunks.
pub fn envelope_from_file(
    path: &str,
    range: TimeRange,
    with_voice: bool,
    cancel: &AtomicBool,
) -> Result<Envelope, String> {
    let mut reader = AudioClipReader::open(path, ANALYSIS_RATE, 1).map_err(|e| e.to_string())?;
    let mut builder = EnvelopeBuilder::new(range.start, with_voice);
    let first = frames_for(range.start.max(0), ANALYSIS_RATE) as i64;
    let total = frames_for(range.duration, ANALYSIS_RATE);
    let mut done = 0usize;
    let mut chunk = vec![0.0f32; CHUNK_FRAMES];
    while done < total {
        if cancel.load(Ordering::Relaxed) {
            return Err("the analysis was cancelled".into());
        }
        let frames = CHUNK_FRAMES.min(total - done);
        let out = &mut chunk[..frames];
        reader
            .read(first + done as i64, frames, out)
            .map_err(|e| e.to_string())?;
        builder.push(out);
        done += frames;
    }
    Ok(builder.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, amplitude: f32) -> Vec<f32> {
        (0..frames)
            .map(|i| amplitude * (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect()
    }

    #[test]
    fn a_full_scale_sine_measures_minus_three_db() {
        let env = envelope_from_pcm(&tone(48_000, 1.0), 0, false);
        assert_eq!(env.db.len(), 100);
        for db in &env.db {
            assert!((db + 3.01).abs() < 0.1, "got {db}");
        }
    }

    #[test]
    fn silence_reads_as_the_floor_and_the_tail_window_is_kept() {
        let env = envelope_from_pcm(&vec![0.0; 1_000], 5_000, false);
        // 1000 frames is two full windows and a partial one.
        assert_eq!(env.db.len(), 3);
        assert!(env.db.iter().all(|db| *db == FLOOR_DB));
        assert_eq!(env.start, 5_000);
        assert_eq!(env.end(), 35_000);
    }

    #[test]
    fn windows_in_maps_source_time_to_indices() {
        let env = envelope_from_pcm(&vec![0.0; 48_000], 1_000_000, false);
        assert_eq!(env.windows_in(TimeRange::new(1_000_000, 20_000)), 0..2);
        assert_eq!(env.windows_in(TimeRange::new(1_015_000, 10_000)), 1..3);
        // Past either end clamps rather than panicking.
        assert_eq!(env.windows_in(TimeRange::new(0, 10_000_000)), 0..100);
    }

    #[test]
    fn bars_show_a_short_loud_word_inside_a_long_gap() {
        let mut pcm = vec![0.0; 48_000];
        pcm[24_000..24_480].copy_from_slice(&tone(480, 0.9));
        let env = envelope_from_pcm(&pcm, 0, false);
        let bars = env.bars(TimeRange::new(0, 1_000_000), 4);
        assert_eq!(bars.len(), 4);
        assert_eq!(bars[0], 0.0);
        assert!(bars[2] > 0.8);
    }

    #[test]
    fn the_voice_series_lines_up_with_the_levels() {
        let env = envelope_from_pcm(&tone(9_600, 0.5), 0, true);
        let voice = env.voice.expect("asked for voice");
        assert_eq!(voice.len(), env.db.len());
        assert!(voice.iter().all(|p| (0.0..=1.0).contains(p)));
    }
}
