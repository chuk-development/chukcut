//! Streaming PCM out of a media file.
//!
//! The same argument `media::VideoDecoder` makes in its header applies here
//! and is if anything sharper: opening a file, finding stream info and
//! initialising a codec costs tens of milliseconds, and preview audio needs a
//! new block every few *milliseconds*. A decoder opened per request would miss
//! every deadline it was given. So a [`AudioClipReader`] owns its demuxer,
//! decoder and resampler across calls, answers a sequential read by decoding
//! forward, and seeks only when the request moves backwards or jumps far
//! ahead — exactly the policy the video decoder uses, for exactly the same
//! reason.
//!
//! ## Everything leaves at the device's format
//!
//! swresample converts sample format, channel layout *and* rate in one pass on
//! the way out, so the rest of the module never sees a planar 24-bit 44.1 kHz
//! 5.1 stream — it sees interleaved stereo `f32` at the device rate. Doing the
//! rate conversion here rather than in the mixer also means the only
//! resampling the mixer does is the one that carries meaning: `speed`.
//!
//! ## Positions are frames, not timestamps
//!
//! The caller asks for "`n` sample frames starting at frame `f`", where `f` is
//! counted from the start of the file at the *output* rate. A reader that took
//! microseconds would have to round twice — once here and once in the mixer —
//! and two roundings that disagree are a sample of drift per block.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use ffmpeg::software::resampling;
use ffmpeg::util::frame;
use ffmpeg_next as ffmpeg;

use super::{AudioError, Result};
use crate::modules::media::{ensure_initialized, ts_to_micros};
use crate::modules::project::document::{Micros, MICROS_PER_SECOND};

/// How far ahead of the current position a request may be before we seek
/// rather than decode through the gap. Audio frames are small and cheap, so
/// this is generous compared with the video decoder's window; a seek costs a
/// codec flush and a keyframe, decoding a second of audio costs almost nothing.
const FORWARD_DECODE_WINDOW: Micros = 2 * MICROS_PER_SECOND;

/// Where sample frames come from, at the device's rate and channel count.
///
/// The trait exists so the mixer can be tested against a generator instead of
/// a file: every interesting question about mixing — boundaries, gain, speed,
/// clipping — is a question about arithmetic, and none of it should need
/// FFmpeg or a fixture on disk to answer.
pub trait ClipReader: Send {
    /// Fill `out` with `frames` interleaved sample frames starting at
    /// `at_frame`, counted from the start of the file.
    ///
    /// Implementations must write exactly `frames * channels` values and
    /// zero-pad past the end of the material: a short answer would shift
    /// everything after it, silently and audibly.
    fn read(&mut self, at_frame: i64, frames: usize, out: &mut [f32]) -> Result<()>;
}

/// Opens readers. Injected into the mixer so tests can supply their own.
pub trait ClipFactory: Send + Sync + std::fmt::Debug {
    fn open(&self, path: &str, rate: u32, channels: u16) -> Result<Box<dyn ClipReader>>;
}

/// The real one.
#[derive(Debug, Default, Clone, Copy)]
pub struct FileClipFactory;

impl ClipFactory for FileClipFactory {
    fn open(&self, path: &str, rate: u32, channels: u16) -> Result<Box<dyn ClipReader>> {
        Ok(Box::new(AudioClipReader::open(path, rate, channels)?))
    }
}

/// A long-lived decoder for one file.
pub struct AudioClipReader {
    path: PathBuf,
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Audio,
    resampler: resampling::Context,
    stream_index: usize,
    time_base: ffmpeg::Rational,
    /// First timestamp of the stream, so "frame zero" means the start of the
    /// file rather than whatever the container felt like numbering it.
    start_ts: i64,

    rate: u32,
    channels: usize,
    /// The channel-layout mask the resampler was configured with. Kept so a
    /// decoded frame that does not name its layout can be given this one; see
    /// [`AudioClipReader::name_the_layout`].
    in_layout_mask: u64,

    /// Decoded, converted samples not yet handed out.
    buffer: VecDeque<f32>,
    /// Output frame index of the first sample frame in `buffer`. `None` until
    /// the first frame after an open or a seek establishes it from a pts.
    buffer_start: Option<i64>,
    /// The demuxer has run out and the decoder has been told so.
    draining: bool,
    /// Nothing more will ever come out.
    finished: bool,
}

/// # Safety
///
/// The same reasoning as `media::VideoDecoder`: the FFmpeg contexts held here
/// carry no thread affinity, every method takes `&mut self`, and the reader is
/// only ever used from the thread that owns it. `resampling::Context` holds a
/// raw pointer, which is the only reason this is not inferred.
unsafe impl Send for AudioClipReader {}

impl AudioClipReader {
    /// Open `path` and deliver `channels`-channel interleaved `f32` at `rate`.
    pub fn open(path: impl AsRef<Path>, rate: u32, channels: u16) -> Result<Self> {
        let path = path.as_ref();
        let rate = rate.max(1);
        let channels = channels.max(1);
        ensure_initialized();

        let input = ffmpeg::format::input(&path).map_err(|source| AudioError::Open {
            path: path.to_path_buf(),
            source,
        })?;

        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or_else(|| AudioError::NoAudioStream(path.to_path_buf()))?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let start_ts = normalize_start(stream.start_time());
        let parameters = stream.parameters();

        let context =
            ffmpeg::codec::context::Context::from_parameters(parameters).map_err(|source| {
                AudioError::Decode {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
        let mut decoder = context
            .decoder()
            .audio()
            .map_err(|source| AudioError::Decode {
                path: path.to_path_buf(),
                source,
            })?;

        // Some containers leave the layout unset even though the channel count
        // is known, and swresample refuses to initialise without one.
        let mut layout = decoder.channel_layout();
        if layout.bits() == 0 {
            layout = ffmpeg::ChannelLayout::default(decoder.channels().max(1) as i32);
            decoder.set_channel_layout(layout);
        }

        let out_layout = ffmpeg::ChannelLayout::default(channels as i32);
        let resampler = resampling::Context::get(
            decoder.format(),
            layout,
            decoder.rate().max(1),
            ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
            out_layout,
            rate,
        )
        .map_err(|source| AudioError::Decode {
            path: path.to_path_buf(),
            source,
        })?;

        Ok(Self {
            path: path.to_path_buf(),
            input,
            decoder,
            resampler,
            stream_index,
            time_base,
            start_ts,
            rate,
            channels: channels as usize,
            in_layout_mask: layout.bits(),
            buffer: VecDeque::new(),
            buffer_start: None,
            draining: false,
            finished: false,
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Output frame index one past the last sample in the buffer.
    fn buffer_end(&self) -> Option<i64> {
        self.buffer_start
            .map(|start| start + (self.buffer.len() / self.channels) as i64)
    }

    fn needs_seek(&self, at_frame: i64) -> bool {
        match (self.buffer_start, self.buffer_end()) {
            (Some(start), Some(end)) => {
                let window = super::clock::micros_to_frames(FORWARD_DECODE_WINDOW, self.rate);
                at_frame < start || at_frame > end + window
            }
            // Nothing decoded yet: only a request near the start avoids a seek.
            _ => at_frame > super::clock::micros_to_frames(FORWARD_DECODE_WINDOW, self.rate),
        }
    }

    fn seek(&mut self, at_frame: i64) -> Result<()> {
        let target = super::clock::frames_to_micros(at_frame.max(0) as u64, self.rate);
        // `Input::seek` passes -1 as the stream index, which makes the
        // timestamp AV_TIME_BASE units — microseconds. The bounded range keeps
        // the search from landing after the instant we asked for, which no
        // amount of decoding forward could recover from.
        let seek_ts = target + ts_to_micros(self.start_ts, self.time_base);
        self.input
            .seek(seek_ts, ..seek_ts)
            .map_err(|source| AudioError::Decode {
                path: self.path.clone(),
                source,
            })?;

        self.decoder.flush();
        self.buffer.clear();
        self.buffer_start = None;
        self.draining = false;
        self.finished = false;
        Ok(())
    }

    /// Decode until the buffer reaches `end_frame` or the file runs out.
    fn fill_to(&mut self, end_frame: i64) -> Result<()> {
        while !self.finished {
            if let Some(end) = self.buffer_end() {
                if end >= end_frame {
                    return Ok(());
                }
            }
            if !self.decode_step()? {
                self.finished = true;
            }
        }
        Ok(())
    }

    /// Pull one decoded frame through the resampler and append it. Returns
    /// false at end of stream.
    fn decode_step(&mut self) -> Result<bool> {
        loop {
            let mut decoded = frame::Audio::empty();
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    self.name_the_layout(&mut decoded);
                    self.append(&decoded)?;
                    return Ok(true);
                }
                // The decoder wants more input before it can produce anything.
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {}
                Err(ffmpeg::Error::Eof) => return Ok(false),
                Err(source) => {
                    return Err(AudioError::Decode {
                        path: self.path.clone(),
                        source,
                    })
                }
            }

            if self.draining {
                return Ok(false);
            }
            if !self.feed_packet()? {
                // Codecs with a decode delay hold the tail of the file until
                // told the stream ended; without this the last block of every
                // clip is silence.
                self.decoder
                    .send_eof()
                    .map_err(|source| AudioError::Decode {
                        path: self.path.clone(),
                        source,
                    })?;
                self.draining = true;
            }
        }
    }

    /// Hand the decoder the next packet of our stream. False at end of file.
    fn feed_packet(&mut self) -> Result<bool> {
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => return Ok(false),
                Err(source) => {
                    return Err(AudioError::Decode {
                        path: self.path.clone(),
                        source,
                    })
                }
            }
            if packet.stream() != self.stream_index {
                continue;
            }
            self.decoder
                .send_packet(&packet)
                .map_err(|source| AudioError::Decode {
                    path: self.path.clone(),
                    source,
                })?;
            return Ok(true);
        }
    }

    /// Give a decoded frame the channel layout the resampler expects, when the
    /// file did not name one.
    ///
    /// FFmpeg 6 replaced the channel-layout bitmask with `AVChannelLayout`,
    /// which distinguishes "two channels, front left and front right" from
    /// "two channels, we were not told which". A plain WAV file — no channel
    /// mask in its `fmt ` chunk — decodes to the second kind, and swresample
    /// compares the frame's layout against the one it was configured with and
    /// rejects anything that does not match, with `Input changed`. Since
    /// `ffmpeg-next` 6.1 still configures the resampler through the *legacy*
    /// mask API, that mismatch is guaranteed for those files rather than
    /// unlucky: every one of them fails, and the failure looks like a codec
    /// problem rather than a bookkeeping one.
    ///
    /// Naming the layout is the fix, and it is a relabelling rather than a
    /// conversion: the samples are already interleaved in the order the mask
    /// describes.
    fn name_the_layout(&self, decoded: &mut frame::Audio) {
        // Safety: `decoded` is a frame this decoder just produced, and an
        // unspecified layout owns no allocation, so overwriting it leaks
        // nothing.
        unsafe {
            let ptr = decoded.as_mut_ptr();
            if (*ptr).ch_layout.order == ffmpeg::ffi::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC {
                ffmpeg::ffi::av_channel_layout_from_mask(
                    &mut (*ptr).ch_layout,
                    self.in_layout_mask,
                );
            }
        }
    }

    /// Convert one decoded frame and append it to the buffer.
    fn append(&mut self, decoded: &frame::Audio) -> Result<()> {
        // Position is taken from a timestamp only when there is nothing to
        // append to. Trusting every frame's pts instead would let a container
        // with jittery timestamps insert or drop a sample per frame; trusting
        // it once and then counting samples cannot drift.
        if self.buffer_start.is_none() {
            let micros = self.frame_micros(decoded);
            self.buffer_start = Some(super::clock::micros_to_frames(micros, self.rate));
        }

        let in_samples = decoded.samples();
        if in_samples == 0 {
            return Ok(());
        }
        // Room for the rate change plus a margin: swresample buffers whatever
        // does not fit and hands it over on the next call, which would show up
        // as a slowly growing delay rather than an error.
        let capacity =
            (in_samples as u64 * self.rate as u64 / decoded.rate().max(1) as u64) as usize + 64;
        let mut converted = frame::Audio::new(
            ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
            capacity,
            ffmpeg::ChannelLayout::default(self.channels as i32),
        );
        self.resampler
            .run(decoded, &mut converted)
            .map_err(|source| AudioError::Decode {
                path: self.path.clone(),
                source,
            })?;
        self.extend_from(&converted);
        Ok(())
    }

    /// Append the packed `f32` payload of a converted frame.
    ///
    /// The byte count comes from `samples()` rather than the length of
    /// `data(0)`: that slice is the whole allocated line, and the resampler is
    /// allowed to write fewer samples than were allocated, so reading all of
    /// it would append uninitialised tail bytes as audio.
    fn extend_from(&mut self, converted: &frame::Audio) {
        const BYTES: usize = std::mem::size_of::<f32>();
        let bytes = converted.data(0);
        let wanted = converted.samples() * self.channels * BYTES;
        let usable = wanted.min(bytes.len() - bytes.len() % BYTES);
        for value in bytes[..usable].as_chunks::<BYTES>().0 {
            // Packed f32 is native-endian, so this is a reinterpretation
            // rather than a byte-order conversion.
            let sample = f32::from_ne_bytes([value[0], value[1], value[2], value[3]]);
            self.buffer
                .push_back(if sample.is_finite() { sample } else { 0.0 });
        }
    }

    fn frame_micros(&self, decoded: &frame::Audio) -> Micros {
        let ts = decoded
            .timestamp()
            .or_else(|| decoded.pts())
            .unwrap_or(self.start_ts);
        ts_to_micros(ts - self.start_ts, self.time_base).max(0)
    }

    /// Drop samples ahead of `at_frame` so the buffer front is the request.
    fn trim_to(&mut self, at_frame: i64) {
        let Some(start) = self.buffer_start else {
            return;
        };
        if at_frame <= start {
            return;
        }
        let drop_frames = ((at_frame - start) as usize).min(self.buffer.len() / self.channels);
        self.buffer.drain(..drop_frames * self.channels);
        self.buffer_start = Some(start + drop_frames as i64);
    }
}

impl AudioClipReader {
    /// Output frame one past the last sample the file holds, once decoding
    /// has run into its end; `None` before.
    pub fn ended_at(&self) -> Option<i64> {
        if self.finished {
            self.buffer_end()
        } else {
            None
        }
    }
}

impl ClipReader for AudioClipReader {
    fn read(&mut self, at_frame: i64, frames: usize, out: &mut [f32]) -> Result<()> {
        let wanted = frames * self.channels;
        debug_assert_eq!(out.len(), wanted);
        for sample in out.iter_mut() {
            *sample = 0.0;
        }
        if frames == 0 {
            return Ok(());
        }

        let at_frame = at_frame.max(0);
        if self.needs_seek(at_frame) {
            self.seek(at_frame)?;
        }

        self.fill_to(at_frame + frames as i64)?;

        // A seek lands on a packet boundary at or before the request, so the
        // buffer usually starts a little early; drop that head rather than
        // playing it, which would put the clip out of sync by up to a packet.
        self.trim_to(at_frame);

        let Some(start) = self.buffer_start else {
            // Nothing decoded at all — past the end of the file. Silence is
            // the right answer and not an error: a segment can legitimately
            // outlast its material after a speed change.
            return Ok(());
        };
        // A seek that undershot leaves the buffer starting *after* the
        // request only if the file itself starts later; offset into `out`
        // rather than shifting the audio earlier.
        let skip = (start - at_frame).max(0) as usize;
        if skip >= frames {
            return Ok(());
        }
        let available = self.buffer.len().min((frames - skip) * self.channels);
        for (index, sample) in self.buffer.iter().take(available).enumerate() {
            out[skip * self.channels + index] = *sample;
        }
        Ok(())
    }
}

fn normalize_start(start_time: i64) -> i64 {
    // AV_NOPTS_VALUE is i64::MIN; a negative start is meaningless here either
    // way.
    if start_time == i64::MIN || start_time < 0 {
        0
    } else {
        start_time
    }
}

// ---------------------------------------------------------------------------
// The export mixer's interface, implemented
// ---------------------------------------------------------------------------

/// [`export::AudioSource`] backed by a real decoder.
///
/// The offline mixer was written against a trait with `SilentAudioSource` as
/// the only implementation, which is why exports currently carry a silent
/// audio stream. Nothing about that interface is preview-specific, so it is
/// answered here rather than duplicated: one decode per segment per export is
/// the shape it already asks for, and opening a reader per call is the right
/// trade when the call covers a whole clip.
///
/// [`export::AudioSource`]: crate::modules::export::AudioSource
#[derive(Debug, Default, Clone, Copy)]
pub struct FileAudioSource;

impl crate::modules::export::AudioSource for FileAudioSource {
    fn samples(
        &self,
        request: &crate::modules::export::AudioRequest<'_>,
    ) -> anyhow::Result<Vec<f32>> {
        let channels = request.channels.max(1);
        let frames = request.frames();
        let mut out = vec![0.0; frames * channels as usize];
        if frames == 0 {
            return Ok(out);
        }
        let mut reader = AudioClipReader::open(request.path, request.sample_rate, channels)?;
        let at = super::clock::micros_to_frames(request.start, request.sample_rate);
        reader.read(at, frames, &mut out)?;
        Ok(out)
    }

    fn stream(
        &self,
        request: &crate::modules::export::AudioRequest<'_>,
    ) -> anyhow::Result<Box<dyn crate::modules::export::AudioStream + '_>> {
        let channels = request.channels.max(1);
        let reader = AudioClipReader::open(request.path, request.sample_rate, channels)?;
        Ok(Box::new(FileStream {
            reader,
            base: super::clock::micros_to_frames(request.start, request.sample_rate),
            frames: request.frames(),
            channels: channels as usize,
            next: 0,
        }))
    }

    fn probe(&self, path: &str) -> anyhow::Result<()> {
        AudioClipReader::open(path, 48_000, 2)?;
        Ok(())
    }
}

/// How far ahead of the last read a [`FileStream`] decodes through rather
/// than seeking, in output frames at 48 kHz (30 s). A read that skips ahead
/// only when a range export starts inside a clip; decoding through keeps the
/// samples identical to a read of the whole clip, and past this a seek is
/// cheaper than the decode.
const DECODE_THROUGH_FRAMES: usize = 30 * 48_000;

/// [`FileAudioSource`]'s stream: one reader, decoding forward.
///
/// The reader is positioned the way the one-shot `samples` positions it — at
/// the request's start — so reading the request in pieces gives the samples
/// one read of the whole request gives: same seek, same decoder state, same
/// resampler phase. Only a read that jumps far ahead seeks again.
struct FileStream {
    reader: AudioClipReader,
    /// Output frame of the request's start in the file.
    base: i64,
    /// Frames the request covers; zero past it.
    frames: usize,
    channels: usize,
    /// Offset the reader is positioned at: one past the last frame read.
    next: usize,
}

impl crate::modules::export::AudioStream for FileStream {
    fn read(&mut self, offset: usize, out: &mut [f32]) -> anyhow::Result<()> {
        out.fill(0.0);
        let wanted = (out.len() / self.channels).min(self.frames.saturating_sub(offset));
        if wanted == 0 {
            return Ok(());
        }
        if offset > self.next && offset - self.next <= DECODE_THROUGH_FRAMES {
            let mut skip = vec![0.0f32; 4_096 * self.channels];
            while self.next < offset {
                let n = (offset - self.next).min(4_096);
                self.decode(self.next, &mut skip[..n * self.channels])?;
                self.next += n;
            }
        }
        self.decode(offset, &mut out[..wanted * self.channels])?;
        self.next = offset + wanted;
        Ok(())
    }
}

impl FileStream {
    fn decode(&mut self, offset: usize, out: &mut [f32]) -> anyhow::Result<()> {
        let at = self.base + offset as i64;
        // Past the end of the file there is only silence; asking the reader
        // would make it seek there once per block.
        if self.reader.ended_at().is_some_and(|end| at >= end) {
            out.fill(0.0);
            return Ok(());
        }
        self.reader.read(at, out.len() / self.channels, out)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample frames in the fixture, and the rate it is written at. Small
    /// enough to build in memory, long enough to seek around inside.
    const FIXTURE_FRAMES: usize = 4_000;
    const FIXTURE_RATE: u32 = 8_000;
    /// Each sample encodes its own frame index times this, so a decoded value
    /// says which part of the file it came from.
    const FIXTURE_STEP: i16 = 8;

    /// A real file, decoded by the real FFmpeg, without a fixture in the repo
    /// or a shell out to the `ffmpeg` binary.
    ///
    /// Uncompressed PCM in a WAV container is the one format simple enough to
    /// write by hand, and it exercises everything that matters here: the
    /// demuxer, the decoder, swresample's format/rate/layout conversion, the
    /// timestamp that positions the first block, and the seek path.
    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new(name: &str, channels: u16) -> Self {
            let path = std::env::temp_dir().join(format!(
                "chukcut-audio-{name}-{}-{channels}.wav",
                std::process::id()
            ));
            let channels = channels.max(1);
            let data_len = FIXTURE_FRAMES * channels as usize * 2;

            let mut bytes = Vec::with_capacity(44 + data_len);
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&16u32.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
            bytes.extend_from_slice(&channels.to_le_bytes());
            bytes.extend_from_slice(&FIXTURE_RATE.to_le_bytes());
            bytes.extend_from_slice(&(FIXTURE_RATE * channels as u32 * 2).to_le_bytes());
            bytes.extend_from_slice(&(channels * 2).to_le_bytes());
            bytes.extend_from_slice(&16u16.to_le_bytes());
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
            for frame in 0..FIXTURE_FRAMES {
                let value = frame as i16 * FIXTURE_STEP;
                for _ in 0..channels {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }

            std::fs::write(&path, bytes).expect("write the audio fixture");
            Self { path }
        }

        fn open(&self, rate: u32) -> AudioClipReader {
            AudioClipReader::open(&self.path, rate, 2).expect("open the audio fixture")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Which source frame a decoded sample came from.
    fn decoded_frame(sample: f32) -> f32 {
        sample * 32_768.0 / FIXTURE_STEP as f32
    }

    fn read(reader: &mut AudioClipReader, at: i64, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        reader.read(at, frames, &mut out).expect("read the fixture");
        out
    }

    #[test]
    fn a_sequential_read_returns_the_samples_that_are_there() {
        let fixture = Fixture::new("sequential", 2);
        let mut reader = fixture.open(FIXTURE_RATE);

        let out = read(&mut reader, 0, 8);
        for frame in 0..8 {
            let found = decoded_frame(out[frame * 2]);
            assert!(
                (found - frame as f32).abs() < 0.5,
                "frame {frame} decoded as {found}"
            );
        }

        // And carrying on from where it left off does not need a seek.
        let out = read(&mut reader, 8, 8);
        assert!((decoded_frame(out[0]) - 8.0).abs() < 0.5);
    }

    #[test]
    fn a_seek_lands_on_the_requested_sample() {
        let fixture = Fixture::new("seek", 2);
        let mut reader = fixture.open(FIXTURE_RATE);

        // Forward, far enough to be a real seek rather than a decode through.
        let out = read(&mut reader, 3_000, 4);
        assert!(
            (decoded_frame(out[0]) - 3_000.0).abs() < 2.0,
            "landed on {}",
            decoded_frame(out[0])
        );

        // And backwards, which can only be a seek.
        let out = read(&mut reader, 500, 4);
        assert!(
            (decoded_frame(out[0]) - 500.0).abs() < 2.0,
            "landed on {}",
            decoded_frame(out[0])
        );
    }

    #[test]
    fn a_device_at_another_rate_gets_the_same_audio_resampled() {
        let fixture = Fixture::new("resample", 2);
        // Twice the file's rate: output frame 2n is source frame n.
        let mut reader = fixture.open(FIXTURE_RATE * 2);

        let out = read(&mut reader, 2_000, 4);
        let found = decoded_frame(out[0]);
        // swresample's filter has a delay of a few dozen samples, which is
        // well under a millisecond and is not corrected for here.
        assert!(
            (found - 1_000.0).abs() < 64.0,
            "output frame 2000 at 2x should be source frame 1000, got {found}"
        );
    }

    #[test]
    fn a_mono_file_arrives_on_both_channels() {
        let fixture = Fixture::new("mono", 1);
        let mut reader = fixture.open(FIXTURE_RATE);

        let out = read(&mut reader, 100, 4);
        // A centre channel spread across a stereo pair is mixed at -3 dB, so
        // the value carries where it came from scaled by 1/√2. That is
        // FFmpeg's `center_mix_level` and it is the energy-preserving answer:
        // duplicating at unity would make every mono clip 3 dB louder than the
        // stereo one next to it.
        let expected = 100.0 * std::f32::consts::FRAC_1_SQRT_2;
        let found = decoded_frame(out[0]);
        assert!((found - expected).abs() < 1.0, "{found} != {expected}");
        assert!(
            (out[0] - out[1]).abs() < 1e-6,
            "a mono source landing on one side would play out of one speaker"
        );
    }

    #[test]
    fn reading_past_the_end_of_the_file_is_silence_and_not_an_error() {
        let fixture = Fixture::new("eof", 2);
        let mut reader = fixture.open(FIXTURE_RATE);

        let frames = 64;
        let out = read(&mut reader, FIXTURE_FRAMES as i64 - 16, frames);
        // The last real sample frame, then padding.
        assert!(decoded_frame(out[0]) > 3_900.0);
        assert_eq!(
            out[(frames - 1) * 2],
            0.0,
            "past the end of the material is silence"
        );

        // Entirely past the end is silence too, and still not an error.
        let out = read(&mut reader, FIXTURE_FRAMES as i64 * 4, 16);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_missing_file_is_an_error_with_the_path_in_it() {
        let message = match AudioClipReader::open("/nonexistent/definitely-not-here.wav", 48_000, 2)
        {
            Ok(_) => panic!("a file that is not there cannot be opened"),
            Err(error) => error.to_string(),
        };
        assert!(message.contains("definitely-not-here.wav"), "{message}");
    }

    #[test]
    fn a_stream_start_that_is_unset_reads_as_zero() {
        assert_eq!(normalize_start(i64::MIN), 0);
        assert_eq!(normalize_start(-5), 0);
        assert_eq!(normalize_start(1_000), 1_000);
    }
}
