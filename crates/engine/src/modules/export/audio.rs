//! Mixing the timeline down to one stereo bed.
//!
//! Video is a stack — the topmost opaque pixel wins. Audio is a *sum*: every
//! unmuted segment that is live at an instant contributes to it. The sum is
//! taken block by block over the range being exported ([`MixStream`]), so an
//! export holds one block of the mix and one window of each clip under it,
//! whatever the timeline's length. It used to be one buffer the length of the
//! whole project — 1.4 GB an hour at 48 kHz stereo — which a two-second range
//! export of a three-hour recording paid in full (6.6 GB) and a clip parked at
//! an absurd time turned into an allocation abort.
//!
//! ## Where the samples come from
//!
//! The `media` module does not expose audio decoding yet — it has a decoder for
//! video frames and a peak reader for waveforms, and neither answers "give me
//! the PCM between these two instants". [`AudioSource`] is the interface this
//! module needs it to grow: one call, one segment's source range, already at
//! the output rate and channel count. Everything below is written against that
//! trait, so wiring it up is implementing one method. Until then
//! [`SilentAudioSource`] stands in and exports carry a valid, silent audio
//! stream rather than failing.
//!
//! ## Gain, speed and clipping
//!
//! Three multipliers reach a sample: the segment's `volume`, its `Volume`
//! keyframe track if it has one, and the track's `volume`. Speed is not a gain
//! but it changes which samples are read. A clip at a constant speed with
//! "Change audio pitch" on is read faster and resampled here (the pitch moves,
//! as a tape's would); every other speed change, a speed curve, and a clip
//! with audio effects is rendered by `modules::audiofx` — the same function
//! the preview's cached render comes from — and mixed at speed 1.
//!
//! The sum is kept in `f32` with no headroom management and clamped once, at
//! the end. Normalizing instead would mean two exports of nearly identical
//! projects come out at different loudnesses, which is worse than the clipping
//! it avoids; every editor clamps and shows the user a meter.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::modules::project::document::{
    AnimatableProperty, KeyframeTrack, Micros, Project, Segment, TrackKind, MICROS_PER_SECOND,
};

use super::{ExportError, Result};

/// One decode request: a slice of one material, at the output format.
#[derive(Debug, Clone, Copy)]
pub struct AudioRequest<'a> {
    pub material_id: &'a str,
    /// Absolute path of the media file.
    pub path: &'a str,
    /// Where in the *source* to start, in microseconds from the file start.
    pub start: Micros,
    /// How much of the source to read. With a `speed` factor this is longer
    /// than the segment occupies on the timeline; the caller resamples.
    pub duration: Micros,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioRequest<'_> {
    /// How many sample frames the answer should contain.
    pub fn frames(&self) -> usize {
        frames_for(self.duration, self.sample_rate)
    }
}

/// Where PCM comes from.
///
/// Implementations must return **exactly** `request.frames()` sample frames of
/// interleaved `f32` in `-1.0..=1.0`, zero-padded when the request runs past
/// the end of the file. Returning a short buffer is not an error the mixer can
/// recover from — it would silently shift everything after it — so the mixer
/// pads and logs instead of trusting the length.
pub trait AudioSource: Send + Sync {
    fn samples(&self, request: &AudioRequest<'_>) -> anyhow::Result<Vec<f32>>;

    /// The same samples as [`Self::samples`], handed out in pieces, so a long
    /// clip is never in memory whole. The default asks `samples` once and
    /// serves slices of the answer, which is right for generators and tests;
    /// a file reader overrides it to decode forward as it is read.
    fn stream(&self, request: &AudioRequest<'_>) -> anyhow::Result<Box<dyn AudioStream + '_>> {
        let channels = request.channels.max(1) as usize;
        let samples = self.samples(request)?;
        Ok(Box::new(BufferedStream { samples, channels }))
    }

    /// Fail now when `path` will not decode, before an export encodes a
    /// frame. The default trusts every path.
    fn probe(&self, _path: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// One request's samples, read in pieces.
pub trait AudioStream: Send {
    /// Fill `out` with the sample frames that start `offset` frames into the
    /// request, zero past its end. Callers read forward; a backward offset is
    /// allowed and may cost a seek.
    fn read(&mut self, offset: usize, out: &mut [f32]) -> anyhow::Result<()>;
}

/// [`AudioSource::stream`]'s default: a whole answer, served in slices.
struct BufferedStream {
    samples: Vec<f32>,
    channels: usize,
}

impl AudioStream for BufferedStream {
    fn read(&mut self, offset: usize, out: &mut [f32]) -> anyhow::Result<()> {
        let from = offset.saturating_mul(self.channels).min(self.samples.len());
        let n = out.len().min(self.samples.len() - from);
        out[..n].copy_from_slice(&self.samples[from..from + n]);
        out[n..].fill(0.0);
        Ok(())
    }
}

/// Silence for everything. The default until `media` grows a PCM reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct SilentAudioSource;

impl AudioSource for SilentAudioSource {
    fn samples(&self, request: &AudioRequest<'_>) -> anyhow::Result<Vec<f32>> {
        Ok(vec![
            0.0;
            request.frames() * request.channels.max(1) as usize
        ])
    }
}

// ---------------------------------------------------------------------------
// The mixer
// ---------------------------------------------------------------------------

/// A fixed-length interleaved buffer that segments are summed into.
#[derive(Debug, Clone)]
pub struct AudioMixer {
    sample_rate: u32,
    channels: usize,
    buffer: Vec<f32>,
}

impl AudioMixer {
    pub fn new(sample_rate: u32, channels: u16, duration: Micros) -> Self {
        let channels = channels.max(1) as usize;
        let frames = frames_for(duration, sample_rate);
        Self {
            sample_rate,
            channels,
            buffer: vec![0.0; frames * channels],
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Sample frames in the mix.
    pub fn frames(&self) -> usize {
        self.buffer.len() / self.channels
    }

    /// Add `samples` at `at`, scaled by a gain that may vary per sample frame.
    ///
    /// Anything past the end of the mix is dropped rather than growing the
    /// buffer: a segment can legitimately extend past the project duration
    /// after a speed change, and the video stops at the duration either way.
    pub fn mix_at(&mut self, samples: &[f32], at: Micros, mut gain: impl FnMut(usize) -> f32) {
        let start = frames_for(at.max(0), self.sample_rate);
        let total = self.frames();
        if start >= total {
            return;
        }
        let available = samples.len() / self.channels;
        let count = available.min(total - start);
        for frame in 0..count {
            let g = gain(frame);
            for channel in 0..self.channels {
                let src = frame * self.channels + channel;
                let dst = (start + frame) * self.channels + channel;
                self.buffer[dst] += samples[src] * g;
            }
        }
    }

    /// Add at a constant gain.
    pub fn mix_at_gain(&mut self, samples: &[f32], at: Micros, gain: f32) {
        self.mix_at(samples, at, |_| gain);
    }

    /// The mix, clamped to full scale.
    ///
    /// Summing several segments can exceed 1.0 and every sample format below
    /// float wraps rather than saturates, which turns a loud passage into
    /// static. Clamping trades that for the much gentler distortion of a
    /// flattened peak.
    pub fn finish(self) -> Vec<f32> {
        self.buffer
            .into_iter()
            .map(|s| {
                if s.is_finite() {
                    s.clamp(-1.0, 1.0)
                } else {
                    // A NaN from a broken float source would otherwise poison
                    // every downstream conversion.
                    0.0
                }
            })
            .collect()
    }

    /// The mix as summed, only broken floats zeroed.
    pub fn finish_unclamped(self) -> Vec<f32> {
        self.buffer
            .into_iter()
            .map(|s| if s.is_finite() { s } else { 0.0 })
            .collect()
    }

    /// The loudest absolute sample before clamping. Diagnostics only — how far
    /// into clipping a project is, for a future meter.
    pub fn peak(&self) -> f32 {
        self.buffer
            .iter()
            .copied()
            .filter(|s| s.is_finite())
            .fold(0.0f32, |acc, s| acc.max(s.abs()))
    }
}

// ---------------------------------------------------------------------------
// The timeline walk
// ---------------------------------------------------------------------------

/// Sample frames one [`MixStream`] block holds. About 0.7 s at 48 kHz: big
/// enough that the per-block bookkeeping is noise, small enough that a block
/// is a few hundred kilobytes whatever the timeline's length.
pub const BLOCK_FRAMES: usize = 32_768;

/// The most source frames a resampling voice holds at once. A constant speed
/// reads `speed` source frames per output frame, so a block is cut into
/// shorter pieces when the speed is high rather than letting one piece read
/// minutes of source.
const MAX_WINDOW_FRAMES: usize = 1 << 18;

/// Mix every audio-bearing segment in `project` into one interleaved buffer.
///
/// Returns exactly `frames_for(project.duration())` sample frames, which is
/// what the encoder needs to keep audio and video the same length. The whole
/// timeline in memory: for a measurement or a test. The export streams
/// ([`MixStream`]).
pub fn mix_timeline(
    project: &Project,
    source: &dyn AudioSource,
    sample_rate: u32,
    channels: u16,
    cancel: &AtomicBool,
) -> Result<Vec<f32>> {
    let range = MixRange::whole(project, sample_rate);
    MixStream::new(project, source, sample_rate, channels, range, cancel)?.collect()
}

/// [`mix_timeline`] without the clamp at the end: what a compound clip's
/// mixed-down sound is (`sequence::bounce`), so its own compressor or a cut
/// in its equaliser sees the peaks the sum really has.
pub fn mix_timeline_unclamped(
    project: &Project,
    source: &dyn AudioSource,
    sample_rate: u32,
    channels: u16,
    cancel: &AtomicBool,
) -> Result<Vec<f32>> {
    let range = MixRange::whole(project, sample_rate);
    MixStream::new(project, source, sample_rate, channels, range, cancel)?
        .unclamped()
        .collect()
}

/// Which sample frames of the timeline a mix covers.
///
/// `first` is a timeline frame (`frames_for` of a time), `frames` the length
/// of the answer. Frames past the project's end are silence, so a range that
/// runs past it is padded rather than truncated (see [`slice_range`], which
/// this replaces for the export).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MixRange {
    pub first: usize,
    pub frames: usize,
}

impl MixRange {
    /// The whole project: `frames_for(project.duration())` frames from zero.
    pub fn whole(project: &Project, sample_rate: u32) -> Self {
        Self {
            first: 0,
            frames: frames_for(project.duration(), sample_rate),
        }
    }

    /// The audio of an export of `[start, start + duration)`. A whole-project
    /// export keeps the project's own length (what the buffered mix always
    /// had), a range export exactly the range's.
    pub fn export(project: &Project, sample_rate: u32, start: Micros, duration: Micros) -> Self {
        if start > 0 || duration < project.duration() {
            Self {
                first: frames_for(start.max(0), sample_rate),
                frames: frames_for(duration.max(0), sample_rate),
            }
        } else {
            Self::whole(project, sample_rate)
        }
    }
}

/// The timeline mixed block by block over one [`MixRange`].
///
/// The samples are the ones the whole-buffer mix had, bit for bit: every
/// segment is placed at the same frame, read through the same decoder from
/// the same position, scaled by the same gain and summed in the same order.
/// What changed is the shape — at most one block of the mix, one window of
/// each live clip's source and the processed renders of the clips under the
/// block are in memory, never the timeline. A clip outside the range is not
/// decoded at all.
///
/// A clip with audio effects, a pitch-preserving speed change or a speed
/// curve is still rendered whole (`audiofx::render`) when the range first
/// reaches it, because a compressor or a reverb tail depends on everything
/// before it in the clip. That costs memory for the clip, not the timeline,
/// and it is dropped as soon as the range has passed it.
pub struct MixStream<'a> {
    source: &'a dyn AudioSource,
    sample_rate: u32,
    channels: usize,
    /// Every audible segment, in the order the sum adds them.
    voices: Vec<Voice<'a>>,
    /// Timeline frame of the next frame [`Self::next_block`] produces.
    next: usize,
    /// Timeline frame one past the range.
    end: usize,
    /// Timeline frame one past the project: nothing is mixed at or after it.
    cap: usize,
    clamp: bool,
    cancel: &'a AtomicBool,
}

/// One audible segment, resolved.
struct Voice<'a> {
    /// Timeline frame of the voice's first sample frame.
    start: usize,
    /// Sample frames the voice contributes from `start`.
    frames: usize,
    base: f32,
    volume: Option<KeyframeTrack>,
    kind: VoiceKind<'a>,
}

enum VoiceKind<'a> {
    /// Rendered through `audiofx`, whole, the first time the range reaches it.
    Rendered {
        spec: crate::modules::audiofx::render::RenderSpec,
        samples: Option<Vec<f32>>,
    },
    /// Read from the file and, at a constant speed whose pitch moves,
    /// stretched linearly (`resample_linear`, piece by piece).
    Plain(PlainVoice<'a>),
}

struct PlainVoice<'a> {
    material_id: String,
    path: String,
    source_start: Micros,
    source_duration: Micros,
    /// Source frames the request covers (`frames_for(source_duration)`).
    in_frames: usize,
    stream: Option<Box<dyn AudioStream + 'a>>,
    /// Source frames `[window_start, window_start + window.len() / ch)`.
    window: Vec<f32>,
    window_start: usize,
}

impl<'a> MixStream<'a> {
    /// Resolve every audible segment of `project` for a mix of `range`.
    ///
    /// Compound clips are flattened here (`sequence::audio`), which renders
    /// the mix-down of any compound clip that processes its own sound and is
    /// not cached yet. Every file a clip under the range reads is opened once
    /// now, so a file without a readable audio stream fails the export before
    /// its first frame rather than in the middle of it.
    pub fn new(
        project: &Project,
        source: &'a dyn AudioSource,
        sample_rate: u32,
        channels: u16,
        range: MixRange,
        cancel: &'a AtomicBool,
    ) -> Result<Self> {
        let cancelled_or = |e: String| {
            if cancel.load(Ordering::Relaxed) {
                ExportError::Cancelled
            } else {
                ExportError::Audio(anyhow::anyhow!(e))
            }
        };
        let cap = frames_for(project.duration(), sample_rate);
        // Compound clips' sound, laid out on lanes of its own; see
        // `sequence::audio`. Borrowed when there are none. A compound clip
        // that processes its own sound is mixed down first and heard through
        // its effects (`sequence::bounce`), rendered here if it is not cached.
        let flat = crate::modules::sequence::audio::flatten_audio_rendered(project, cancel)
            .map_err(cancelled_or)?;
        let project = flat.as_ref();
        let channel_count = channels.max(1) as usize;
        let end = range.first.saturating_add(range.frames);

        let mut voices = Vec::new();
        for track in &project.tracks {
            // `muted` silences a lane and `hidden` conceals it: a hidden video
            // track is still heard, which is how people use a lane they are
            // comparing against.
            if track.muted || !track_bears_audio(track.kind) {
                continue;
            }
            for segment in &track.segments {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ExportError::Cancelled);
                }
                let Some(path) = audio_path(project, segment) else {
                    continue;
                };
                if segment.target_range.duration <= 0 {
                    continue;
                }
                // The same rule the preview mixer applies, and it has to be
                // applied in both places or an export sounds different from
                // what was monitored. A file imported with both streams
                // becomes two linked segments of one material — picture on a
                // video lane, sound on an audio lane — and `audio_path` above
                // resolves for both, because a video material carrying audio
                // contributes sound wherever it sits. Mixing both is the same
                // waveform summed with itself: 6 dB up and phasing with every
                // microsecond they are out by.
                if project.sound_is_on_a_linked_lane(track, segment) {
                    continue;
                }
                let start = frames_for(segment.target_range.start.max(0), sample_rate);
                let frames = frames_for(segment.target_range.duration, sample_rate);
                // Outside the range, or past the project's end: never heard
                // in this mix, so never decoded.
                let last = start.saturating_add(frames).min(cap);
                if last <= range.first || start >= end || start >= last {
                    continue;
                }

                // A speed factor changes how much source a segment consumes;
                // the document keeps both ranges, but `source_range.duration`
                // is the authority and speed is what maps between them.
                let speed = if segment.speed.is_finite() && segment.speed > 0.0 {
                    segment.speed as f64
                } else {
                    1.0
                };
                let source_duration =
                    ((segment.target_range.duration as f64) * speed).round() as Micros;

                // Voice cleanup swaps in a denoised file and adds a normalising
                // gain; the preview mixer resolves it the same way.
                let effective = crate::modules::voice::effective_source(project, segment, path);
                let track_gain = finite_or(track.volume, 1.0);
                let volume = segment
                    .keyframes
                    .iter()
                    .find(|k| k.property == AnimatableProperty::Volume)
                    .cloned();
                let base = finite_or(segment.volume, 1.0) * track_gain * effective.gain;

                // A pitch-preserving speed change, a speed curve or an effect
                // stack: the clip is rendered through `audiofx`, the function
                // the preview's cached render comes from, and mixed at speed 1.
                let kind =
                    match crate::modules::audiofx::spec_for(project, segment, &effective.path) {
                        Some(spec) => VoiceKind::Rendered {
                            spec,
                            samples: None,
                        },
                        None => VoiceKind::Plain(PlainVoice {
                            material_id: segment.material_id.clone(),
                            path: effective.path.clone(),
                            source_start: segment.source_range.start,
                            source_duration,
                            in_frames: frames_for(source_duration, sample_rate),
                            stream: None,
                            window: Vec::new(),
                            window_start: 0,
                        }),
                    };
                voices.push(Voice {
                    start,
                    frames,
                    base,
                    volume,
                    kind,
                });
            }
        }

        // Open every file once, up front: the mix used to be what told the
        // export the audio was decodable before any frame was encoded, and a
        // stream that finds out ten minutes in is worse.
        let mut probed = std::collections::HashSet::new();
        for voice in &voices {
            let path = match &voice.kind {
                VoiceKind::Rendered { spec, .. } => spec.path.as_str(),
                VoiceKind::Plain(plain) => plain.path.as_str(),
            };
            if probed.insert(path.to_string()) {
                source.probe(path).map_err(ExportError::Audio)?;
            }
        }

        Ok(Self {
            source,
            sample_rate,
            channels: channel_count,
            voices,
            next: range.first,
            end,
            cap,
            clamp: true,
            cancel,
        })
    }

    /// Hand out the sum as it is, only broken floats zeroed: a compound
    /// clip's mix-down, whose own effects must see the real peaks.
    pub fn unclamped(mut self) -> Self {
        self.clamp = false;
        self
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Sample frames still to come.
    pub fn remaining(&self) -> usize {
        self.end - self.next
    }

    /// The next block of at most `max_frames` sample frames, interleaved, into
    /// `out` (resized to fit). `false` once the range is exhausted.
    pub fn next_block(&mut self, max_frames: usize, out: &mut Vec<f32>) -> Result<bool> {
        let frames = max_frames.max(1).min(self.remaining());
        out.clear();
        if frames == 0 {
            return Ok(false);
        }
        if self.cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        out.resize(frames * self.channels, 0.0);
        let block_start = self.next;
        let block_end = block_start + frames;
        let mixed_end = block_end.min(self.cap);
        let mut scratch = Vec::new();
        for voice in &mut self.voices {
            let lo = block_start.max(voice.start);
            let hi = mixed_end.min(voice.start.saturating_add(voice.frames));
            if lo < hi {
                voice.mix(
                    self.source,
                    self.sample_rate,
                    self.channels,
                    lo - voice.start..hi - voice.start,
                    &mut out[(lo - block_start) * self.channels..],
                    &mut scratch,
                    self.cancel,
                )?;
            }
            // Past the voice: let its decoder and render go.
            if block_end >= voice.start.saturating_add(voice.frames) {
                voice.release();
            }
        }
        for s in out.iter_mut() {
            *s = if !s.is_finite() {
                // A NaN from a broken float source would otherwise poison
                // every downstream conversion.
                0.0
            } else if self.clamp {
                // Summing several segments can exceed 1.0 and every sample
                // format below float wraps rather than saturates; see
                // `AudioMixer::finish`.
                s.clamp(-1.0, 1.0)
            } else {
                *s
            };
        }
        self.next = block_end;
        Ok(true)
    }

    /// The whole range in one buffer.
    pub fn collect(mut self) -> Result<Vec<f32>> {
        let mut all = Vec::with_capacity(self.remaining().saturating_mul(self.channels));
        let mut block = Vec::new();
        while self.next_block(BLOCK_FRAMES, &mut block)? {
            all.extend_from_slice(&block);
        }
        Ok(all)
    }
}

/// The gain of a voice's sample frame `frame`, counted from its start.
fn voice_gain(base: f32, volume: Option<&KeyframeTrack>, frame: usize, sample_rate: u32) -> f32 {
    match volume {
        None => base,
        // Keyframe times are relative to the segment start, so the offset is
        // the frame's position inside the segment, not on the timeline.
        Some(track) => {
            let offset = micros_for(frame, sample_rate);
            base * track
                .sample(offset)
                .map(|v| finite_or(v, 1.0))
                .unwrap_or(1.0)
        }
    }
}

impl<'a> Voice<'a> {
    fn release(&mut self) {
        match &mut self.kind {
            VoiceKind::Rendered { samples, .. } => *samples = None,
            VoiceKind::Plain(plain) => {
                plain.stream = None;
                plain.window = Vec::new();
            }
        }
    }

    /// Add the voice's frames `frames` (counted from its start) into `out`,
    /// whose first frame is the voice's frame `frames.start`.
    #[allow(clippy::too_many_arguments)] // one call site, all of it the mix's state
    fn mix(
        &mut self,
        source: &'a dyn AudioSource,
        sample_rate: u32,
        channels: usize,
        frames: std::ops::Range<usize>,
        out: &mut [f32],
        scratch: &mut Vec<f32>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let Voice {
            frames: out_frames,
            base,
            volume,
            kind,
            ..
        } = self;
        let (base, volume, out_frames) = (*base, volume.as_ref(), *out_frames);
        let base_frame = frames.start;
        // The arithmetic of `AudioMixer::mix_at`, sample for sample, so the
        // sum is the same floats in the same order.
        let mut add = |samples: &[f32], first: usize| {
            for (i, frame_samples) in samples.chunks_exact(channels).enumerate() {
                let frame = first + i;
                let g = voice_gain(base, volume, frame, sample_rate);
                let at = (frame - base_frame) * channels;
                for (channel, sample) in frame_samples.iter().enumerate() {
                    out[at + channel] += sample * g;
                }
            }
        };
        match kind {
            VoiceKind::Rendered { spec, samples } => {
                if samples.is_none() {
                    let rendered =
                        crate::modules::audiofx::render(spec, source, sample_rate, cancel)
                            .map_err(|e| {
                                if cancel.load(Ordering::Relaxed) {
                                    ExportError::Cancelled
                                } else {
                                    ExportError::Audio(anyhow::anyhow!(e))
                                }
                            })?;
                    *samples = Some(remap_stereo(rendered, channels));
                }
                let all = samples.as_deref().unwrap_or_default();
                // A render shorter than the clip adds nothing past its end,
                // as the buffered mix did.
                let hi = frames.end.min(all.len() / channels);
                if frames.start < hi {
                    add(&all[frames.start * channels..hi * channels], frames.start);
                }
            }
            VoiceKind::Plain(plain) => {
                let mut first = frames.start;
                while first < frames.end {
                    let piece = plain.read(
                        source,
                        sample_rate,
                        channels,
                        first..frames.end,
                        out_frames,
                        scratch,
                    )?;
                    add(scratch, first);
                    first += piece;
                }
            }
        }
        Ok(())
    }
}

impl<'a> PlainVoice<'a> {
    /// Fill `scratch` with the voice's stretched frames from `wanted.start`,
    /// at most up to `wanted.end`, and answer how many it holds: exactly what
    /// `resample_linear` makes of the whole clip, one piece of it.
    fn read(
        &mut self,
        source: &'a dyn AudioSource,
        sample_rate: u32,
        channels: usize,
        wanted: std::ops::Range<usize>,
        out_frames: usize,
        scratch: &mut Vec<f32>,
    ) -> Result<usize> {
        let in_frames = self.in_frames;
        let first = wanted.start;
        scratch.clear();
        if in_frames == 0 {
            // `resample_linear` of nothing: silence of the clip's length.
            let count = wanted.len();
            scratch.resize(count * channels, 0.0);
            return Ok(count);
        }
        if self.stream.is_none() {
            let request = AudioRequest {
                material_id: &self.material_id,
                path: &self.path,
                start: self.source_start,
                duration: self.source_duration,
                sample_rate,
                channels: channels as u16,
            };
            self.stream = Some(source.stream(&request).map_err(ExportError::Audio)?);
        }
        let stream = self.stream.as_mut().expect("opened above");

        if in_frames == out_frames {
            // No stretch: the source frames are the voice's frames.
            let count = wanted.len().min(MAX_WINDOW_FRAMES);
            scratch.resize(count * channels, 0.0);
            stream
                .read(first, scratch.as_mut_slice())
                .map_err(ExportError::Audio)?;
            return Ok(count);
        }

        let ratio = in_frames as f64 / out_frames as f64;
        // Output frames whose source window stays under the bound.
        let per_piece = ((MAX_WINDOW_FRAMES as f64 / ratio.max(1.0)) as usize).max(1);
        let count = wanted.len().min(per_piece);
        let last = first + count - 1;
        let index_of = |frame: usize| ((frame as f64 * ratio).floor() as usize).min(in_frames - 1);
        let need_lo = index_of(first);
        let need_hi = (index_of(last) + 1).min(in_frames - 1);

        // Slide the window: drop what is behind, read what is ahead.
        let have_end = self.window_start + self.window.len() / channels;
        if need_lo >= have_end || need_lo < self.window_start {
            self.window.clear();
            self.window_start = need_lo;
        } else if need_lo > self.window_start {
            self.window
                .drain(..(need_lo - self.window_start) * channels);
            self.window_start = need_lo;
        }
        let have_end = self.window_start + self.window.len() / channels;
        if need_hi + 1 > have_end {
            let old = self.window.len();
            self.window
                .resize((need_hi + 1 - self.window_start) * channels, 0.0);
            stream
                .read(have_end, &mut self.window[old..])
                .map_err(ExportError::Audio)?;
        }

        scratch.reserve(count * channels);
        let window = &self.window;
        let window_start = self.window_start;
        for frame in first..first + count {
            let position = frame as f64 * ratio;
            let index = position.floor() as usize;
            let fraction = (position - index as f64) as f32;
            let next = (index + 1).min(in_frames - 1);
            let index = index.min(in_frames - 1);
            for channel in 0..channels {
                let a = window[(index - window_start) * channels + channel];
                let b = window[(next - window_start) * channels + channel];
                scratch.push(a + (b - a) * fraction);
            }
        }
        Ok(count)
    }
}

/// The slice of a full-project mix that lies inside `[start, start + duration)`,
/// padded with silence to exactly the range's length.
///
/// A range export walks only its own frames, and the audio has to match them
/// sample for sample: a bed that is one frame short would end early, and one
/// that keeps the project's length would drift the whole file. So the length
/// of the answer is `frames_for(duration)` whatever the input held — a range
/// that runs past the mix (clamping can leave the video one frame longer than
/// the sound) is padded rather than truncated.
pub fn slice_range(
    mixed: Vec<f32>,
    channels: usize,
    sample_rate: u32,
    start: Micros,
    duration: Micros,
) -> Vec<f32> {
    let channels = channels.max(1);
    let total = mixed.len() / channels;
    let from = frames_for(start.max(0), sample_rate).min(total);
    let want = frames_for(duration.max(0), sample_rate);
    let to = (from + want).min(total);
    let mut slice = mixed[from * channels..to * channels].to_vec();
    slice.resize(want * channels, 0.0);
    slice
}

/// A stereo render as `channels` channels: the mono mix for one, the pair
/// repeated for more. Renders are stereo because the mix bus is.
fn remap_stereo(stereo: Vec<f32>, channels: usize) -> Vec<f32> {
    if channels == 2 {
        return stereo;
    }
    let mut out = Vec::with_capacity(stereo.len() / 2 * channels);
    for frame in stereo.as_chunks::<2>().0 {
        if channels == 1 {
            out.push((frame[0] + frame[1]) * 0.5);
        } else {
            for c in 0..channels {
                out.push(frame[c % 2]);
            }
        }
    }
    out
}

/// Which track kinds can contribute sound.
fn track_bears_audio(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Audio | TrackKind::Video)
}

/// The file a segment's audio would come from, or `None` when it has none.
fn audio_path<'a>(project: &'a Project, segment: &Segment) -> Option<&'a str> {
    if let Some(audio) = project.materials.audio(&segment.material_id) {
        return Some(&audio.path);
    }
    // A video material only counts when the container actually carries an
    // audio stream; asking a decoder for the audio of a silent file is a
    // guaranteed error per segment.
    project
        .materials
        .video(&segment.material_id)
        .filter(|video| video.has_audio)
        .map(|video| video.path.as_str())
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        fallback
    }
}

/// Sample frames in `duration`.
pub fn frames_for(duration: Micros, sample_rate: u32) -> usize {
    if duration <= 0 {
        return 0;
    }
    let numerator = duration as i128 * sample_rate as i128;
    let denominator = MICROS_PER_SECOND as i128;
    ((numerator + denominator / 2) / denominator) as usize
}

/// Inverse of [`frames_for`].
pub fn micros_for(frames: usize, sample_rate: u32) -> Micros {
    if sample_rate == 0 {
        return 0;
    }
    (frames as i128 * MICROS_PER_SECOND as i128 / sample_rate as i128) as Micros
}

/// Stretch or squeeze interleaved audio to `out_frames`.
///
/// Linear interpolation between neighbouring sample frames. This is the naive
/// speed change: the pitch moves with the rate, because the samples are simply
/// read faster. Only "Change audio pitch" clips come through here; the
/// pitch-preserving stretch is `modules::audiofx::stretch`.
///
/// The mapping is `in_frames / out_frames` per output frame rather than
/// `(in-1)/(out-1)`: the endpoint-preserving form would make a segment's
/// playback rate depend on its length, so two halves of a split clip would no
/// longer join seamlessly.
pub fn resample_linear(input: &[f32], channels: usize, out_frames: usize) -> Vec<f32> {
    let channels = channels.max(1);
    let in_frames = input.len() / channels;
    if out_frames == 0 {
        return Vec::new();
    }
    if in_frames == 0 {
        return vec![0.0; out_frames * channels];
    }
    if in_frames == out_frames {
        return input.to_vec();
    }

    let ratio = in_frames as f64 / out_frames as f64;
    let mut out = Vec::with_capacity(out_frames * channels);
    for frame in 0..out_frames {
        let position = frame as f64 * ratio;
        let index = position.floor() as usize;
        let fraction = (position - index as f64) as f32;
        let next = (index + 1).min(in_frames - 1);
        let index = index.min(in_frames - 1);
        for channel in 0..channels {
            let a = input[index * channels + channel];
            let b = input[next * channels + channel];
            out.push(a + (b - a) * fraction);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        AudioMaterial, CanvasConfig, Easing, Keyframe, KeyframeTrack, TimeRange, Track, Transform,
        VideoMaterial,
    };

    const RATE: u32 = 48_000;

    /// A source that answers every request with the same constant sample.
    struct Constant {
        value: f32,
    }

    impl AudioSource for Constant {
        fn samples(&self, request: &AudioRequest<'_>) -> anyhow::Result<Vec<f32>> {
            Ok(vec![
                self.value;
                request.frames() * request.channels.max(1) as usize
            ])
        }
    }

    /// A source that records what it was asked for.
    #[derive(Default)]
    struct Recording {
        requests: parking_lot::Mutex<Vec<(String, Micros, Micros)>>,
    }

    impl AudioSource for Recording {
        fn samples(&self, request: &AudioRequest<'_>) -> anyhow::Result<Vec<f32>> {
            self.requests.lock().push((
                request.material_id.to_string(),
                request.start,
                request.duration,
            ));
            Ok(vec![1.0; request.frames() * request.channels as usize])
        }
    }

    fn project() -> Project {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.materials.audios.push(AudioMaterial {
            id: "a1".into(),
            path: "/tmp/a1.wav".into(),
            duration: 10 * MICROS_PER_SECOND,
            sample_rate: 48_000,
            channels: 2,
        });
        project.materials.videos.push(VideoMaterial {
            id: "v1".into(),
            path: "/tmp/v1.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 10 * MICROS_PER_SECOND,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        project.materials.videos.push(VideoMaterial {
            id: "silent".into(),
            path: "/tmp/silent.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 10 * MICROS_PER_SECOND,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        project
    }

    fn segment(material: &str, start: Micros, duration: Micros) -> Segment {
        Segment {
            id: format!("s-{material}-{start}"),
            material_id: material.into(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(0, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    fn track(kind: TrackKind, segments: Vec<Segment>) -> Track {
        let mut track = Track::new(kind, "lane");
        track.segments = segments;
        track
    }

    #[test]
    fn frame_counts_follow_the_sample_rate() {
        assert_eq!(frames_for(MICROS_PER_SECOND, 48_000), 48_000);
        assert_eq!(frames_for(MICROS_PER_SECOND / 2, 44_100), 22_050);
        assert_eq!(frames_for(0, 48_000), 0);
        assert_eq!(frames_for(-1, 48_000), 0);
        assert_eq!(micros_for(48_000, 48_000), MICROS_PER_SECOND);
    }

    #[test]
    fn gain_scales_what_is_mixed_in() {
        let mut mixer = AudioMixer::new(RATE, 2, MICROS_PER_SECOND);
        mixer.mix_at_gain(&[0.5; 200], 0, 0.5);
        let mixed = mixer.finish();
        assert!((mixed[0] - 0.25).abs() < 1e-6);
        assert!((mixed[199] - 0.25).abs() < 1e-6);
        // Nothing was written past the samples supplied.
        assert_eq!(mixed[200], 0.0);
    }

    #[test]
    fn overlapping_segments_sum_and_then_clip() {
        let mut mixer = AudioMixer::new(RATE, 2, MICROS_PER_SECOND);
        mixer.mix_at_gain(&[0.8; 100], 0, 1.0);
        mixer.mix_at_gain(&[0.8; 100], 0, 1.0);
        // 1.6 before the clamp: the peak is what a meter would show.
        assert!((mixer.peak() - 1.6).abs() < 1e-6);

        let mixed = mixer.finish();
        assert!((mixed[0] - 1.0).abs() < 1e-6);
        assert!(mixed.iter().all(|s| (-1.0..=1.0).contains(s)));
    }

    #[test]
    fn negative_peaks_clip_symmetrically() {
        let mut mixer = AudioMixer::new(RATE, 2, MICROS_PER_SECOND);
        mixer.mix_at_gain(&[-0.9; 100], 0, 2.0);
        let mixed = mixer.finish();
        assert!((mixed[0] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_broken_sample_does_not_poison_the_mix() {
        let mut mixer = AudioMixer::new(RATE, 2, MICROS_PER_SECOND);
        mixer.mix_at_gain(&[f32::NAN, 0.5], 0, 1.0);
        let mixed = mixer.finish();
        assert_eq!(mixed[0], 0.0);
        assert!((mixed[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn samples_land_at_the_segments_position() {
        let mut mixer = AudioMixer::new(RATE, 2, 2 * MICROS_PER_SECOND);
        mixer.mix_at_gain(&[1.0; 2], MICROS_PER_SECOND, 1.0);
        let mixed = mixer.finish();
        assert_eq!(mixed[0], 0.0);
        // One second in, in interleaved stereo, is sample index rate*2.
        assert_eq!(mixed[RATE as usize * 2], 1.0);
    }

    #[test]
    fn audio_past_the_end_of_the_mix_is_dropped_not_appended() {
        let mut mixer = AudioMixer::new(RATE, 2, 1_000);
        let frames = mixer.frames();
        mixer.mix_at_gain(&[1.0; 10_000], 0, 1.0);
        assert_eq!(mixer.frames(), frames);
        assert_eq!(mixer.finish().len(), frames * 2);
    }

    #[test]
    fn resampling_keeps_the_rate_constant_rather_than_the_endpoints() {
        // Halving the length reads every second frame: a 2x speed change.
        let input = vec![0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0];
        let out = resample_linear(&input, 2, 2);
        assert_eq!(out, vec![0.0, 0.0, 2.0, 2.0]);
    }

    #[test]
    fn resampling_interpolates_between_frames() {
        let input = vec![0.0, 4.0];
        let out = resample_linear(&input, 1, 4);
        assert_eq!(out.len(), 4);
        assert!((out[1] - 2.0).abs() < 1e-6);
        assert!((out[2] - 4.0).abs() < 1e-6);
        // The last frame is clamped to the end of the input rather than
        // reading past it.
        assert!((out[3] - 4.0).abs() < 1e-6);
    }

    #[test]
    fn resampling_degenerate_input_gives_silence_of_the_right_length() {
        assert_eq!(resample_linear(&[], 2, 3), vec![0.0; 6]);
        assert_eq!(resample_linear(&[1.0, 1.0], 2, 0), Vec::<f32>::new());
    }

    /// An import puts a clip's picture and its sound on two lanes as one linked
    /// pair. Both segments name the same video material, and `audio_path`
    /// resolves for both — so without the linked-lane rule the export mixes the
    /// waveform with itself and comes out 6 dB hotter than the preview.
    ///
    /// The assertion is on the *level*, not on a boolean, because that is the
    /// symptom: heard once at the source level, twice at double it.
    ///
    /// The source is deliberately quiet. `AudioMixer::finish` clamps to full
    /// scale, so at 1.0 both the correct and the doubled mix peak at exactly
    /// 1.0 and this test passes either way — it was written that way first, and
    /// only the negative case below exposed it.
    #[test]
    fn a_linked_pair_is_heard_once_and_at_the_level_the_preview_gave_it() {
        let mut project = project();
        let group = "link-1".to_string();
        project.materials.links.insert(group.clone());

        let mut picture = segment("v1", 0, MICROS_PER_SECOND);
        picture.id = "picture".into();
        picture.extras.push(group.clone());
        let mut sound = segment("v1", 0, MICROS_PER_SECOND);
        sound.id = "sound".into();
        sound.extras.push(group.clone());

        project
            .tracks
            .push(track(TrackKind::Video, vec![picture.clone()]));
        project
            .tracks
            .push(track(TrackKind::Audio, vec![sound.clone()]));

        let mixed = mix_timeline(
            &project,
            &Constant { value: 0.25 },
            RATE,
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        let peak = mixed.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
        assert!(
            (peak - 0.25).abs() < 1e-6,
            "a linked pair peaked at {peak}, want 0.25 — at 0.5 it was mixed twice"
        );

        // And the deferral is about where the partner *sits*, not about a
        // stored role: with the sound on its own audio lane and nothing linked,
        // the same two segments are two independent sources and both are heard.
        let mut unlinked = project.clone();
        unlinked.materials.links.clear();
        for lane in &mut unlinked.tracks {
            for seg in &mut lane.segments {
                seg.extras.clear();
            }
        }
        let mixed = mix_timeline(
            &unlinked,
            &Constant { value: 0.25 },
            RATE,
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        let peak = mixed.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
        assert!(
            (peak - 0.5).abs() < 1e-6,
            "two unlinked segments of the same material should sum to 0.5, peaked at {peak}"
        );
    }

    #[test]
    fn a_muted_track_contributes_nothing() {
        let mut project = project();
        let mut lane = track(TrackKind::Audio, vec![segment("a1", 0, MICROS_PER_SECOND)]);
        lane.muted = true;
        project.tracks.push(lane);

        let mixed = mix_timeline(
            &project,
            &Constant { value: 1.0 },
            RATE,
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(mixed.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn track_and_segment_volume_multiply() {
        let mut project = project();
        let mut seg = segment("a1", 0, MICROS_PER_SECOND);
        seg.volume = 0.5;
        let mut lane = track(TrackKind::Audio, vec![seg]);
        lane.volume = 0.5;
        project.tracks.push(lane);

        let mixed = mix_timeline(
            &project,
            &Constant { value: 1.0 },
            RATE,
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!((mixed[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_silent_video_material_is_skipped() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Video,
            vec![segment("silent", 0, MICROS_PER_SECOND)],
        ));

        let mixed = mix_timeline(
            &project,
            &Constant { value: 1.0 },
            RATE,
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(mixed.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_video_track_with_sound_is_mixed() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Video,
            vec![segment("v1", 0, MICROS_PER_SECOND)],
        ));

        let mixed = mix_timeline(
            &project,
            &Constant { value: 0.5 },
            RATE,
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!((mixed[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn speed_asks_the_source_for_a_longer_range() {
        // "Change audio pitch" on: the tape-speed path, which reads exactly the
        // source the clip spans and resamples it.
        let mut project = project();
        let mut seg = segment("a1", 0, MICROS_PER_SECOND);
        seg.speed = 2.0;
        seg.source_range = TimeRange::new(500_000, 2 * MICROS_PER_SECOND);
        seg.extras.push("fx".into());
        project.materials.extras.insert(
            "fx".into(),
            serde_json::json!({ "audio_fx": { "pitch_follows_speed": true } }),
        );
        project.tracks.push(track(TrackKind::Audio, vec![seg]));

        let source = Recording::default();
        let mixed = mix_timeline(&project, &source, RATE, 2, &AtomicBool::new(false)).unwrap();

        let requests = source.requests.lock().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].1, 500_000);
        // One second of timeline at 2x reads two seconds of source.
        assert_eq!(requests[0].2, 2 * MICROS_PER_SECOND);
        // And still lands as one second of output.
        assert_eq!(mixed.len(), frames_for(MICROS_PER_SECOND, RATE) * 2);
    }

    #[test]
    fn a_pitch_kept_speed_change_reads_around_the_clip_and_lands_at_its_length() {
        // The default: the stretcher reads pre-roll before the clip and runs
        // past its end, then the result is exactly the clip's length.
        let mut project = project();
        let mut seg = segment("a1", 0, MICROS_PER_SECOND);
        seg.speed = 2.0;
        seg.source_range = TimeRange::new(2 * MICROS_PER_SECOND, 2 * MICROS_PER_SECOND);
        project.tracks.push(track(TrackKind::Audio, vec![seg]));

        let source = Recording::default();
        let mixed = mix_timeline(&project, &source, RATE, 2, &AtomicBool::new(false)).unwrap();
        let requests = source.requests.lock().clone();
        assert_eq!(requests.len(), 1);
        let (start, duration) = (requests[0].1, requests[0].2);
        assert!(start < 2 * MICROS_PER_SECOND, "pre-roll before the clip");
        assert!(start + duration > 4 * MICROS_PER_SECOND, "and past its end");
        assert_eq!(mixed.len(), frames_for(MICROS_PER_SECOND, RATE) * 2);
    }

    #[test]
    fn a_clip_on_a_speed_curve_is_heard() {
        use crate::modules::project::{SpeedCurveMaterial, SpeedPoint};
        let mut project = project();
        let points = vec![
            SpeedPoint {
                source: 0,
                speed: 0.5,
            },
            SpeedPoint {
                source: 2 * MICROS_PER_SECOND,
                speed: 2.0,
            },
        ];
        let source_range = TimeRange::new(0, 2 * MICROS_PER_SECOND);
        let length = crate::modules::project::speed::curve_target_duration(&points, source_range);
        project.materials.speed_curves.push(SpeedCurveMaterial {
            id: "curve".into(),
            preset: None,
            points,
        });
        let mut seg = segment("a1", 0, length);
        seg.source_range = source_range;
        seg.extras.push("curve".into());
        project.tracks.push(track(TrackKind::Audio, vec![seg]));

        let mixed = mix_timeline(
            &project,
            &Constant { value: 0.25 },
            RATE,
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        // A constant source stretched is still about that constant; what
        // matters is that it is not the silence a curved clip used to be.
        let middle = &mixed[mixed.len() / 4..mixed.len() * 3 / 4];
        let level = middle.iter().map(|s| s.abs()).sum::<f32>() / middle.len() as f32;
        assert!(level > 0.1, "a curved clip is no longer muted ({level})");
    }

    /// A source whose samples depend on where in the file they are: two
    /// tones, so a phase shift or a dip between two mixes shows up as a
    /// difference. A constant source would hide both.
    struct Tones;

    impl AudioSource for Tones {
        fn samples(&self, request: &AudioRequest<'_>) -> anyhow::Result<Vec<f32>> {
            let rate = request.sample_rate as f64;
            let first =
                crate::modules::audio::clock::micros_to_frames(request.start, request.sample_rate);
            let channels = request.channels.max(1) as usize;
            let mut out = Vec::with_capacity(request.frames() * channels);
            for i in 0..request.frames() {
                let t = (first + i as i64) as f64 / rate;
                let v = 0.3 * (std::f64::consts::TAU * 220.0 * t).sin()
                    + 0.2 * (std::f64::consts::TAU * 1_330.0 * t).sin();
                for _ in 0..channels {
                    out.push(v as f32);
                }
            }
            Ok(out)
        }
    }

    /// The same document mixed twice is the same samples. Pitch-preserving
    /// audio through a speed curve used to differ by up to 0.12 between two
    /// mixes, with phase shifts and dips: the stretcher randomises its phase
    /// advance whenever it slows by more than 2x and seeded that randomness
    /// from `std::random_device`. The curve here goes down to 0.2x to reach
    /// that path, and up to 3x for the other end.
    #[test]
    fn a_speed_curve_mixes_to_the_same_samples_every_time() {
        use crate::modules::project::{SpeedCurveMaterial, SpeedPoint};
        let mut project = project();
        let points = vec![
            SpeedPoint {
                source: 0,
                speed: 1.0,
            },
            SpeedPoint {
                source: 600_000,
                speed: 0.2,
            },
            SpeedPoint {
                source: 1_200_000,
                speed: 3.0,
            },
            SpeedPoint {
                source: 3 * MICROS_PER_SECOND,
                speed: 0.4,
            },
        ];
        let source_range = TimeRange::new(500_000, 3 * MICROS_PER_SECOND);
        let length = crate::modules::project::speed::curve_target_duration(&points, source_range);
        project.materials.speed_curves.push(SpeedCurveMaterial {
            id: "curve".into(),
            preset: None,
            points,
        });
        let mut seg = segment("a1", 0, length);
        seg.source_range = source_range;
        seg.extras.push("curve".into());
        project.tracks.push(track(TrackKind::Audio, vec![seg]));

        let mix = || mix_timeline(&project, &Tones, RATE, 2, &AtomicBool::new(false)).unwrap();
        let first = mix();
        assert!(
            first.iter().any(|s| s.abs() > 0.05),
            "the curved clip is heard at all"
        );
        // Mixed on other threads as well: nothing may depend on which thread
        // or in which order the renders run.
        let others: Vec<Vec<f32>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..3).map(|_| scope.spawn(mix)).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (run, other) in std::iter::once(mix()).chain(others).enumerate() {
            assert_eq!(first.len(), other.len());
            let worst = first
                .iter()
                .zip(&other)
                .enumerate()
                .map(|(i, (a, b))| (i, (a - b).abs()))
                .fold((0, 0.0f32), |w, d| if d.1 > w.1 { d } else { w });
            assert!(
                worst.1 == 0.0,
                "mix {} differs from the first by {} at sample {} of {}",
                run + 2,
                worst.1,
                worst.0,
                first.len()
            );
        }
    }

    #[test]
    fn volume_keyframes_fade_the_segment() {
        let mut project = project();
        let mut seg = segment("a1", 0, MICROS_PER_SECOND);
        seg.keyframes.push(KeyframeTrack {
            property: AnimatableProperty::Volume,
            keyframes: vec![
                Keyframe {
                    time: 0,
                    value: 0.0,
                    easing: Easing::Linear,
                },
                Keyframe {
                    time: MICROS_PER_SECOND,
                    value: 1.0,
                    easing: Easing::Linear,
                },
            ],
        });
        project.tracks.push(track(TrackKind::Audio, vec![seg]));

        let mixed = mix_timeline(
            &project,
            &Constant { value: 1.0 },
            RATE,
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(mixed[0].abs() < 1e-6);
        let middle = (RATE as usize / 2) * 2;
        assert!((mixed[middle] - 0.5).abs() < 0.01);
        let last = mixed.len() - 2;
        assert!(mixed[last] > 0.99);
    }

    #[test]
    fn the_mix_is_exactly_as_long_as_the_project() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![
                segment("a1", 0, MICROS_PER_SECOND),
                segment("a1", 2 * MICROS_PER_SECOND, MICROS_PER_SECOND),
            ],
        ));
        assert_eq!(project.duration(), 3 * MICROS_PER_SECOND);

        let mixed = mix_timeline(
            &project,
            &Constant { value: 1.0 },
            RATE,
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(mixed.len(), frames_for(3 * MICROS_PER_SECOND, RATE) * 2);
        // The gap between the two segments is silent.
        let gap = frames_for(3 * MICROS_PER_SECOND / 2, RATE) * 2;
        assert_eq!(mixed[gap], 0.0);
    }

    #[test]
    fn cancellation_stops_the_walk() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", 0, MICROS_PER_SECOND)],
        ));

        let cancel = AtomicBool::new(true);
        let error = mix_timeline(&project, &Constant { value: 1.0 }, RATE, 2, &cancel).unwrap_err();
        assert!(error.is_cancellation());
    }

    #[test]
    fn a_source_failure_is_reported_rather_than_silently_dropped() {
        struct Broken;
        impl AudioSource for Broken {
            fn samples(&self, _: &AudioRequest<'_>) -> anyhow::Result<Vec<f32>> {
                Err(anyhow::anyhow!("the file went away"))
            }
        }

        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", 0, MICROS_PER_SECOND)],
        ));

        let error = mix_timeline(&project, &Broken, RATE, 2, &AtomicBool::new(false)).unwrap_err();
        assert!(error.to_string().contains("the file went away"));
    }

    // -- slicing a mix for a range export ---------------------------------

    /// A recognisable mix: sample frame `n` holds the value `n` on every
    /// channel, so the slice's *content* can be asserted, not only its length.
    fn counting_mix(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| std::iter::repeat_n(n as f32, channels))
            .collect()
    }

    #[test]
    fn a_slice_is_exactly_as_long_as_its_range_and_starts_at_the_right_sample() {
        // Two seconds of mix; take the middle second.
        let mixed = counting_mix(2 * RATE as usize, 2);
        let slice = slice_range(mixed, 2, RATE, 500_000, MICROS_PER_SECOND);

        assert_eq!(slice.len(), RATE as usize * 2);
        // The first sample frame of the slice is the mix's frame at 0.5 s.
        assert_eq!(slice[0], (RATE / 2) as f32);
        // And the last is the frame just before 1.5 s.
        assert_eq!(slice[slice.len() - 1], (RATE + RATE / 2 - 1) as f32);
    }

    #[test]
    fn a_slice_past_the_end_of_the_mix_is_padded_with_silence_not_truncated() {
        // One second of mix, a range asking for its last half plus another
        // half that does not exist — which is what clamping a video range one
        // frame longer than the audio produces.
        let mixed = counting_mix(RATE as usize, 2);
        let slice = slice_range(mixed, 2, RATE, 500_000, MICROS_PER_SECOND);

        assert_eq!(slice.len(), RATE as usize * 2, "the range's length, always");
        assert_eq!(slice[0], (RATE / 2) as f32);
        // Everything past the mix's end is silence.
        assert!(slice[RATE as usize..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_whole_project_slice_is_the_mix_unchanged() {
        let mixed = counting_mix(RATE as usize, 2);
        let slice = slice_range(mixed.clone(), 2, RATE, 0, MICROS_PER_SECOND);
        assert_eq!(slice, mixed);
    }

    // -- the streamed mix against the whole-buffer one ----------------------

    /// The whole-buffer mix as it was before the export streamed, kept
    /// verbatim as the reference the stream must equal bit for bit.
    fn reference_mix(
        project: &Project,
        source: &dyn AudioSource,
        rate: u32,
        channels: u16,
        clamp: bool,
    ) -> Vec<f32> {
        let cancel = AtomicBool::new(false);
        let mut mixer = AudioMixer::new(rate, channels, project.duration());
        let flat =
            crate::modules::sequence::audio::flatten_audio_rendered(project, &cancel).unwrap();
        let project = flat.as_ref();
        for track in &project.tracks {
            if track.muted || !track_bears_audio(track.kind) {
                continue;
            }
            for segment in &track.segments {
                let Some(path) = audio_path(project, segment) else {
                    continue;
                };
                if segment.target_range.duration <= 0 {
                    continue;
                }
                if project.sound_is_on_a_linked_lane(track, segment) {
                    continue;
                }
                let speed = if segment.speed.is_finite() && segment.speed > 0.0 {
                    segment.speed as f64
                } else {
                    1.0
                };
                let source_duration =
                    ((segment.target_range.duration as f64) * speed).round() as Micros;
                let effective = crate::modules::voice::effective_source(project, segment, path);
                let channel_count = channels.max(1) as usize;
                let track_gain = finite_or(track.volume, 1.0);
                let volume_track = segment
                    .keyframes
                    .iter()
                    .find(|k| k.property == AnimatableProperty::Volume);
                let base = finite_or(segment.volume, 1.0) * track_gain * effective.gain;
                let gain_at = |frame: usize| match volume_track {
                    None => base,
                    Some(track) => {
                        let offset = micros_for(frame, rate);
                        base * track
                            .sample(offset)
                            .map(|v| finite_or(v, 1.0))
                            .unwrap_or(1.0)
                    }
                };
                if let Some(spec) =
                    crate::modules::audiofx::spec_for(project, segment, &effective.path)
                {
                    let rendered =
                        crate::modules::audiofx::render(&spec, source, rate, &cancel).unwrap();
                    let rendered = remap_stereo(rendered, channel_count);
                    mixer.mix_at(&rendered, segment.target_range.start, gain_at);
                    continue;
                }
                let request = AudioRequest {
                    material_id: &segment.material_id,
                    path: &effective.path,
                    start: segment.source_range.start,
                    duration: source_duration,
                    sample_rate: rate,
                    channels,
                };
                let decoded = source.samples(&request).unwrap();
                let target_frames = frames_for(segment.target_range.duration, rate);
                let stretched = resample_linear(&decoded, channel_count, target_frames);
                mixer.mix_at(&stretched, segment.target_range.start, gain_at);
            }
        }
        if clamp {
            mixer.finish()
        } else {
            mixer.finish_unclamped()
        }
    }

    fn streamed(
        project: &Project,
        source: &dyn AudioSource,
        channels: u16,
        range: MixRange,
        block: usize,
    ) -> Vec<f32> {
        let cancel = AtomicBool::new(false);
        let mut stream = MixStream::new(project, source, RATE, channels, range, &cancel).unwrap();
        let mut all = Vec::new();
        let mut out = Vec::new();
        while stream.next_block(block, &mut out).unwrap() {
            all.extend_from_slice(&out);
        }
        all
    }

    /// Everything the mixer does at once: plain clips, a gap, a linked pair,
    /// volume keyframes, tape-speed clips at odd ratios (both directions), a
    /// pitch-kept speed change, a curve, an effect stack, overlapping lanes
    /// that clip, a muted lane and a clip that starts inside its source.
    fn busy_project() -> Project {
        use crate::modules::project::{SpeedCurveMaterial, SpeedPoint};
        let mut project = project();
        let s = MICROS_PER_SECOND;
        project.materials.extras.insert(
            "tape".into(),
            serde_json::json!({ "audio_fx": { "pitch_follows_speed": true } }),
        );
        let mut a = segment("a1", 0, 2 * s);
        a.source_range = TimeRange::new(300_000, 2 * s);
        a.keyframes.push(KeyframeTrack {
            property: AnimatableProperty::Volume,
            keyframes: vec![
                Keyframe {
                    time: 0,
                    value: 0.2,
                    easing: Easing::Linear,
                },
                Keyframe {
                    time: 2 * s,
                    value: 1.4,
                    easing: Easing::Linear,
                },
            ],
        });
        let mut fast = segment("a1", 2_500_000, 1_300_001);
        fast.speed = 2.37;
        fast.extras.push("tape".into());
        let mut slow = segment("a1", 4_000_000, 1_700_000);
        slow.speed = 0.61;
        slow.source_range = TimeRange::new(1_234_567, 1_037_000);
        slow.extras.push("tape".into());
        let mut kept = segment("a1", 6 * s, s);
        kept.speed = 1.5;
        let points = vec![
            SpeedPoint {
                source: 0,
                speed: 0.5,
            },
            SpeedPoint {
                source: s,
                speed: 2.0,
            },
        ];
        let source_range = TimeRange::new(0, s);
        let length = crate::modules::project::speed::curve_target_duration(&points, source_range);
        project.materials.speed_curves.push(SpeedCurveMaterial {
            id: "curve".into(),
            preset: None,
            points,
        });
        let mut curved = segment("a1", 7_100_000, length);
        curved.source_range = source_range;
        curved.extras.push("curve".into());
        project
            .tracks
            .push(track(TrackKind::Audio, vec![a, fast, slow, kept, curved]));

        // A second lane over the first: the sum clips.
        let mut loud = segment("a1", 500_000, 3 * s);
        loud.volume = 2.5;
        let mut eq = segment("a1", 5_000_000, 2 * s);
        eq.extras.push("eq".into());
        project.materials.extras.insert(
            "eq".into(),
            serde_json::json!({ "audio_fx": { "effects": [
                { "id": "r1", "kind": "reverb" },
                { "id": "c1", "kind": "compressor" }
            ] } }),
        );
        project.tracks.push(track(TrackKind::Audio, vec![loud, eq]));

        // A linked pair heard once, and a muted lane heard not at all.
        let group = "link".to_string();
        project.materials.links.insert(group.clone());
        let mut picture = segment("v1", 3 * s, s);
        picture.id = "pic".into();
        picture.extras.push(group.clone());
        let mut sound = segment("v1", 3 * s, s);
        sound.id = "snd".into();
        sound.extras.push(group);
        project.tracks.push(track(TrackKind::Video, vec![picture]));
        project.tracks.push(track(TrackKind::Audio, vec![sound]));
        let mut muted = track(TrackKind::Audio, vec![segment("a1", 0, s)]);
        muted.muted = true;
        project.tracks.push(muted);
        project
    }

    #[test]
    fn the_streamed_mix_is_the_buffered_mix_bit_for_bit() {
        let project = busy_project();
        for channels in [2u16, 1] {
            let reference = reference_mix(&project, &Tones, RATE, channels, true);
            assert!(reference.iter().any(|s| s.abs() > 0.1), "the mix is heard");
            for block in [1_000, 4_801, BLOCK_FRAMES, usize::MAX] {
                let whole = MixRange::whole(&project, RATE);
                let mixed = streamed(&project, &Tones, channels, whole, block);
                assert!(
                    mixed == reference,
                    "{channels} channel(s), blocks of {block}: the stream differs"
                );
            }
            // Ranges, cut exactly as the buffered export cut its slice.
            let duration = project.duration();
            for (start, length) in [
                (0, 1_000_000),
                (1_234_567, 2_000_000),
                (2_600_000, 3_333_333),
                (duration - 500_000, 2_000_000),
                (duration + 1_000_000, 1_000_000),
            ] {
                let range = MixRange::export(&project, RATE, start, length);
                let mixed = streamed(&project, &Tones, channels, range, 4_801);
                let expected =
                    slice_range(reference.clone(), channels as usize, RATE, start, length);
                assert!(
                    mixed == expected,
                    "{channels} channel(s), range {start}+{length}: the stream differs"
                );
            }
        }
        // And unclamped, as a compound clip's mix-down is.
        let reference = reference_mix(&project, &Tones, RATE, 2, false);
        assert!(reference.iter().any(|s| s.abs() > 1.0), "the sum clips");
        let cancel = AtomicBool::new(false);
        let mixed = MixStream::new(
            &project,
            &Tones,
            RATE,
            2,
            MixRange::whole(&project, RATE),
            &cancel,
        )
        .unwrap()
        .unclamped()
        .collect()
        .unwrap();
        assert!(mixed == reference);
    }

    /// The far clip from the audit: a hand-edited file put one at
    /// 9·10¹⁸ µs, and the export of its first two seconds asked for a buffer
    /// of 3.5·10¹⁸ bytes and aborted. The range mix never sizes anything by
    /// the timeline.
    #[test]
    fn a_clip_at_an_absurd_time_does_not_size_the_mix_of_a_short_range() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![
                segment("a1", 0, MICROS_PER_SECOND),
                segment("a1", 9_000_000_000_000_000_000, MICROS_PER_SECOND),
            ],
        ));
        let source = Recording::default();
        let range = MixRange::export(&project, RATE, 0, 2 * MICROS_PER_SECOND);
        let mixed = streamed(&project, &source, 2, range, BLOCK_FRAMES);
        assert_eq!(mixed.len(), frames_for(2 * MICROS_PER_SECOND, RATE) * 2);
        assert_eq!(mixed[0], 1.0);
        assert_eq!(*mixed.last().unwrap(), 0.0);
        // The far clip was never decoded.
        assert_eq!(source.requests.lock().len(), 1);
    }

    /// A range export decodes only the clips under it.
    #[test]
    fn clips_outside_the_range_are_not_decoded() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![
                segment("a1", 0, MICROS_PER_SECOND),
                segment("a1", 5 * MICROS_PER_SECOND, MICROS_PER_SECOND),
            ],
        ));
        let source = Recording::default();
        let range = MixRange::export(&project, RATE, 5 * MICROS_PER_SECOND, MICROS_PER_SECOND);
        let mixed = streamed(&project, &source, 2, range, BLOCK_FRAMES);
        assert!(mixed.iter().all(|s| *s == 1.0));
        let requests = source.requests.lock().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "a1");
    }

    #[test]
    fn the_silent_source_answers_with_the_right_length() {
        let request = AudioRequest {
            material_id: "a1",
            path: "/tmp/a1.wav",
            start: 0,
            duration: MICROS_PER_SECOND,
            sample_rate: RATE,
            channels: 2,
        };
        let samples = SilentAudioSource.samples(&request).unwrap();
        assert_eq!(samples.len(), RATE as usize * 2);
        assert!(samples.iter().all(|s| *s == 0.0));
    }
}
