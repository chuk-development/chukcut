//! The timeline mixer, incremental.
//!
//! `export::audio::mix_timeline` already walks the timeline and sums it, and
//! this is deliberately not a second copy of it: the walk, the gain rules and
//! the "which segments carry sound" question are the same, and where the two
//! agree this module borrows rather than restates — [`FileAudioSource`] in
//! `decode.rs` is the export mixer's own interface, answered for the first
//! time.
//!
//! What cannot be borrowed is the *shape*. The export mixer allocates one
//! buffer the length of the project and adds every segment into its slice of
//! it, which is the right trade when the answer is written to a file in one
//! pass. A preview starts anywhere, is seeked at any moment, and must produce
//! the next few milliseconds *now* — so this mixer is a function from a
//! position to the next `n` sample frames, holding nothing but its decoders.
//!
//! ## Why the source position is computed absolutely
//!
//! Every block recomputes where it sits in the source from the timeline
//! position it was asked for, rather than continuing from where the last block
//! ended. With `speed` in play those two differ: a block-relative resampler
//! accumulates a fraction of a sample per block and a clip at 0.9x drifts
//! audibly inside a minute. Positioning each block from the playhead makes
//! drift impossible and makes a seek no different from a continuation.
//!
//! ## Clipping
//!
//! Summing is what audio does — every live segment contributes — so two clips
//! at full volume exceed full scale, and every sample format below float wraps
//! rather than saturates, which turns a loud passage into static. The sum is
//! soft-limited: linear below the knee so ordinary material is untouched, and
//! asymptotic above it, which is a gentler distortion than a flattened peak
//! and much gentler than a wrap.

use std::collections::HashSet;
use std::sync::Arc;

use crate::modules::project::document::{
    AnimatableProperty, Id, KeyframeTrack, Micros, Project, Segment, TimeRange, TrackKind,
};

use super::clock::{frames_to_micros, micros_to_frames};
use super::decode::{ClipFactory, ClipReader};

/// The mix bus is stereo. Anything else is a device concern, handled on the
/// way into the ring, so the mixer never has to think about a 5.1 card.
pub const MIX_CHANNELS: usize = 2;

/// Above this the sum is compressed rather than passed through.
const LIMIT_KNEE: f32 = 0.7;

/// How many decoders stay open. One per *segment*, not per file: two segments
/// of the same material sit at different source positions, and a single
/// decoder shared between them would seek back and forth once per block.
const MAX_OPEN_READERS: usize = 8;

/// Bounds on `speed`, so a nonsense value cannot make one block ask for an
/// hour of source.
const MIN_SPEED: f64 = 0.01;
const MAX_SPEED: f64 = 16.0;

/// One audio-bearing segment, with everything the mixer needs already
/// resolved. Built once per project snapshot so the hot path never touches the
/// document.
#[derive(Debug, Clone)]
pub struct PlannedSegment {
    pub segment_id: Id,
    pub path: String,
    pub target: TimeRange,
    pub source_start: Micros,
    pub speed: f64,
    /// Segment volume times track volume, already multiplied.
    pub gain: f32,
    pub volume: Option<KeyframeTrack>,
}

/// Which segments of `project` make sound, in the order they will be summed.
///
/// The rules are the export mixer's: a muted track contributes nothing, a
/// hidden one still does (people mute a lane they are comparing against and
/// hide the one they are not looking at), and a video material only counts
/// when its container actually carries an audio stream.
///
/// One rule is newer than the others: a clip whose sound has been given its own
/// linked segment on an audio lane does not also play it here. Without that,
/// importing a file with both streams — which now puts the picture on a video
/// lane and the sound on an audio lane — would mix the same waveform with
/// itself. See `Project::sound_is_on_a_linked_lane`.
pub fn plan(project: &Project) -> Vec<PlannedSegment> {
    let mut planned = Vec::new();
    for track in &project.tracks {
        if track.muted || !track_bears_audio(track.kind) {
            continue;
        }
        for segment in &track.segments {
            if segment.target_range.duration <= 0 {
                continue;
            }
            if project.sound_is_on_a_linked_lane(track, segment) {
                continue;
            }
            let Some(path) = audio_path(project, segment) else {
                continue;
            };
            planned.push(PlannedSegment {
                segment_id: segment.id.clone(),
                path: path.to_string(),
                target: segment.target_range,
                source_start: segment.source_range.start.max(0),
                speed: sane_speed(segment.speed),
                gain: finite_or(segment.volume, 1.0) * finite_or(track.volume, 1.0),
                volume: segment
                    .keyframes
                    .iter()
                    .find(|k| k.property == AnimatableProperty::Volume)
                    .cloned(),
            });
        }
    }
    planned
}

fn track_bears_audio(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Audio | TrackKind::Video)
}

fn audio_path<'a>(project: &'a Project, segment: &Segment) -> Option<&'a str> {
    if let Some(audio) = project.materials.audio(&segment.material_id) {
        return Some(&audio.path);
    }
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

fn sane_speed(speed: f32) -> f64 {
    let speed = speed as f64;
    if speed.is_finite() && speed > 0.0 {
        speed.clamp(MIN_SPEED, MAX_SPEED)
    } else {
        1.0
    }
}

/// Keep a sum inside full scale without flattening its peaks.
///
/// Identity below the knee and continuous in value *and* slope at it — a
/// limiter with a corner in it is audible as a buzz on sustained material.
pub fn soft_limit(sample: f32) -> f32 {
    if !sample.is_finite() {
        // One NaN from a broken float source would otherwise poison every
        // conversion downstream of here.
        return 0.0;
    }
    let magnitude = sample.abs();
    if magnitude <= LIMIT_KNEE {
        return sample;
    }
    let headroom = 1.0 - LIMIT_KNEE;
    let over = (magnitude - LIMIT_KNEE) / headroom;
    (LIMIT_KNEE + headroom * over.tanh()) * sample.signum()
}

/// An open decoder and when it was last used.
struct OpenReader {
    segment_id: Id,
    reader: Box<dyn ClipReader>,
    used: u64,
}

/// Produces the next `n` sample frames of the timeline from any position.
pub struct TimelineMixer {
    rate: u32,
    plan: Vec<PlannedSegment>,
    factory: Arc<dyn ClipFactory>,
    readers: Vec<OpenReader>,
    /// Segments whose file could not be opened or read. Kept so one broken
    /// clip costs one log line rather than one per block.
    broken: HashSet<Id>,
    /// Monotonic counter standing in for a timestamp on the reader cache.
    tick: u64,
    /// Decoded source samples for the segment being mixed. Reused so a block
    /// does not allocate.
    source: Vec<f32>,
    master: f32,
}

impl TimelineMixer {
    pub fn new(rate: u32, factory: Arc<dyn ClipFactory>) -> Self {
        Self {
            rate: rate.max(1),
            plan: Vec::new(),
            factory,
            readers: Vec::new(),
            broken: HashSet::new(),
            tick: 0,
            source: Vec::new(),
            master: 1.0,
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn master(&self) -> f32 {
        self.master
    }

    /// Master volume, applied before the limiter so turning it down actually
    /// avoids clipping rather than merely quietening a clipped signal.
    pub fn set_master(&mut self, volume: f32) {
        self.master = if volume.is_finite() {
            volume.clamp(0.0, 4.0)
        } else {
            1.0
        };
    }

    /// Adopt a new project snapshot. Decoders for segments that survived the
    /// edit are kept — a trim should not cost a reopen of every file.
    pub fn set_plan(&mut self, plan: Vec<PlannedSegment>) {
        let live: HashSet<&Id> = plan.iter().map(|p| &p.segment_id).collect();
        self.readers.retain(|open| live.contains(&open.segment_id));
        self.broken.clear();
        self.plan = plan;
    }

    pub fn plan(&self) -> &[PlannedSegment] {
        &self.plan
    }

    /// Whether anything at all would be heard. The engine uses this to leave
    /// the device idle on a project with no sound in it.
    pub fn is_silent(&self) -> bool {
        self.plan.is_empty()
    }

    /// Write the `out.len() / 2` sample frames that start at timeline frame
    /// `from_frame`.
    ///
    /// Always writes every value: silence where nothing is live, which is what
    /// makes a gap in the timeline a gap in the sound rather than whatever was
    /// in the buffer last.
    pub fn fill(&mut self, from_frame: i64, out: &mut [f32]) {
        for sample in out.iter_mut() {
            *sample = 0.0;
        }
        let frames = out.len() / MIX_CHANNELS;
        if frames == 0 {
            return;
        }
        let window_end = from_frame + frames as i64;

        for index in 0..self.plan.len() {
            self.mix_segment(index, from_frame, window_end, out);
        }

        if (self.master - 1.0).abs() > f32::EPSILON {
            for sample in out.iter_mut() {
                *sample *= self.master;
            }
        }
        for sample in out.iter_mut() {
            *sample = soft_limit(*sample);
        }
    }

    fn mix_segment(&mut self, index: usize, from_frame: i64, window_end: i64, out: &mut [f32]) {
        let planned = self.plan[index].clone();
        if self.broken.contains(&planned.segment_id) {
            return;
        }

        // Segment boundaries are converted to frames once, from the same
        // rounding the playhead uses, so the first and last sample of a
        // segment land in exactly one block and never in both or neither.
        let segment_start = micros_to_frames(planned.target.start, self.rate);
        let segment_end = micros_to_frames(planned.target.end(), self.rate);
        let start = from_frame.max(segment_start);
        let end = window_end.min(segment_end);
        if end <= start {
            return;
        }
        let count = (end - start) as usize;

        // Where in the source the first of those frames comes from, and how
        // fast we walk through it.
        let source_base = micros_to_frames(planned.source_start, self.rate) as f64
            + (start - segment_start) as f64 * planned.speed;
        let source_span = (count.saturating_sub(1)) as f64 * planned.speed;
        let first_source = source_base.floor() as i64;
        // One extra frame for the interpolation partner of the last sample.
        let source_frames =
            ((source_base + source_span).floor() as i64 - first_source + 2) as usize;

        self.source.clear();
        self.source.resize(source_frames * MIX_CHANNELS, 0.0);
        if let Err(error) = self.read_source(&planned, first_source, source_frames) {
            tracing::warn!(
                segment = %planned.segment_id,
                path = %planned.path,
                %error,
                "audio for this segment cannot be read; it will be silent"
            );
            self.broken.insert(planned.segment_id.clone());
            return;
        }

        let has_keyframes = planned.volume.is_some();
        for i in 0..count {
            let position = source_base + i as f64 * planned.speed - first_source as f64;
            let lower = position.floor();
            let fraction = (position - lower) as f32;
            let lower = lower.max(0.0) as usize;
            let upper = (lower + 1).min(source_frames.saturating_sub(1));
            let lower = lower.min(source_frames.saturating_sub(1));

            let mut gain = planned.gain;
            if has_keyframes {
                // Keyframe times are relative to the segment start, so the
                // offset is the frame's position inside the segment and not on
                // the timeline.
                let offset = frames_to_micros((start + i as i64 - segment_start) as u64, self.rate);
                if let Some(value) = planned
                    .volume
                    .as_ref()
                    .and_then(|track| track.sample(offset))
                {
                    gain *= finite_or(value, 1.0);
                }
            }

            let destination = (start + i as i64 - from_frame) as usize * MIX_CHANNELS;
            for channel in 0..MIX_CHANNELS {
                let a = self.source[lower * MIX_CHANNELS + channel];
                let b = self.source[upper * MIX_CHANNELS + channel];
                out[destination + channel] += (a + (b - a) * fraction) * gain;
            }
        }
    }

    /// Decode `frames` sample frames of `planned`'s file starting at
    /// `at_frame`, opening the decoder if it is not already open.
    fn read_source(
        &mut self,
        planned: &PlannedSegment,
        at_frame: i64,
        frames: usize,
    ) -> super::Result<()> {
        self.tick += 1;
        let tick = self.tick;

        let existing = self
            .readers
            .iter()
            .position(|open| open.segment_id == planned.segment_id);
        let slot = match existing {
            Some(slot) => slot,
            None => {
                let reader = self
                    .factory
                    .open(&planned.path, self.rate, MIX_CHANNELS as u16)?;
                if self.readers.len() >= MAX_OPEN_READERS {
                    // Evict the one that has gone longest without being asked
                    // for anything: during playback that is a segment the
                    // playhead has left behind.
                    if let Some(oldest) = self
                        .readers
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, open)| open.used)
                        .map(|(index, _)| index)
                    {
                        self.readers.remove(oldest);
                    }
                }
                self.readers.push(OpenReader {
                    segment_id: planned.segment_id.clone(),
                    reader,
                    used: tick,
                });
                self.readers.len() - 1
            }
        };

        self.readers[slot].used = tick;
        self.readers[slot]
            .reader
            .read(at_frame, frames, &mut self.source)
    }
}

impl std::fmt::Debug for TimelineMixer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimelineMixer")
            .field("rate", &self.rate)
            .field("segments", &self.plan.len())
            .field("open_readers", &self.readers.len())
            .field("master", &self.master)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        AudioMaterial, CanvasConfig, Easing, Keyframe, Track, Transform, VideoMaterial,
        MICROS_PER_SECOND,
    };
    use parking_lot::Mutex;

    const RATE: u32 = 48_000;

    /// A reader that answers with a constant, so a sample's value says which
    /// clip it came from.
    struct Constant(f32);

    impl ClipReader for Constant {
        fn read(&mut self, _at: i64, frames: usize, out: &mut [f32]) -> super::super::Result<()> {
            for sample in out.iter_mut().take(frames * MIX_CHANNELS) {
                *sample = self.0;
            }
            Ok(())
        }
    }

    /// How far a `Position` sample is scaled down from the frame index it
    /// encodes. Kept well below the limiter's knee so these tests measure
    /// where the samples came from and not what the limiter did to them.
    const POSITION_SCALE: f32 = 1e-6;

    /// A reader whose sample value *is* its position in the file, which turns
    /// "did we read the right part of the source" into an equality check.
    struct Position;

    impl ClipReader for Position {
        fn read(&mut self, at: i64, frames: usize, out: &mut [f32]) -> super::super::Result<()> {
            for frame in 0..frames {
                for channel in 0..MIX_CHANNELS {
                    out[frame * MIX_CHANNELS + channel] =
                        (at + frame as i64) as f32 * POSITION_SCALE;
                }
            }
            Ok(())
        }
    }

    /// The source frame index a mixed sample came from.
    fn source_frame(sample: f32) -> f32 {
        sample / POSITION_SCALE
    }

    #[derive(Debug)]
    struct Factory {
        kind: Kind,
        opened: Mutex<Vec<String>>,
    }

    #[derive(Debug, Clone, Copy)]
    enum Kind {
        Constant(f32),
        Position,
        Broken,
    }

    impl Factory {
        fn new(kind: Kind) -> Arc<Self> {
            Arc::new(Self {
                kind,
                opened: Mutex::new(Vec::new()),
            })
        }
    }

    impl ClipFactory for Factory {
        fn open(
            &self,
            path: &str,
            _rate: u32,
            _channels: u16,
        ) -> super::super::Result<Box<dyn ClipReader>> {
            self.opened.lock().push(path.to_string());
            match self.kind {
                Kind::Constant(value) => Ok(Box::new(Constant(value))),
                Kind::Position => Ok(Box::new(Position)),
                Kind::Broken => Err(super::super::AudioError::Invalid("no such file".into())),
            }
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

    fn mixer(project: &Project, kind: Kind) -> (TimelineMixer, Arc<Factory>) {
        let factory = Factory::new(kind);
        let mut mixer = TimelineMixer::new(RATE, factory.clone());
        mixer.set_plan(plan(project));
        (mixer, factory)
    }

    fn block(frames: usize) -> Vec<f32> {
        vec![0.0; frames * MIX_CHANNELS]
    }

    // -----------------------------------------------------------------------
    // The plan
    // -----------------------------------------------------------------------

    #[test]
    fn a_muted_track_is_not_in_the_plan_at_all() {
        let mut project = project();
        let mut lane = track(TrackKind::Audio, vec![segment("a1", 0, MICROS_PER_SECOND)]);
        lane.muted = true;
        project.tracks.push(lane);
        assert!(plan(&project).is_empty());
    }

    #[test]
    fn a_hidden_track_is_still_heard() {
        let mut project = project();
        let mut lane = track(TrackKind::Video, vec![segment("v1", 0, MICROS_PER_SECOND)]);
        lane.hidden = true;
        project.tracks.push(lane);
        assert_eq!(plan(&project).len(), 1);
    }

    #[test]
    fn a_clip_whose_sound_has_its_own_linked_lane_is_heard_exactly_once() {
        // What an import of a file with both streams produces. Both segments
        // name the same video material, and the mixer's ordinary rule — "a
        // video material whose container carries audio makes sound, wherever it
        // sits" — would plan both and sum the same waveform with itself.
        let mut project = project();
        let mut picture = segment("v1", 0, MICROS_PER_SECOND);
        picture.id = "picture".into();
        picture.extras.push("group-1".into());
        let mut sound = segment("v1", 0, MICROS_PER_SECOND);
        sound.id = "sound".into();
        sound.extras.push("group-1".into());
        project.materials.links.insert("group-1".into());
        project.tracks.push(track(TrackKind::Video, vec![picture]));
        project.tracks.push(track(TrackKind::Audio, vec![sound]));

        let planned = plan(&project);
        assert_eq!(planned.len(), 1, "the file plays once, not twice");
        assert_eq!(
            planned[0].segment_id, "sound",
            "and it is the clip on the audio lane that plays it"
        );

        // Unlink and both are heard again, because they are now two ordinary
        // clips that happen to share a file.
        project.materials.links.clear();
        assert_eq!(plan(&project).len(), 2);
    }

    #[test]
    fn a_video_without_an_audio_stream_is_skipped() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Video,
            vec![segment("silent", 0, MICROS_PER_SECOND)],
        ));
        assert!(plan(&project).is_empty());
    }

    #[test]
    fn track_and_segment_volume_are_multiplied_into_one_gain() {
        let mut project = project();
        let mut seg = segment("a1", 0, MICROS_PER_SECOND);
        seg.volume = 0.5;
        let mut lane = track(TrackKind::Audio, vec![seg]);
        lane.volume = 0.25;
        project.tracks.push(lane);

        let planned = plan(&project);
        assert!((planned[0].gain - 0.125).abs() < 1e-6);
    }

    #[test]
    fn a_nonsense_speed_falls_back_to_real_time() {
        assert_eq!(sane_speed(f32::NAN), 1.0);
        assert_eq!(sane_speed(0.0), 1.0);
        assert_eq!(sane_speed(-2.0), 1.0);
        assert_eq!(sane_speed(2.0), 2.0);
        assert_eq!(
            sane_speed(1_000.0),
            MAX_SPEED,
            "one block cannot ask for an hour"
        );
    }

    // -----------------------------------------------------------------------
    // Boundaries
    // -----------------------------------------------------------------------

    #[test]
    fn a_segment_starts_and_stops_on_the_exact_sample() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", MICROS_PER_SECOND, MICROS_PER_SECOND)],
        ));
        let (mut mixer, _) = mixer(&project, Kind::Constant(0.5));

        // A block straddling the segment start: silent up to it, loud from it.
        let start = RATE as i64; // one second in
        let mut out = block(8);
        mixer.fill(start - 4, &mut out);
        assert_eq!(&out[..8], &[0.0; 8], "audio before the segment starts");
        assert!(out[8..].iter().all(|s| (*s - 0.5).abs() < 1e-6));

        // And the last sample frame of the segment is the one before its end.
        let end = 2 * RATE as i64;
        let mut out = block(8);
        mixer.fill(end - 4, &mut out);
        assert!(out[..8].iter().all(|s| (*s - 0.5).abs() < 1e-6));
        assert_eq!(&out[8..], &[0.0; 8], "the end of a range is exclusive");
    }

    #[test]
    fn a_gap_between_segments_is_silence_and_not_stale_buffer() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![
                segment("a1", 0, MICROS_PER_SECOND),
                segment("a1", 2 * MICROS_PER_SECOND, MICROS_PER_SECOND),
            ],
        ));
        let (mut mixer, _) = mixer(&project, Kind::Constant(0.5));

        let mut out = block(64);
        mixer.fill(0, &mut out);
        assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6));

        // Reusing the same buffer, one and a half seconds in.
        mixer.fill(3 * RATE as i64 / 2, &mut out);
        assert!(out.iter().all(|s| *s == 0.0), "the gap must be silent");
    }

    #[test]
    fn a_block_before_the_project_starts_is_silent() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", 0, MICROS_PER_SECOND)],
        ));
        let (mut mixer, _) = mixer(&project, Kind::Constant(1.0));

        let mut out = block(16);
        mixer.fill(-64, &mut out);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    // -----------------------------------------------------------------------
    // Seeking and source positions
    // -----------------------------------------------------------------------

    #[test]
    fn a_seek_into_the_middle_of_a_segment_reads_from_the_matching_source() {
        let mut project = project();
        let mut seg = segment("a1", MICROS_PER_SECOND, 4 * MICROS_PER_SECOND);
        // The segment is trimmed: its first sample is half a second into the
        // file.
        seg.source_range = TimeRange::new(MICROS_PER_SECOND / 2, 4 * MICROS_PER_SECOND);
        project.tracks.push(track(TrackKind::Audio, vec![seg]));
        let (mut mixer, _) = mixer(&project, Kind::Position);

        // Land two seconds into the timeline: one second into the segment, so
        // one and a half seconds into the file.
        let mut out = block(4);
        mixer.fill(2 * RATE as i64, &mut out);
        let expected = 1.5 * RATE as f32;
        let landed = source_frame(out[0]);
        assert!((landed - expected).abs() < 1.0, "{landed} != {expected}");
        assert!((source_frame(out[2]) - (expected + 1.0)).abs() < 1.0);
    }

    #[test]
    fn consecutive_blocks_join_without_a_gap_or_a_repeat() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", 0, 4 * MICROS_PER_SECOND)],
        ));
        let (mut mixer, _) = mixer(&project, Kind::Position);

        let mut first = block(512);
        let mut second = block(512);
        mixer.fill(1_000, &mut first);
        mixer.fill(1_512, &mut second);

        let last_of_first = source_frame(first[511 * MIX_CHANNELS]);
        let first_of_second = source_frame(second[0]);
        assert!(
            (first_of_second - last_of_first - 1.0).abs() < 1e-3,
            "block boundary jumped: {last_of_first} then {first_of_second}"
        );
    }

    #[test]
    fn speed_walks_through_the_source_faster_without_moving_the_segment() {
        let mut project = project();
        let mut seg = segment("a1", 0, 2 * MICROS_PER_SECOND);
        seg.speed = 2.0;
        seg.source_range = TimeRange::new(0, 4 * MICROS_PER_SECOND);
        project.tracks.push(track(TrackKind::Audio, vec![seg]));
        let (mut mixer, _) = mixer(&project, Kind::Position);

        let mut out = block(4);
        mixer.fill(0, &mut out);
        assert!(source_frame(out[0]).abs() < 1e-2);
        assert!(
            (source_frame(out[2]) - 2.0).abs() < 1e-2,
            "one output frame is two source frames"
        );

        // Half way through the segment is a whole second into the source.
        mixer.fill(RATE as i64 / 2, &mut out);
        assert!((source_frame(out[0]) - RATE as f32).abs() < 1.0);
    }

    #[test]
    fn half_speed_interpolates_between_source_frames() {
        let mut project = project();
        let mut seg = segment("a1", 0, 2 * MICROS_PER_SECOND);
        seg.speed = 0.5;
        project.tracks.push(track(TrackKind::Audio, vec![seg]));
        let (mut mixer, _) = mixer(&project, Kind::Position);

        let mut out = block(4);
        mixer.fill(0, &mut out);
        assert!(source_frame(out[0]).abs() < 1e-2);
        assert!(
            (source_frame(out[2]) - 0.5).abs() < 1e-2,
            "the midpoint of two source frames"
        );
        assert!((source_frame(out[4]) - 1.0).abs() < 1e-2);
    }

    // -----------------------------------------------------------------------
    // Gain
    // -----------------------------------------------------------------------

    #[test]
    fn gains_compose_from_segment_track_and_master() {
        let mut project = project();
        let mut seg = segment("a1", 0, MICROS_PER_SECOND);
        seg.volume = 0.5;
        let mut lane = track(TrackKind::Audio, vec![seg]);
        lane.volume = 0.5;
        project.tracks.push(lane);

        let (mut mixer, _) = mixer(&project, Kind::Constant(1.0));
        let mut out = block(4);
        mixer.fill(0, &mut out);
        assert!((out[0] - 0.25).abs() < 1e-6);

        mixer.set_master(0.5);
        mixer.fill(0, &mut out);
        assert!((out[0] - 0.125).abs() < 1e-6);
    }

    #[test]
    fn a_volume_keyframe_fades_across_the_segment() {
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
        let (mut mixer, _) = mixer(&project, Kind::Constant(0.5));

        let mut out = block(2);
        mixer.fill(0, &mut out);
        assert!(out[0].abs() < 1e-3, "the fade starts at silence");

        mixer.fill(RATE as i64 / 2, &mut out);
        assert!(
            (out[0] - 0.25).abs() < 0.01,
            "half way through is half volume"
        );

        mixer.fill(RATE as i64 - 2, &mut out);
        assert!(out[0] > 0.49);
    }

    #[test]
    fn the_master_volume_is_clamped_to_something_sane() {
        let (mut mixer, _) = mixer(&project(), Kind::Constant(1.0));
        mixer.set_master(f32::NAN);
        assert_eq!(mixer.master(), 1.0);
        mixer.set_master(-1.0);
        assert_eq!(mixer.master(), 0.0);
        mixer.set_master(100.0);
        assert_eq!(mixer.master(), 4.0);
    }

    // -----------------------------------------------------------------------
    // Summing
    // -----------------------------------------------------------------------

    #[test]
    fn overlapping_segments_sum() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", 0, MICROS_PER_SECOND)],
        ));
        let mut other = track(TrackKind::Audio, vec![segment("v1", 0, MICROS_PER_SECOND)]);
        other.volume = 1.0;
        project.tracks.push(other);

        let (mut mixer, _) = mixer(&project, Kind::Constant(0.25));
        let mut out = block(4);
        mixer.fill(0, &mut out);
        assert!((out[0] - 0.5).abs() < 1e-6, "two lanes at 0.25 make 0.5");
    }

    #[test]
    fn two_clips_at_full_volume_do_not_leave_full_scale() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", 0, MICROS_PER_SECOND)],
        ));
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("v1", 0, MICROS_PER_SECOND)],
        ));

        let (mut mixer, _) = mixer(&project, Kind::Constant(1.0));
        let mut out = block(16);
        mixer.fill(0, &mut out);
        assert!(
            out.iter().all(|s| (-1.0..=1.0).contains(s)),
            "the sum was {} and would wrap in any integer format",
            out[0]
        );
        assert!(
            out[0] > 0.9,
            "and it is still loud, not attenuated to nothing"
        );
    }

    #[test]
    fn the_limiter_is_transparent_below_the_knee_and_asymptotic_above_it() {
        assert_eq!(soft_limit(0.0), 0.0);
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.5), -0.5);
        assert_eq!(soft_limit(LIMIT_KNEE), LIMIT_KNEE);
        assert!(soft_limit(1.0) < 1.0 && soft_limit(1.0) > LIMIT_KNEE);
        // Full scale is the asymptote, so a wildly hot sum lands on it and
        // never past it — which is all any integer format needs.
        assert!(soft_limit(10.0) <= 1.0);
        assert!(soft_limit(1e9) <= 1.0);
        assert!(soft_limit(-10.0) >= -1.0);
        assert_eq!(soft_limit(f32::NAN), 0.0);
        assert_eq!(soft_limit(f32::INFINITY), 0.0);

        // Continuous at the knee: a corner there is audible as a buzz.
        let below = soft_limit(LIMIT_KNEE - 1e-4);
        let above = soft_limit(LIMIT_KNEE + 1e-4);
        assert!((above - below).abs() < 1e-3);
        // And monotonic, so louder input is never quieter output.
        let mut previous = 0.0;
        for step in 0..200 {
            let value = soft_limit(step as f32 * 0.05);
            assert!(value >= previous, "not monotonic at {step}");
            previous = value;
        }
    }

    // -----------------------------------------------------------------------
    // Failure
    // -----------------------------------------------------------------------

    #[test]
    fn a_file_that_cannot_be_opened_is_silent_rather_than_fatal() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", 0, MICROS_PER_SECOND)],
        ));
        let (mut mixer, factory) = mixer(&project, Kind::Broken);

        let mut out = block(8);
        mixer.fill(0, &mut out);
        assert!(out.iter().all(|s| *s == 0.0));

        // And it is not retried on every block, which at 100 blocks a second
        // would be a hundred log lines and a hundred failed opens.
        mixer.fill(8, &mut out);
        mixer.fill(16, &mut out);
        assert_eq!(factory.opened.lock().len(), 1);
    }

    #[test]
    fn a_decoder_is_opened_once_and_reused_across_blocks() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", 0, 4 * MICROS_PER_SECOND)],
        ));
        let (mut mixer, factory) = mixer(&project, Kind::Constant(1.0));

        let mut out = block(256);
        for block_index in 0..20 {
            mixer.fill(block_index * 256, &mut out);
        }
        assert_eq!(
            factory.opened.lock().len(),
            1,
            "a decoder per block is what makes playback a slideshow"
        );
    }

    #[test]
    fn re_planning_keeps_the_decoders_of_segments_that_survived() {
        let mut project = project();
        project.tracks.push(track(
            TrackKind::Audio,
            vec![segment("a1", 0, 4 * MICROS_PER_SECOND)],
        ));
        let (mut mixer, factory) = mixer(&project, Kind::Constant(1.0));

        let mut out = block(64);
        mixer.fill(0, &mut out);
        assert_eq!(factory.opened.lock().len(), 1);

        // The same segment, trimmed. Nothing about the file changed.
        let mut trimmed = project.clone();
        trimmed.tracks[0].segments[0].target_range = TimeRange::new(0, 3 * MICROS_PER_SECOND);
        mixer.set_plan(plan(&trimmed));
        mixer.fill(0, &mut out);
        assert_eq!(
            factory.opened.lock().len(),
            1,
            "a trim must not reopen the file"
        );
    }
}
