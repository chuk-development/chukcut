//! The audio a transcriber hears: the timeline's mix, 16 kHz mono.
//!
//! The *mix* rather than one clip's file, because captions belong to the
//! timeline: what was cut out must not be captioned, a clip that was moved must
//! be captioned where it now is, and a voiceover on its own lane counts. The
//! playback mixer already answers "what does the timeline sound like at
//! instant t", so this asks it at 16 kHz — the rate every Whisper model was
//! trained on — and averages the two channels. Times in the transcript are then
//! timeline times with no mapping at all.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::modules::audio::{plan, ClipFactory, FileClipFactory, TimelineMixer, MIX_CHANNELS};
use crate::modules::project::{Micros, Project};

/// Whisper's sample rate.
pub const RATE: u32 = 16_000;

/// Samples to microseconds at [`RATE`].
pub fn micros(samples: usize) -> Micros {
    (samples as i64 * 1_000_000) / RATE as i64
}

/// The whole timeline, mixed down to 16 kHz mono `f32`.
pub fn timeline_audio(project: &Project, cancel: &AtomicBool) -> Result<Vec<f32>, String> {
    timeline_audio_with(project, Arc::new(FileClipFactory), cancel)
}

pub fn timeline_audio_with(
    project: &Project,
    factory: Arc<dyn ClipFactory>,
    cancel: &AtomicBool,
) -> Result<Vec<f32>, String> {
    let planned = plan(project);
    if planned.is_empty() {
        return Err("the timeline has no sound to caption".to_string());
    }
    let mut mixer = TimelineMixer::new(RATE, factory);
    mixer.set_plan(planned);

    let total = (project.duration().max(0) as u128 * RATE as u128 / 1_000_000) as usize;
    let mut mono = Vec::with_capacity(total);
    // Ten seconds per block: big enough that the per-block overhead is
    // nothing, small enough to notice a cancel quickly.
    let block = RATE as usize * 10;
    let mut stereo = vec![0f32; block * MIX_CHANNELS];
    let mut at = 0usize;
    while at < total {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let frames = block.min(total - at);
        let out = &mut stereo[..frames * MIX_CHANNELS];
        mixer.fill(at as i64, out);
        mono.extend(
            out.as_chunks::<MIX_CHANNELS>()
                .0
                .iter()
                .map(|frame| frame.iter().sum::<f32>() / MIX_CHANNELS as f32),
        );
        at += frames;
    }
    Ok(mono)
}

/// 16-bit PCM WAV, mono, [`RATE`]: what every transcription API accepts, at
/// 32 KB a second — ten minutes is 19 MB, under OpenAI's and Groq's 25 MB.
pub fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Ten minutes: 19 MB as WAV, comfortably under the common 25 MB limit.
pub const MAX_CHUNK: usize = RATE as usize * 600;
/// How far back from a chunk's end to look for a pause to cut at.
pub const CUT_SEARCH: usize = RATE as usize * 30;

/// Split `samples` into ranges of at most `max_len`, each cut at the quietest
/// moment in the last `search` samples before the limit — so a cut falls
/// between words rather than through one, which would garble a word on each
/// side of it.
pub fn chunks(samples: &[f32], max_len: usize, search: usize) -> Vec<Range<usize>> {
    let max_len = max_len.max(1);
    let window = (RATE as usize / 20).max(1); // 50 ms
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while samples.len() - start > max_len {
        let limit = start + max_len;
        let from = limit.saturating_sub(search).max(start + window);
        let mut best = limit;
        let mut quietest = f32::INFINITY;
        let mut at = from;
        while at + window <= limit {
            let energy: f32 = samples[at..at + window].iter().map(|s| s * s).sum();
            if energy < quietest {
                quietest = energy;
                best = at + window / 2;
            }
            at += window / 2;
        }
        ranges.push(start..best);
        start = best;
    }
    if start < samples.len() || ranges.is_empty() {
        ranges.push(start..samples.len());
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_header_describes_16k_mono_pcm() {
        let wav = wav_bytes(&[0.0, 1.0, -1.0, 2.0]);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        assert_eq!(wav.len(), 44 + 8);
        // Clipped, not wrapped.
        assert_eq!(i16::from_le_bytes([wav[50], wav[51]]), i16::MAX);
    }

    #[test]
    fn chunks_cover_everything_and_cut_in_the_quiet() {
        // Loud everywhere except a pause at 2.5 s.
        let len = RATE as usize * 6;
        let pause = RATE as usize * 5 / 2;
        let samples: Vec<f32> = (0..len)
            .map(|i| {
                if (pause..pause + 1600).contains(&i) {
                    0.0
                } else {
                    0.5
                }
            })
            .collect();
        let ranges = chunks(&samples, RATE as usize * 3, RATE as usize);
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, len);
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "no gap, no overlap");
        }
        for range in &ranges {
            assert!(range.len() <= RATE as usize * 3);
        }
        let cut = ranges[0].end;
        assert!(
            (pause..pause + 1600).contains(&cut),
            "cut at {cut}, the pause is at {pause}"
        );
    }

    #[test]
    fn short_audio_is_one_chunk() {
        assert_eq!(chunks(&[0.0; 100], 1000, 10), vec![0..100]);
        assert_eq!(chunks(&[], 1000, 10), vec![0..0]);
    }

    #[test]
    fn an_empty_timeline_has_nothing_to_caption() {
        let project = Project::new("t", Default::default(), 30.0);
        assert!(timeline_audio(&project, &AtomicBool::new(false)).is_err());
    }
}
