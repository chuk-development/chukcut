//! Audio envelopes for waveform drawing.
//!
//! The timeline draws an audio lane as a column per horizontal pixel, so what
//! it needs is not samples but a *reduction*: for each of N equal slices of the
//! file, enough to draw that slice. A minute of 48 kHz stereo is 5.7 million
//! samples and about 800 pixels of lane, so reducing here rather than in the
//! webview is the difference between a few kilobytes of JSON and 45 megabytes.
//!
//! ## Three numbers per bucket, not one
//!
//! A peak alone draws a solid block for anything compressed or limited — which
//! is most music and all of modern speech — because the peak of a 10 ms slice
//! of a loud passage is 1.0 and so is the peak of the slice next to it. What
//! makes a waveform readable is the *difference* between the outline and the
//! body: a signed minimum and maximum give the outline, and the RMS gives the
//! body, so a quiet passage and a limited one no longer look identical. That is
//! why every drawing tool that gets this right (Audacity, Reaper, Ableton)
//! stores exactly these three.
//!
//! Signed rather than absolute, and per-channel maxima rather than a downmix:
//! averaging a hard-panned sound with the silent opposite channel halves it,
//! and the lane would show a quiet passage where the user hears a loud one.
//!
//! ## Resolution is the cache's business, not the caller's
//!
//! Zooming the timeline changes how many columns the lane has, and re-decoding
//! a file on every zoom step is unaffordable. So the decode runs once at a
//! fixed [`CACHE_BUCKETS_PER_SECOND`], the result is cached on disk under
//! [`workspace::paths::waveform_dir`], and any resolution the caller asks for
//! is reduced out of that in memory. The bucket count in the request is
//! therefore free: it costs a fold over an array, not a decode.
//!
//! [`workspace::paths::waveform_dir`]: crate::modules::workspace::paths::waveform_dir

use std::path::{Path, PathBuf};

use ffmpeg::software::resampling;
use ffmpeg::util::frame;
use ffmpeg_next as ffmpeg;
use serde::{Deserialize, Serialize};

use super::{ensure_initialized, ts_to_micros, MediaError, Result};
use crate::modules::project::Micros;
use crate::modules::workspace::paths;

/// Ceiling on what a caller may ask for.
///
/// A material occupies at most a screen's width of lane, and the timeline
/// quantises its requests to a ladder that tops out well below this. Anything
/// past it is a mistake that would cost megabytes of JSON to answer.
const MAX_REQUEST_BUCKETS: usize = 20_000;

/// Buckets per second of audio held on disk — one per millisecond.
///
/// Finer than a lane is ever drawn for any clip longer than a few seconds, so
/// every zoom level is served by reduction rather than by another decode.
const CACHE_BUCKETS_PER_SECOND: u32 = 1_000;

/// Ceiling on the stored resolution, and deliberately equal to
/// [`MAX_REQUEST_BUCKETS`]: storing more columns than any caller may ask for is
/// disk space that can never be read. It puts a 240 KB bound on one file's
/// cache, whether it is twenty seconds long or three hours.
const MAX_CACHE_BUCKETS: usize = MAX_REQUEST_BUCKETS;

/// What a file with no declared duration gets. Nothing can be spread evenly
/// across a length nobody knows, so this is a fallback shape, not a resolution.
const UNKNOWN_DURATION_BUCKETS: usize = 8_192;

/// Bumped when the cache file's layout or the meaning of its numbers changes.
/// An older file is ignored and recomputed rather than migrated — it is a
/// cache, and regenerating it costs one decode.
const CACHE_VERSION: u32 = 1;

const CACHE_MAGIC: [u8; 4] = *b"CKWF";
/// magic + version + buckets + rate + channels + duration + peak.
const CACHE_HEADER: usize = 4 + 4 + 4 + 4 + 4 + 8 + 4;

/// A file's audio reduced to `buckets` equal slices.
///
/// Every value is normalized so the loudest sample in the file sits at ±1.0;
/// `peak` carries the divisor, so a caller that wants absolute amplitude can
/// multiply it back. Normalizing is what keeps a quietly mastered clip from
/// drawing as a flat line, which is the only thing the lane is for.
///
/// **`min <= 0 <= max` always holds**, by construction rather than by luck: the
/// per-bucket extremes start at zero, so a signal with a DC offset — or a bucket
/// covering only the rising half of a wave — still draws as a column anchored on
/// the baseline. A lane whose columns floated off the centre line would read as
/// the editor having broken the audio, and the offset is not something the user
/// can act on from the timeline anyway. The frontend may rely on this: every
/// column runs from `min` up to `max` through zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Waveform {
    /// Length of `min`, `max` and `rms`. Equal to what the caller asked for.
    pub buckets: usize,
    /// Length of the audio the envelope covers.
    pub duration: Micros,
    pub sample_rate: u32,
    pub channels: u16,
    /// Most negative sample in each bucket, normalized. `-1.0..=0.0`.
    pub min: Vec<f32>,
    /// Most positive sample in each bucket, normalized. `0.0..=1.0`.
    pub max: Vec<f32>,
    /// Root mean square of each bucket, normalized. `0.0..=1.0`, and never
    /// larger than `max(|min|, |max|)` of the same bucket.
    pub rms: Vec<f32>,
    /// The full-scale amplitude that was divided out. Zero for a silent file.
    pub peak: f32,
}

impl Waveform {
    fn empty(buckets: usize) -> Self {
        Self {
            buckets,
            duration: 0,
            sample_rate: 0,
            channels: 0,
            min: vec![0.0; buckets],
            max: vec![0.0; buckets],
            rms: vec![0.0; buckets],
            peak: 0.0,
        }
    }

    /// Reduce (or, past the stored resolution, stretch) this envelope to
    /// `buckets` columns.
    ///
    /// Extremes combine by taking the extreme — a transient that survived into
    /// the stored envelope must survive into the drawn one, which is the whole
    /// reason the outline is kept separately from the body. RMS combines in the
    /// energy domain, because the mean of two RMS values is not the RMS of
    /// their union and averaging them would quietly flatten the body.
    pub fn resampled(&self, buckets: usize) -> Self {
        if buckets == self.buckets {
            return self.clone();
        }
        let source = self.buckets;
        if source == 0 || buckets == 0 {
            return Self {
                duration: self.duration,
                sample_rate: self.sample_rate,
                channels: self.channels,
                peak: self.peak,
                ..Self::empty(buckets)
            };
        }

        let mut min = Vec::with_capacity(buckets);
        let mut max = Vec::with_capacity(buckets);
        let mut rms = Vec::with_capacity(buckets);

        for index in 0..buckets {
            let (from, to) = span(index, buckets, source);
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            let mut energy = 0.0f64;
            for i in from..to {
                lo = lo.min(self.min[i]);
                hi = hi.max(self.max[i]);
                energy += (self.rms[i] as f64) * (self.rms[i] as f64);
            }
            min.push(lo);
            max.push(hi);
            rms.push((energy / (to - from) as f64).sqrt() as f32);
        }

        Self {
            buckets,
            duration: self.duration,
            sample_rate: self.sample_rate,
            channels: self.channels,
            min,
            max,
            rms,
            peak: self.peak,
        }
    }
}

/// Which stored buckets output bucket `index` of `buckets` covers.
///
/// Always at least one, so asking for more columns than were stored stretches
/// the envelope rather than producing empty ones.
fn span(index: usize, buckets: usize, source: usize) -> (usize, usize) {
    let from = (index as u128 * source as u128 / buckets as u128) as usize;
    let to = ((index as u128 + 1) * source as u128 / buckets as u128) as usize;
    let from = from.min(source - 1);
    (from, to.max(from + 1).min(source))
}

/// Decode `path`'s audio and reduce it to `buckets` columns of envelope.
///
/// Answers from the disk cache when one is there for this exact file — the key
/// folds in size and modification time, so re-exporting a clip to the same name
/// invalidates it rather than showing the old shape forever.
pub fn waveform(path: impl AsRef<Path>, buckets: usize) -> Result<Waveform> {
    let path = path.as_ref();

    if buckets == 0 {
        return Err(MediaError::Invalid(
            "a waveform needs at least one bucket".into(),
        ));
    }
    if buckets > MAX_REQUEST_BUCKETS {
        return Err(MediaError::Invalid(format!(
            "a waveform of {buckets} buckets is more than the {MAX_REQUEST_BUCKETS} \
             any timeline can draw"
        )));
    }

    let file = cache_file(path);
    if let Some(cached) = read_cache(&file) {
        return Ok(cached.resampled(buckets));
    }

    let stored = analyze(path)?;

    // A cache that cannot be written is not a reason to fail the request: the
    // user asked for a waveform, not for a cache, and the next call simply
    // decodes again.
    if stored.duration > 0 {
        if let Err(error) = write_cache(&file, &stored) {
            tracing::debug!(%error, path = %file.display(), "could not cache the waveform");
        }
    }

    Ok(stored.resampled(buckets))
}

/// Decode the whole file and build the envelope at the cached resolution.
fn analyze(path: &Path) -> Result<Waveform> {
    ensure_initialized();

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

    let context =
        ffmpeg::codec::context::Context::from_parameters(parameters).map_err(|source| {
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
    let layout_mask = layout.bits();

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
    let buckets = cache_buckets(duration);
    let mut envelope = Envelope::new(buckets, expected_sample_count(duration, rate), channels);

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
        drain(
            path,
            &mut decoder,
            &mut resampler,
            layout_mask,
            &mut envelope,
        )?;
    }

    // Codecs with a decode delay hold the tail of the file until told the
    // stream ended; without this the waveform stops short of the clip.
    decoder.send_eof().map_err(|source| MediaError::Decode {
        path: path.to_path_buf(),
        source,
    })?;
    drain(
        path,
        &mut decoder,
        &mut resampler,
        layout_mask,
        &mut envelope,
    )?;

    Ok(envelope.finish(duration, rate, channels))
}

/// Pull every frame the decoder currently has, convert it, and fold it in.
fn drain(
    path: &Path,
    decoder: &mut ffmpeg::decoder::Audio,
    resampler: &mut resampling::Context,
    layout_mask: u64,
    envelope: &mut Envelope,
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
        name_the_layout(&mut decoded, layout_mask);

        let mut converted = frame::Audio::empty();
        resampler
            .run(&decoded, &mut converted)
            .map_err(|source| MediaError::Decode {
                path: path.to_path_buf(),
                source,
            })?;
        envelope.push(&converted);
    }
}

/// Give a decoded frame the channel layout the resampler expects, when the file
/// did not name one.
///
/// FFmpeg 6 replaced the channel-layout bitmask with `AVChannelLayout`, which
/// distinguishes "two channels, front left and front right" from "two channels,
/// we were not told which". A plain PCM WAV — no channel mask in its `fmt `
/// chunk — decodes to the second kind, and swresample compares the frame's
/// layout against the one it was configured with and rejects anything that does
/// not match, with `Input changed`. Since `ffmpeg-next` 6.1 still configures the
/// resampler through the *legacy* mask API, that mismatch is guaranteed for
/// those files rather than unlucky: every one of them fails, and the failure
/// reads like a codec problem rather than a bookkeeping one.
///
/// Naming the layout is the fix, and it is a relabelling rather than a
/// conversion: the samples are already interleaved in the order the mask
/// describes. The same eight lines live in `audio::decode::AudioClipReader`,
/// which met this first; they are duplicated rather than shared because the two
/// modules deliberately do not depend on each other's FFmpeg handling.
fn name_the_layout(decoded: &mut frame::Audio, mask: u64) {
    // Safety: `decoded` is a frame the decoder just produced, and an
    // unspecified layout owns no allocation, so overwriting it leaks nothing.
    unsafe {
        let ptr = decoded.as_mut_ptr();
        if (*ptr).ch_layout.order == ffmpeg::ffi::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC {
            ffmpeg::ffi::av_channel_layout_from_mask(&mut (*ptr).ch_layout, mask);
        }
    }
}

/// Folds interleaved f32 sample frames into per-bucket extremes and energy.
struct Envelope {
    min: Vec<f32>,
    max: Vec<f32>,
    /// Summed square of the loudest channel per sample frame, in f64 because a
    /// long file accumulates millions of terms and f32 stops adding small ones.
    energy: Vec<f64>,
    counts: Vec<u64>,
    /// Sample frames folded in so far; this is the x axis of the waveform.
    seen: u64,
    /// Sample frames the container says the stream holds. Real files miss this
    /// by a little in both directions, which `bucket_for` absorbs.
    expected: u64,
    channels: usize,
}

impl Envelope {
    fn new(buckets: usize, expected: u64, channels: u16) -> Self {
        Self {
            min: vec![0.0; buckets],
            max: vec![0.0; buckets],
            energy: vec![0.0; buckets],
            counts: vec![0; buckets],
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
    /// reading all of it would fold uninitialised tail bytes into the envelope.
    fn push(&mut self, converted: &frame::Audio) {
        const BYTES: usize = std::mem::size_of::<f32>();

        let bytes = converted.data(0);
        let stride = self.channels * BYTES;
        if stride == 0 || bytes.is_empty() {
            return;
        }
        let wanted = converted.samples() * stride;
        let usable = wanted.min(bytes.len() - bytes.len() % stride);

        for group in bytes[..usable].chunks_exact(stride) {
            // Both extremes start at zero rather than at the first sample, which
            // is what makes `min <= 0 <= max` a guarantee the frontend can draw
            // against. See the note on [`Waveform`].
            //
            // Packed f32 is native-endian, so reading a sample below is a
            // reinterpretation rather than a byte-order conversion.
            let mut lo = 0.0f32;
            let mut hi = 0.0f32;
            let mut loudest = 0.0f32;
            for sample in group.chunks_exact(BYTES) {
                let value = f32::from_ne_bytes([sample[0], sample[1], sample[2], sample[3]]);
                // Broken float streams do turn up; one NaN must not blank a file.
                if !value.is_finite() {
                    continue;
                }
                lo = lo.min(value);
                hi = hi.max(value);
                if value.abs() > loudest.abs() {
                    loudest = value;
                }
            }

            let bucket = bucket_for(self.seen, self.expected, self.min.len());
            self.seen += 1;
            if lo < self.min[bucket] {
                self.min[bucket] = lo;
            }
            if hi > self.max[bucket] {
                self.max[bucket] = hi;
            }
            // The loudest channel rather than the mean across channels, for the
            // same reason the extremes take the loudest: a hard-panned sound
            // must not read as half as loud as a centred one.
            self.energy[bucket] += (loudest as f64) * (loudest as f64);
            self.counts[bucket] += 1;
        }
    }

    fn finish(self, duration: Micros, sample_rate: u32, channels: u16) -> Waveform {
        let buckets = self.min.len();
        let mut rms: Vec<f32> = Vec::with_capacity(buckets);
        for (energy, count) in self.energy.iter().zip(&self.counts) {
            rms.push(if *count == 0 {
                0.0
            } else {
                (energy / *count as f64).sqrt() as f32
            });
        }

        let peak = self
            .min
            .iter()
            .chain(self.max.iter())
            .fold(0.0f32, |acc, v| acc.max(v.abs()));

        let (min, max, rms) = if peak > 0.0 && peak.is_finite() {
            let scale = 1.0 / peak;
            (
                self.min
                    .iter()
                    .map(|v| (v * scale).clamp(-1.0, 1.0))
                    .collect(),
                self.max
                    .iter()
                    .map(|v| (v * scale).clamp(-1.0, 1.0))
                    .collect(),
                rms.iter().map(|v| (v * scale).clamp(0.0, 1.0)).collect(),
            )
        } else {
            // A silent file stays silent rather than being amplified into noise.
            (vec![0.0; buckets], vec![0.0; buckets], vec![0.0; buckets])
        };

        Waveform {
            buckets,
            duration,
            sample_rate,
            channels,
            min,
            max,
            rms,
            peak: if peak.is_finite() { peak } else { 0.0 },
        }
    }
}

/// How many buckets a file of this length is stored at.
fn cache_buckets(duration: Micros) -> usize {
    if duration <= 0 {
        return UNKNOWN_DURATION_BUCKETS;
    }
    let seconds = duration as f64 / 1_000_000.0;
    ((seconds * CACHE_BUCKETS_PER_SECOND as f64).ceil() as usize).clamp(1, MAX_CACHE_BUCKETS)
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

// ---------------------------------------------------------------------------
// The disk cache
// ---------------------------------------------------------------------------

/// Where this file's envelope lives.
///
/// The directory comes from `workspace::paths` so a "clear cache" action has
/// one place to look and the user can move the cache to another disk; the file
/// name inside it carries the fingerprint, so a file edited in place gets a new
/// name instead of a stale envelope.
fn cache_file(media_path: &Path) -> PathBuf {
    paths::waveform_dir(media_path).join(format!(
        "v{CACHE_VERSION}-{}.env",
        super::thumbnails::fingerprint(media_path)
    ))
}

/// Read a cached envelope, or `None` for anything that is not exactly one.
///
/// Every failure — missing, truncated, written by an older version, corrupted
/// by a half-finished write — answers `None` and costs one decode. A cache is
/// never a reason to fail a request.
fn read_cache(file: &Path) -> Option<Waveform> {
    let bytes = std::fs::read(file).ok()?;
    if bytes.len() < CACHE_HEADER || bytes[..4] != CACHE_MAGIC {
        return None;
    }
    let u32_at = |offset: usize| {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    if u32_at(4) != CACHE_VERSION {
        return None;
    }
    let buckets = u32_at(8) as usize;
    if buckets == 0 || buckets > MAX_CACHE_BUCKETS {
        return None;
    }
    let sample_rate = u32_at(12);
    let channels = u32_at(16) as u16;
    let duration = i64::from_le_bytes(bytes[20..28].try_into().ok()?);
    let peak = f32::from_le_bytes(bytes[28..32].try_into().ok()?);

    let payload = 3 * buckets * 4;
    if bytes.len() != CACHE_HEADER + payload {
        return None;
    }
    let floats = |start: usize| -> Vec<f32> {
        bytes[start..start + buckets * 4]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    };
    let min = floats(CACHE_HEADER);
    let max = floats(CACHE_HEADER + buckets * 4);
    let rms = floats(CACHE_HEADER + 2 * buckets * 4);
    if min.iter().chain(&max).chain(&rms).any(|v| !v.is_finite()) {
        return None;
    }

    Some(Waveform {
        buckets,
        duration,
        sample_rate,
        channels,
        min,
        max,
        rms,
        peak,
    })
}

/// Write an envelope, via a temporary file and a rename.
///
/// The rename is what keeps a run killed mid-write from leaving a half file
/// that the next run reads as a waveform — [`read_cache`] would reject it on
/// length, but only until a truncation happened to land on a bucket boundary.
fn write_cache(file: &Path, waveform: &Waveform) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut bytes = Vec::with_capacity(CACHE_HEADER + waveform.buckets * 12);
    bytes.extend_from_slice(&CACHE_MAGIC);
    bytes.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(waveform.buckets as u32).to_le_bytes());
    bytes.extend_from_slice(&waveform.sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(waveform.channels as u32).to_le_bytes());
    bytes.extend_from_slice(&waveform.duration.to_le_bytes());
    bytes.extend_from_slice(&waveform.peak.to_le_bytes());
    for values in [&waveform.min, &waveform.max, &waveform.rms] {
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }

    // The counter as well as the process id: two lanes can ask for the same
    // material at two resolutions at once, and two threads writing one staging
    // path would rename a file that is half of each into the cache.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let staging = file.with_extension(format!(
        "partial.{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&staging, &bytes)?;
    std::fs::rename(&staging, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(min: &[f32], max: &[f32], rms: &[f32]) -> Waveform {
        Waveform {
            buckets: min.len(),
            duration: 1_000_000,
            sample_rate: 48_000,
            channels: 2,
            min: min.to_vec(),
            max: max.to_vec(),
            rms: rms.to_vec(),
            peak: 1.0,
        }
    }

    // -- bucket arithmetic --------------------------------------------------

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
    fn expected_sample_count_follows_duration() {
        assert_eq!(expected_sample_count(1_000_000, 48_000), 48_000);
        assert_eq!(expected_sample_count(500_000, 44_100), 22_050);
        assert_eq!(expected_sample_count(0, 48_000), 0);
        assert_eq!(expected_sample_count(-1, 48_000), 0);
    }

    #[test]
    fn the_stored_resolution_follows_the_length_and_is_bounded() {
        assert_eq!(cache_buckets(1_000_000), CACHE_BUCKETS_PER_SECOND as usize);
        assert_eq!(cache_buckets(10_000_000), 10_000);
        // Four hours would be 14.4 million buckets without the ceiling.
        assert_eq!(cache_buckets(4 * 3_600 * 1_000_000), MAX_CACHE_BUCKETS);
        // And nothing is ever stored that no caller could ask to read.
        assert!(MAX_CACHE_BUCKETS <= MAX_REQUEST_BUCKETS);
        assert_eq!(cache_buckets(0), UNKNOWN_DURATION_BUCKETS);
        // A file shorter than one bucket still gets one.
        assert_eq!(cache_buckets(100), 1);
    }

    // -- reduction ----------------------------------------------------------

    #[test]
    fn spans_cover_every_stored_bucket_exactly_once() {
        let source = 100;
        let mut covered = 0;
        for index in 0..10 {
            let (from, to) = span(index, 10, source);
            assert_eq!(from, covered, "spans must not skip or overlap");
            covered = to;
        }
        assert_eq!(covered, source);
    }

    #[test]
    fn reduction_keeps_the_extremes_and_the_energy() {
        let stored = envelope(
            &[-0.1, -0.9, -0.2, -0.3],
            &[0.4, 0.2, 1.0, 0.1],
            &[0.5, 0.5, 0.5, 0.5],
        );
        let drawn = stored.resampled(2);
        assert_eq!(drawn.buckets, 2);
        // The transient in bucket 1 survives being merged with a quiet one.
        assert_eq!(drawn.min, vec![-0.9, -0.3]);
        assert_eq!(drawn.max, vec![0.4, 1.0]);
        // Equal RMS values combine to themselves rather than to something else.
        assert!(drawn.rms.iter().all(|v| (v - 0.5).abs() < 1e-6));
    }

    #[test]
    fn rms_combines_in_the_energy_domain() {
        // Averaging would give 0.5; the RMS of the union is √0.5 ≈ 0.707.
        let stored = envelope(&[0.0, 0.0], &[1.0, 0.0], &[1.0, 0.0]);
        let drawn = stored.resampled(1);
        assert!(
            (drawn.rms[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
            "got {}",
            drawn.rms[0]
        );
    }

    #[test]
    fn asking_for_the_stored_resolution_changes_nothing() {
        let stored = envelope(&[-0.5, -0.1], &[0.5, 0.1], &[0.3, 0.05]);
        assert_eq!(stored.resampled(2), stored);
    }

    #[test]
    fn asking_for_more_columns_than_were_stored_stretches_rather_than_blanks() {
        let stored = envelope(&[-1.0, -0.5], &[1.0, 0.5], &[0.7, 0.3]);
        let drawn = stored.resampled(8);
        assert_eq!(drawn.buckets, 8);
        assert!(
            drawn.min.iter().all(|v| *v < 0.0) && drawn.max.iter().all(|v| *v > 0.0),
            "no column may come back empty: {drawn:?}"
        );
        assert_eq!(drawn.min[0], -1.0);
        assert_eq!(drawn.min[7], -0.5);
    }

    #[test]
    fn reduction_to_one_column_covers_the_whole_file() {
        let stored = envelope(&[-0.2, -0.9, -0.1], &[0.3, 0.2, 0.8], &[0.1, 0.2, 0.3]);
        let drawn = stored.resampled(1);
        assert_eq!(drawn.min, vec![-0.9]);
        assert_eq!(drawn.max, vec![0.8]);
    }

    #[test]
    fn a_thousand_reductions_never_index_out_of_range() {
        // The span arithmetic is the one place an off-by-one is a panic in
        // front of the user rather than a wrong pixel.
        let stored = envelope(&[-1.0; 37], &[1.0; 37], &[0.5; 37]);
        for buckets in 1..=1_000 {
            assert_eq!(stored.resampled(buckets).buckets, buckets);
        }
    }

    // -- folding samples ----------------------------------------------------

    /// Fold interleaved f32 into an envelope through the real [`Envelope::push`].
    ///
    /// The samples are packed into an actual `AVFrame` rather than handed to a
    /// parallel implementation, so what these tests exercise is the code that
    /// runs in production — including the `samples()`-derived byte count that
    /// keeps uninitialised tail bytes out of the envelope.
    fn fold(buckets: usize, channels: usize, expected: u64, samples: &[f32]) -> Waveform {
        let frames = samples.len() / channels;
        let mut audio = frame::Audio::new(
            ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
            frames,
            ffmpeg::ChannelLayout::default(channels as i32),
        );
        {
            let bytes = audio.data_mut(0);
            for (index, value) in samples.iter().enumerate() {
                bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
            }
        }

        let mut envelope = Envelope::new(buckets, expected, channels as u16);
        envelope.push(&audio);
        envelope.finish(1_000_000, 48_000, channels as u16)
    }

    #[test]
    fn normalization_puts_the_loudest_sample_at_full_scale() {
        let folded = fold(2, 1, 4, &[0.25, -0.5, 0.1, 0.05]);
        assert_eq!(folded.min[0], -1.0, "the loudest sample sets the scale");
        assert_eq!(folded.max[0], 0.5);
        assert!((folded.peak - 0.5).abs() < 1e-6);
    }

    #[test]
    fn silence_normalizes_to_silence_rather_than_to_noise() {
        let folded = fold(3, 2, 6, &[0.0; 12]);
        assert!(folded.min.iter().all(|v| *v == 0.0));
        assert!(folded.max.iter().all(|v| *v == 0.0));
        assert!(folded.rms.iter().all(|v| *v == 0.0));
        assert_eq!(folded.peak, 0.0);
    }

    #[test]
    fn rms_sits_below_the_peak_and_apart_from_it_for_a_sine() {
        // A full-scale sine's RMS is 1/√2 of its peak. That gap is the entire
        // reason RMS is stored: a peak-only lane draws both as the same block.
        let frames = 4_800;
        let samples: Vec<f32> = (0..frames)
            .map(|n| (n as f32 * std::f32::consts::TAU / 48.0).sin())
            .collect();
        let folded = fold(1, 1, frames as u64, &samples);
        assert!((folded.max[0] - 1.0).abs() < 1e-3);
        assert!(
            (folded.rms[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-2,
            "rms {} should be about 0.707 of the peak",
            folded.rms[0]
        );
    }

    #[test]
    fn a_hard_panned_sound_is_not_halved() {
        // Left at full scale, right silent. A downmix would draw this at half
        // the height of the same sound in the centre.
        let folded = fold(1, 2, 2, &[1.0, 0.0, -1.0, 0.0]);
        assert_eq!(folded.max[0], 1.0);
        assert_eq!(folded.min[0], -1.0);
        assert!((folded.rms[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn non_finite_samples_do_not_blank_the_file() {
        let folded = fold(1, 2, 2, &[f32::NAN, 0.3, f32::INFINITY, 0.6]);
        assert!((folded.max[0] - 1.0).abs() < 1e-6, "0.6 becomes full scale");
        assert!(folded.peak.is_finite());
    }

    #[test]
    fn the_envelope_brackets_zero_even_for_a_one_sided_signal() {
        // Every sample positive — a DC offset, or a bucket that happens to cover
        // only the rising half of a wave. The column must still be anchored on
        // the baseline, because a lane of columns floating off the centre line
        // reads as the editor having damaged the audio.
        let folded = fold(1, 1, 3, &[0.5, 0.7, 0.6]);
        assert_eq!(folded.min[0], 0.0, "the column starts at the baseline");
        assert_eq!(folded.max[0], 1.0, "and reaches the loudest sample");

        // And the same on the other side.
        let folded = fold(1, 1, 3, &[-0.5, -0.7, -0.6]);
        assert_eq!(folded.max[0], 0.0);
        assert_eq!(folded.min[0], -1.0);
    }

    // -- the cache file -----------------------------------------------------

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chukcut-waveform-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn a_cached_envelope_round_trips_byte_for_byte() {
        let dir = scratch("roundtrip");
        let file = dir.join("wave.env");
        let stored = envelope(&[-0.25, -1.0], &[0.5, 1.0], &[0.1, 0.9]);
        write_cache(&file, &stored).expect("write the cache");
        assert_eq!(read_cache(&file), Some(stored));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_or_foreign_cache_file_is_ignored_rather_than_trusted() {
        let dir = scratch("corrupt");
        let stored = envelope(&[-1.0, -1.0], &[1.0, 1.0], &[0.5, 0.5]);

        let good = dir.join("good.env");
        write_cache(&good, &stored).expect("write the cache");
        let bytes = std::fs::read(&good).expect("read it back");

        let truncated = dir.join("truncated.env");
        std::fs::write(&truncated, &bytes[..bytes.len() - 4]).unwrap();
        assert_eq!(
            read_cache(&truncated),
            None,
            "a short file is not a waveform"
        );

        let foreign = dir.join("foreign.env");
        let mut wrong = bytes.clone();
        wrong[0] = b'X';
        std::fs::write(&foreign, &wrong).unwrap();
        assert_eq!(
            read_cache(&foreign),
            None,
            "another format is not a waveform"
        );

        let older = dir.join("older.env");
        let mut stale = bytes.clone();
        stale[4..8].copy_from_slice(&(CACHE_VERSION + 1).to_le_bytes());
        std::fs::write(&older, &stale).unwrap();
        assert_eq!(read_cache(&older), None, "another version is not migrated");

        assert_eq!(read_cache(&dir.join("absent.env")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cache_path_changes_when_the_file_does() {
        let dir = scratch("invalidation");
        let media = dir.join("clip.wav");
        std::fs::write(&media, b"first").unwrap();
        let before = cache_file(&media);

        // A rewrite of the same path with different contents must not be
        // answered from the old envelope.
        std::fs::write(&media, b"second and longer").unwrap();
        assert_ne!(before, cache_file(&media));

        // And the directory is under the workspace cache root, not temp.
        assert!(before.starts_with(paths::cache_root()), "{before:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_impossible_bucket_count_is_refused_with_a_sentence() {
        let error = waveform("/nonexistent.wav", 0).unwrap_err().to_string();
        assert!(error.contains("at least one bucket"), "{error}");

        let error = waveform("/nonexistent.wav", MAX_REQUEST_BUCKETS + 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("timeline can draw"), "{error}");
    }
}
