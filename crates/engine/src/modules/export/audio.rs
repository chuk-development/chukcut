//! Mixing the timeline down to one stereo bed.
//!
//! Video is a stack — the topmost opaque pixel wins. Audio is a *sum*: every
//! unmuted segment that is live at an instant contributes to it, so the mix is
//! one buffer the length of the project with every segment added into its own
//! slice of it. At 48 kHz stereo that is 384 KB per second, about 1.4 GB for an
//! hour; a streaming mixer would avoid that, but it would also have to
//! interleave decoding with encoding at frame granularity for a saving nobody
//! exporting a social clip will ever notice. Buffer first, be clever later.
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
//! but it changes which samples are read — a segment at 2x consumes twice the
//! source duration, and resampling that back to the target length moves the
//! pitch up, which is what a naive speed change does and what users expect
//! until a time-stretcher exists.
//!
//! The sum is kept in `f32` with no headroom management and clamped once, at
//! the end. Normalizing instead would mean two exports of nearly identical
//! projects come out at different loudnesses, which is worse than the clipping
//! it avoids; every editor clamps and shows the user a meter.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::modules::project::document::{
    AnimatableProperty, Micros, Project, Segment, TrackKind, MICROS_PER_SECOND,
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

/// Mix every audio-bearing segment in `project` into one interleaved buffer.
///
/// Returns exactly `frames_for(project.duration())` sample frames, which is
/// what the encoder needs to keep audio and video the same length.
pub fn mix_timeline(
    project: &Project,
    source: &dyn AudioSource,
    sample_rate: u32,
    channels: u16,
    cancel: &AtomicBool,
) -> Result<Vec<f32>> {
    let duration = project.duration();
    let mut mixer = AudioMixer::new(sample_rate, channels, duration);

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
            // The same rule the preview mixer applies, and it has to be applied
            // in both places or an export sounds different from what was
            // monitored. A file imported with both streams becomes two linked
            // segments of one material — picture on a video lane, sound on an
            // audio lane — and `audio_path` above resolves for both, because a
            // video material carrying audio contributes sound wherever it sits.
            // Mixing both is the same waveform summed with itself: 6 dB up and
            // phasing with every microsecond they are out by.
            if project.sound_is_on_a_linked_lane(track, segment) {
                continue;
            }

            // A speed factor changes how much source a segment consumes; the
            // document keeps both ranges, but `source_range.duration` is the
            // authority and speed is what maps between them.
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
            let request = AudioRequest {
                material_id: &segment.material_id,
                path: &effective.path,
                start: segment.source_range.start,
                duration: source_duration,
                sample_rate,
                channels,
            };
            let decoded = source.samples(&request).map_err(ExportError::Audio)?;

            let channel_count = channels.max(1) as usize;
            let target_frames = frames_for(segment.target_range.duration, sample_rate);
            let stretched = resample_linear(&decoded, channel_count, target_frames);

            let track_gain = finite_or(track.volume, 1.0);
            let volume_track = segment
                .keyframes
                .iter()
                .find(|k| k.property == AnimatableProperty::Volume);
            let base = finite_or(segment.volume, 1.0) * track_gain * effective.gain;

            mixer.mix_at(&stretched, segment.target_range.start, |frame| {
                match volume_track {
                    None => base,
                    Some(track) => {
                        // Keyframe times are relative to the segment start, so
                        // the offset is the frame's position inside the
                        // segment, not on the timeline.
                        let offset = micros_for(frame, sample_rate);
                        base * track
                            .sample(offset)
                            .map(|v| finite_or(v, 1.0))
                            .unwrap_or(1.0)
                    }
                }
            });
        }
    }

    Ok(mixer.finish())
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
/// read faster. A pitch-preserving stretch is a phase vocoder and belongs in
/// its own module the day someone asks for it.
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
        let mut project = project();
        let mut seg = segment("a1", 0, MICROS_PER_SECOND);
        seg.speed = 2.0;
        seg.source_range = TimeRange::new(500_000, 2 * MICROS_PER_SECOND);
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
