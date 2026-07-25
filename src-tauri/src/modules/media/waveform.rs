//! Audio peaks for waveform drawing.
//!
//! The timeline draws an audio lane as a bar per horizontal pixel, so what it
//! needs is not samples but *peaks*: for each of N equal slices of the file,
//! the loudest thing in that slice. A minute of 48 kHz stereo is 5.7 million
//! samples and about 800 pixels of lane, so reducing here rather than in the
//! webview is the difference between a few kilobytes of JSON and 45 megabytes.
//!
//! Peaks, not averages: an average smears a drum hit into the silence around it
//! and the waveform stops lining up with what the user hears. Max across
//! channels rather than a downmix, for the same reason — a sound panned hard to
//! one side must not look half as loud as one in the centre.

use std::path::Path;

use ffmpeg_next as ffmpeg;
use ffmpeg::software::resampling;
use ffmpeg::util::frame;

use super::{ensure_initialized, ts_to_micros, MediaError, Result};
use crate::modules::project::Micros;

/// Decode `path`'s audio and reduce it to `buckets` normalized peaks.
///
/// Values are `0.0..=1.0`, scaled so the loudest bucket in the file is `1.0`.
pub fn waveform(path: impl AsRef<Path>, buckets: usize) -> Result<Vec<f32>> {
    let path = path.as_ref();
    ensure_initialized();

    if buckets == 0 {
        return Err(MediaError::Invalid(
            "a waveform needs at least one bucket".into(),
        ));
    }

    let mut input = ffmpeg::format::input(&path).map_err(|source| MediaError::Open {
        path: path.to_path_buf(),
        source,
    })?;

    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or_else(|| MediaError::NoAudioStream(path.to_path_buf()))?;
    let stream_index = stream.index();
    let time_base = stream.time_base();
    let stream_duration = stream.duration();
    let parameters = stream.parameters();
    let codec_name = parameters.id().name().to_string();

    let context = ffmpeg::codec::context::Context::from_parameters(parameters).map_err(|source| {
        MediaError::Decode {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let mut decoder = context
        .decoder()
        .audio()
        .map_err(|_| MediaError::NoDecoder {
            path: path.to_path_buf(),
            codec: codec_name,
        })?;

    let rate = decoder.rate().max(1);
    let channels = decoder.channels().max(1);

    // Some containers leave the layout unset even though the channel count is
    // known; swresample refuses to initialise without one, so fall back to the
    // canonical layout for that many channels.
    let mut layout = decoder.channel_layout();
    if layout.bits() == 0 {
        layout = ffmpeg::ChannelLayout::default(channels as i32);
        decoder.set_channel_layout(layout);
    }

    // Resample only the sample *format*, not the rate or the layout. Every
    // codec has its own idea of a sample (planar floats, packed 16-bit
    // integers, 24-bit in 32-bit containers); converting to packed f32 means
    // one code path below instead of one per format, and costs nothing else
    // because the rate and layout pass through untouched.
    let mut resampler = resampling::Context::get(
        decoder.format(),
        layout,
        rate,
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        layout,
        rate,
    )
    .map_err(|source| MediaError::Decode {
        path: path.to_path_buf(),
        source,
    })?;

    let duration = if stream_duration > 0 {
        ts_to_micros(stream_duration, time_base)
    } else {
        input.duration().max(0)
    };
    let mut peaks = PeakAccumulator::new(buckets, expected_sample_count(duration, rate), channels);

    let mut packet = ffmpeg::Packet::empty();
    loop {
        match packet.read(&mut input) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(source) => {
                return Err(MediaError::Decode {
                    path: path.to_path_buf(),
                    source,
                })
            }
        }
        if packet.stream() != stream_index {
            continue;
        }
        decoder
            .send_packet(&packet)
            .map_err(|source| MediaError::Decode {
                path: path.to_path_buf(),
                source,
            })?;
        drain(path, &mut decoder, &mut resampler, &mut peaks)?;
    }

    // Codecs with a decode delay hold the tail of the file until told the
    // stream ended; without this the waveform stops short of the clip.
    decoder.send_eof().map_err(|source| MediaError::Decode {
        path: path.to_path_buf(),
        source,
    })?;
    drain(path, &mut decoder, &mut resampler, &mut peaks)?;

    Ok(peaks.finish())
}

/// Pull every frame the decoder currently has, convert it, and fold it in.
fn drain(
    path: &Path,
    decoder: &mut ffmpeg::decoder::Audio,
    resampler: &mut resampling::Context,
    peaks: &mut PeakAccumulator,
) -> Result<()> {
    loop {
        let mut decoded = frame::Audio::empty();
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {}
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => return Ok(()),
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(source) => {
                return Err(MediaError::Decode {
                    path: path.to_path_buf(),
                    source,
                })
            }
        }

        let mut converted = frame::Audio::empty();
        resampler
            .run(&decoded, &mut converted)
            .map_err(|source| MediaError::Decode {
                path: path.to_path_buf(),
                source,
            })?;
        peaks.push(&converted);
    }
}

/// Folds interleaved f32 sample frames into per-bucket peaks.
struct PeakAccumulator {
    peaks: Vec<f32>,
    /// Sample frames folded in so far; this is the x axis of the waveform.
    seen: u64,
    /// Sample frames the container says the stream holds. Real files miss this
    /// by a little in both directions, which `bucket_for` absorbs.
    expected: u64,
    channels: usize,
}

impl PeakAccumulator {
    fn new(buckets: usize, expected: u64, channels: u16) -> Self {
        Self {
            peaks: vec![0.0; buckets],
            seen: 0,
            expected,
            channels: (channels as usize).max(1),
        }
    }

    /// Fold in one converted frame of packed f32 samples.
    ///
    /// The byte count is computed from `samples()` rather than trusting the
    /// slice `data()` hands back: that slice is the whole *allocated* line, and
    /// the resampler is allowed to write fewer samples than were allocated, so
    /// reading all of it would fold uninitialised tail bytes into the peaks.
    fn push(&mut self, converted: &frame::Audio) {
        const BYTES: usize = std::mem::size_of::<f32>();

        let bytes = converted.data(0);
        let wanted = converted.samples() * self.channels * BYTES;
        let usable = wanted.min(bytes.len() - bytes.len() % (self.channels * BYTES));

        for group in bytes[..usable].chunks_exact(self.channels * BYTES) {
            // Packed f32 is native-endian, so this is a reinterpretation rather
            // than a byte-order conversion.
            let samples: Vec<f32> = group
                .chunks_exact(BYTES)
                .map(|s| f32::from_ne_bytes([s[0], s[1], s[2], s[3]]))
                .collect();
            let peak = peak_across_channels(&samples);

            let bucket = bucket_for(self.seen, self.expected, self.peaks.len());
            self.seen += 1;
            if peak > self.peaks[bucket] {
                self.peaks[bucket] = peak;
            }
        }
    }

    fn finish(self) -> Vec<f32> {
        normalize(self.peaks)
    }
}

/// Loudest channel of one sample frame, as a magnitude.
///
/// Max rather than a downmix: averaging a hard-panned sound with the silent
/// opposite channel halves it, and the lane would show a quiet passage where
/// the user hears a loud one. Absolute value because a waveform is symmetric —
/// the sign of a sample carries no information at this scale.
fn peak_across_channels(samples: &[f32]) -> f32 {
    samples
        .iter()
        .filter(|s| s.is_finite())
        .fold(0.0f32, |acc, s| acc.max(s.abs()))
}

/// How many sample frames a stream of `duration` at `rate` should contain.
fn expected_sample_count(duration: Micros, rate: u32) -> u64 {
    if duration <= 0 {
        return 0;
    }
    (duration as i128 * rate as i128 / 1_000_000) as u64
}

/// Which bucket a sample frame falls into.
///
/// `total` is the *expected* number of frames, derived from the container's
/// duration, which real files under- and over-shoot. Clamping is what keeps a
/// file that runs a few milliseconds long from panicking on the last bucket,
/// and a container that lies about its duration falls back to spreading
/// everything across the buckets in order.
fn bucket_for(index: u64, total: u64, buckets: usize) -> usize {
    debug_assert!(buckets > 0);
    if total == 0 {
        return (index as usize).min(buckets - 1);
    }
    let bucket = (index as u128 * buckets as u128 / total as u128) as usize;
    bucket.min(buckets - 1)
}

/// Scale peaks so the loudest bucket is 1.0.
///
/// A clip mastered quietly would otherwise draw as a flat line in the lane and
/// be useless for finding beats, which is the only thing the waveform is for.
/// A silent file stays all zeros rather than being amplified into noise.
fn normalize(mut peaks: Vec<f32>) -> Vec<f32> {
    let loudest = peaks.iter().copied().fold(0.0f32, f32::max);
    if loudest <= 0.0 || !loudest.is_finite() {
        return peaks.iter().map(|_| 0.0).collect();
    }
    for peak in &mut peaks {
        *peak = (*peak / loudest).clamp(0.0, 1.0);
    }
    peaks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_spread_samples_evenly() {
        assert_eq!(bucket_for(0, 100, 10), 0);
        assert_eq!(bucket_for(9, 100, 10), 0);
        assert_eq!(bucket_for(10, 100, 10), 1);
        assert_eq!(bucket_for(99, 100, 10), 9);
    }

    #[test]
    fn buckets_clamp_when_a_file_runs_past_its_declared_duration() {
        // A container that under-reports its length must not index past the end.
        assert_eq!(bucket_for(1_000, 100, 10), 9);
        assert_eq!(bucket_for(u64::MAX, 100, 10), 9);
    }

    #[test]
    fn buckets_without_a_known_duration_fill_in_order() {
        assert_eq!(bucket_for(0, 0, 4), 0);
        assert_eq!(bucket_for(3, 0, 4), 3);
        assert_eq!(bucket_for(9, 0, 4), 3);
    }

    #[test]
    fn bucket_math_does_not_overflow_on_long_files() {
        // Three hours of 48 kHz audio overflows a u64 multiply by bucket count
        // if the arithmetic is not widened.
        let total = 3 * 60 * 60 * 48_000;
        assert_eq!(bucket_for(0, total, 2_000), 0);
        assert_eq!(bucket_for(total - 1, total, 2_000), 1_999);
        assert_eq!(bucket_for(total / 2, total, 2_000), 1_000);
    }

    #[test]
    fn stereo_peaks_take_the_louder_channel() {
        // A sound panned hard left must read as loud, not as half as loud.
        assert_eq!(peak_across_channels(&[0.8, 0.0]), 0.8);
        assert_eq!(peak_across_channels(&[0.0, 0.8]), 0.8);
        assert_eq!(peak_across_channels(&[0.2, 0.5, 0.1]), 0.5);
    }

    #[test]
    fn peaks_are_magnitudes() {
        assert_eq!(peak_across_channels(&[-0.9, 0.1]), 0.9);
        assert_eq!(peak_across_channels(&[]), 0.0);
    }

    #[test]
    fn peaks_ignore_non_finite_samples() {
        // Broken float streams do turn up; one NaN must not blank the file.
        assert_eq!(peak_across_channels(&[f32::NAN, 0.3]), 0.3);
        assert_eq!(peak_across_channels(&[f32::INFINITY, 0.3]), 0.3);
    }

    #[test]
    fn expected_sample_count_follows_duration() {
        assert_eq!(expected_sample_count(1_000_000, 48_000), 48_000);
        assert_eq!(expected_sample_count(500_000, 44_100), 22_050);
        assert_eq!(expected_sample_count(0, 48_000), 0);
        assert_eq!(expected_sample_count(-1, 48_000), 0);
    }

    #[test]
    fn normalization_puts_the_loudest_bucket_at_one() {
        let peaks = normalize(vec![0.1, 0.25, 0.5, 0.05]);
        assert_eq!(peaks, vec![0.2, 0.5, 1.0, 0.1]);
    }

    #[test]
    fn silence_normalizes_to_silence() {
        assert_eq!(normalize(vec![0.0, 0.0, 0.0]), vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn normalization_clamps_samples_above_full_scale() {
        // Float codecs are allowed to exceed 1.0; the lane cannot draw that.
        let peaks = normalize(vec![1.5, 0.75]);
        assert!(peaks.iter().all(|p| (0.0..=1.0).contains(p)));
        assert_eq!(peaks[0], 1.0);
        assert_eq!(peaks[1], 0.5);
    }
}
