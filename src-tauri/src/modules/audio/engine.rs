//! Transport, and the thread that keeps the device fed.
//!
//! The engine is the only thing in this module with a lifetime longer than a
//! call. It owns the device, the mixer and the producing end of the ring, and
//! it exists to answer five questions — play, pause, seek, set the volume,
//! stop — while a real-time callback several threads away keeps its deadline.
//!
//! ## One thread does all the work
//!
//! Opening and losing the device, rebuilding the mix plan after an edit,
//! reacting to a seek, and filling the ring all happen on the same thread. Not
//! for simplicity: the mixer holds FFmpeg decoders, which are `Send` and not
//! `Sync`, and the producing end of a single-producer ring is only single if
//! exactly one thread ever touches it. Commands therefore do not *do*
//! anything; they publish an intention in an atomic and wake the thread, which
//! is also why every one of them returns immediately and none of them can
//! block the UI.
//!
//! ## Why a pause flushes
//!
//! Because the ring holds up to a tenth of a second of audio and the user
//! pressing pause expects silence, not the rest of the buffer. The same
//! applies, more sharply, to a seek: audio from where the playhead used to be
//! is far more noticeable than a stale frame, which is why the flush is the
//! first thing that happens and the refill the second.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde::Serialize;

use super::clock::{micros_to_frames, AudioTimeSource, DeviceClock};
use super::decode::{ClipFactory, FileClipFactory};
use super::device::{map_channels, AudioOutput};
use super::mixer::{plan, TimelineMixer, MIX_CHANNELS};
use super::ring::RingProducer;
use crate::modules::preview::clock::TimeSource;
use crate::modules::project::document::{Micros, Project};

/// Sample frames mixed per pass. One block is about 10 ms at 48 kHz: short
/// enough that a seek is acted on promptly, long enough that the per-block
/// overhead disappears.
const BLOCK_FRAMES: usize = 512;

/// How long the fill thread sleeps when there is nothing to do. Also the
/// longest a command can go unnoticed, so it is short.
const IDLE_TICK: Duration = Duration::from_millis(2);

/// How long to wait before trying a device that would not open again.
const REOPEN_DELAY: Duration = Duration::from_secs(2);

/// What the frontend is told about the audio path.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStatus {
    /// Whether a device is open. When false the editor still plays, silently,
    /// and `error` says why.
    pub available: bool,
    pub device: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_frames: Option<u32>,
    /// The device's reported output latency, which the playhead is corrected
    /// by.
    pub latency: Micros,
    /// Whether the playhead is following the device rather than wall time.
    pub clock_master: bool,
    pub volume: f32,
    pub playing: bool,
    /// Why there is no sound, in prose, when there is none.
    pub error: Option<String>,
}

/// Everything the transport and the fill thread share.
struct Shared {
    clock: Arc<DeviceClock>,
    factory: Arc<dyn ClipFactory>,

    /// Whether the fill thread should be producing audio.
    playing: AtomicBool,
    /// Whether a device should be open at all.
    wanted: AtomicBool,
    shutdown: AtomicBool,

    /// Master volume as `f32` bits.
    volume: AtomicU32,

    /// Bumped by every seek; the fill thread compares it against what it has
    /// acted on. A counter rather than a flag so two seeks in a row cannot
    /// collapse into one.
    seek_generation: AtomicU64,
    seek_target: AtomicI64,

    project_generation: AtomicU64,
    project: Mutex<Option<Arc<Project>>>,

    /// Kept here only so the stream stays alive and `status` can describe it.
    output: Mutex<Option<AudioOutput>>,
    error: Mutex<Option<String>>,

    wake_lock: Mutex<()>,
    wake: Condvar,
}

impl Shared {
    fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    fn wake(&self) {
        self.wake.notify_all();
    }
}

/// The audio path, from the project document to the sound card.
pub struct AudioEngine {
    shared: Arc<Shared>,
    time: Arc<AudioTimeSource>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl AudioEngine {
    /// Start the engine. Never fails: a machine with no sound card gets an
    /// engine that reports itself unavailable and a clock that runs on wall
    /// time, and the editor is fully usable.
    pub fn new() -> Arc<Self> {
        Self::with_factory(Arc::new(FileClipFactory))
    }

    /// An engine that decodes through `factory`. The seam the tests use.
    pub fn with_factory(factory: Arc<dyn ClipFactory>) -> Arc<Self> {
        let clock = Arc::new(DeviceClock::new());
        let shared = Arc::new(Shared {
            clock: Arc::clone(&clock),
            factory,
            playing: AtomicBool::new(false),
            wanted: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            volume: AtomicU32::new(1.0f32.to_bits()),
            seek_generation: AtomicU64::new(0),
            seek_target: AtomicI64::new(0),
            project_generation: AtomicU64::new(0),
            project: Mutex::new(None),
            output: Mutex::new(None),
            error: Mutex::new(None),
            wake_lock: Mutex::new(()),
            wake: Condvar::new(),
        });

        let engine = Arc::new(Self {
            shared: Arc::clone(&shared),
            time: Arc::new(AudioTimeSource::new(clock)),
            thread: Mutex::new(None),
        });

        let thread = std::thread::Builder::new()
            .name("chukcut-audio-mixer".into())
            .spawn(move || fill_loop(shared))
            .expect("spawn audio mixer thread");
        *engine.thread.lock() = Some(thread);
        engine
    }

    /// The clock the preview should run on.
    ///
    /// This is the whole point of the module from the preview's side: hand
    /// this to `PreviewServer::with_time_source` and the picture follows the
    /// sound instead of a wall clock that agrees with it only approximately.
    pub fn time_source(&self) -> Arc<dyn TimeSource> {
        Arc::clone(&self.time) as Arc<dyn TimeSource>
    }

    /// Adopt a project snapshot. Cheap, and safe to call on every edit: the
    /// decoders of segments that survived the edit are kept open.
    pub fn set_project(&self, project: Arc<Project>) {
        *self.shared.project.lock() = Some(project);
        self.shared.project_generation.fetch_add(1, Ordering::Release);
        self.shared.wanted.store(true, Ordering::Relaxed);
        self.shared.wake();
    }

    /// Start playing from `from`.
    ///
    /// Takes the position rather than remembering one, because the playhead
    /// lives in the preview clock and two copies of it would eventually
    /// disagree.
    pub fn play(&self, from: Micros) {
        self.shared.wanted.store(true, Ordering::Relaxed);
        self.request_position(from);
        self.shared.playing.store(true, Ordering::Relaxed);
        self.shared.wake();
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        self.shared.wake();
    }

    /// Move the playhead. The ring is discarded, so nothing from the old
    /// position is heard after this returns.
    pub fn seek(&self, to: Micros) {
        self.request_position(to);
        self.shared.wake();
    }

    /// Close the session: stop, flush, and let go of the device.
    pub fn stop(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        self.shared.wanted.store(false, Ordering::Relaxed);
        *self.shared.project.lock() = None;
        self.shared.project_generation.fetch_add(1, Ordering::Release);
        self.shared.wake();
    }

    /// Master volume, `0.0` to `4.0`. Applied before the limiter, so turning
    /// it down really does avoid clipping.
    pub fn set_volume(&self, volume: f32) {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 4.0)
        } else {
            1.0
        };
        self.shared.volume.store(volume.to_bits(), Ordering::Relaxed);
        self.shared.wake();
    }

    pub fn volume(&self) -> f32 {
        self.shared.volume()
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    /// Whether the sound card is driving the playhead.
    pub fn is_clock_master(&self) -> bool {
        self.time.is_device_master()
    }

    pub fn status(&self) -> AudioStatus {
        let output = self.shared.output.lock();
        let info = output.as_ref().map(|output| output.info().clone());
        drop(output);

        AudioStatus {
            available: info.is_some(),
            device: info.as_ref().map(|info| info.name.clone()),
            sample_rate: info.as_ref().map(|info| info.sample_rate).unwrap_or(0),
            channels: info.as_ref().map(|info| info.channels).unwrap_or(0),
            buffer_frames: info.as_ref().and_then(|info| info.buffer_frames),
            latency: self.shared.clock.latency(),
            clock_master: self.time.is_device_master(),
            volume: self.shared.volume(),
            playing: self.is_playing(),
            error: self.shared.error.lock().clone(),
        }
    }

    /// Stop the thread and release the device. Idempotent.
    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.wake();
        if let Some(thread) = self.thread.lock().take() {
            let _ = thread.join();
        }
        *self.shared.output.lock() = None;
        self.shared.clock.detach();
    }

    /// Publish a new mix position and tell the clock that everything the
    /// device has been given is now stale.
    ///
    /// The queue origin is marked here, on the calling thread, rather than on
    /// the fill thread: the preview clock re-anchors immediately after this
    /// returns, and it must anchor against the *held* reading, not the one
    /// from before the seek. See [`DeviceClock::mark_queue_origin`].
    fn request_position(&self, to: Micros) {
        self.shared.seek_target.store(to.max(0), Ordering::Relaxed);
        self.shared.seek_generation.fetch_add(1, Ordering::Release);
        self.shared.clock.mark_queue_origin();
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.wake();
        if let Some(thread) = self.thread.lock().take() {
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for AudioEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEngine")
            .field("playing", &self.is_playing())
            .field("volume", &self.volume())
            .field("clock_master", &self.is_clock_master())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// The fill thread
// ---------------------------------------------------------------------------

/// State the loop carries between iterations. Extracted so the loop body reads
/// as the sequence of decisions it is rather than a wall of locals.
struct Filling {
    mixer: Option<TimelineMixer>,
    producer: Option<RingProducer>,
    rate: u32,
    channels: usize,
    /// Next timeline frame to mix, at the device rate.
    position: i64,
    seen_seek: u64,
    seen_project: u64,
    was_playing: bool,
    retry_at: Option<Instant>,
    stereo: Vec<f32>,
    interleaved: Vec<f32>,
}

impl Filling {
    fn new() -> Self {
        Self {
            mixer: None,
            producer: None,
            rate: 0,
            channels: MIX_CHANNELS,
            position: 0,
            seen_seek: 0,
            seen_project: 0,
            was_playing: false,
            retry_at: None,
            stereo: Vec::new(),
            interleaved: Vec::new(),
        }
    }

    fn release(&mut self, shared: &Shared) {
        self.producer = None;
        self.mixer = None;
        self.rate = 0;
        *shared.output.lock() = None;
        shared.clock.detach();
    }
}

fn fill_loop(shared: Arc<Shared>) {
    let mut state = Filling::new();

    while !shared.shutdown.load(Ordering::Acquire) {
        manage_device(&shared, &mut state);
        let worked = pump(&shared, &mut state);

        if !worked {
            let mut guard = shared.wake_lock.lock();
            shared.wake.wait_for(&mut guard, IDLE_TICK);
        }
    }

    state.release(&shared);
}

/// Open, close and reopen the device as the world changes under it.
fn manage_device(shared: &Arc<Shared>, state: &mut Filling) {
    if !shared.wanted.load(Ordering::Relaxed) {
        if state.producer.is_some() {
            tracing::debug!("audio device released");
            state.release(shared);
        }
        return;
    }

    let lost = shared
        .output
        .lock()
        .as_ref()
        .is_some_and(|output| output.is_lost());
    if lost {
        // The device walked away. Let go of it cleanly and try again shortly;
        // the clock has already stopped claiming to be the master, so playback
        // continues silently in the meantime rather than freezing.
        state.release(shared);
        *shared.error.lock() = Some("the audio device was disconnected".into());
        state.retry_at = Some(Instant::now() + REOPEN_DELAY);
    }

    if state.producer.is_some() {
        return;
    }
    if state.retry_at.is_some_and(|at| Instant::now() < at) {
        return;
    }

    match AudioOutput::open(Arc::clone(&shared.clock)) {
        Ok((output, producer)) => {
            state.rate = output.sample_rate();
            state.channels = output.channels();
            *shared.output.lock() = Some(output);
            state.producer = Some(producer);
            state.retry_at = None;
            *shared.error.lock() = None;

            // Everything downstream runs at the device's rate, so a device
            // that opened at 44.1 gets a mixer and decoders at 44.1 rather
            // than a resampler bolted on afterwards.
            let mut mixer = TimelineMixer::new(state.rate, Arc::clone(&shared.factory));
            mixer.set_master(shared.volume());
            if let Some(project) = shared.project.lock().as_ref() {
                mixer.set_plan(plan(project));
            }
            state.seen_project = shared.project_generation.load(Ordering::Acquire);
            state.mixer = Some(mixer);
            state.position = micros_to_frames(shared.seek_target.load(Ordering::Relaxed), state.rate);
            state.seen_seek = shared.seek_generation.load(Ordering::Acquire);
        }
        Err(error) => {
            let message = error.to_string();
            let changed = shared.error.lock().as_deref() != Some(message.as_str());
            if changed {
                tracing::warn!(%error, "no audio output; playback will be silent");
            }
            *shared.error.lock() = Some(message);
            state.retry_at = Some(Instant::now() + REOPEN_DELAY);
        }
    }
}

/// One pass of "notice what changed, then fill". Returns whether anything was
/// done, which is what decides between going round again and sleeping.
fn pump(shared: &Arc<Shared>, state: &mut Filling) -> bool {
    let (Some(mixer), Some(producer)) = (state.mixer.as_mut(), state.producer.as_mut()) else {
        return false;
    };

    let generation = shared.project_generation.load(Ordering::Acquire);
    if generation != state.seen_project {
        state.seen_project = generation;
        let planned = shared
            .project
            .lock()
            .as_ref()
            .map(|project| plan(project))
            .unwrap_or_default();
        mixer.set_plan(planned);
    }

    let volume = shared.volume();
    if (volume - mixer.master()).abs() > f32::EPSILON {
        mixer.set_master(volume);
    }

    let seek = shared.seek_generation.load(Ordering::Acquire);
    if seek != state.seen_seek {
        state.seen_seek = seek;
        state.position = micros_to_frames(shared.seek_target.load(Ordering::Relaxed), state.rate);
        producer.flush();
    }

    let playing = shared.playing.load(Ordering::Relaxed);
    if state.was_playing && !playing {
        // Pausing has to take back what is queued, or the device plays on for
        // as long as the ring is deep.
        producer.flush();
    }
    state.was_playing = playing;

    if !playing || mixer.is_silent() {
        return false;
    }

    let block = BLOCK_FRAMES * state.channels;
    let mut worked = false;
    while producer.free() >= block {
        state.stereo.resize(BLOCK_FRAMES * MIX_CHANNELS, 0.0);
        mixer.fill(state.position, &mut state.stereo);
        map_channels(&state.stereo, state.channels, &mut state.interleaved);

        let pushed = producer.push(&state.interleaved);
        state.position += (pushed / state.channels) as i64;
        worked = true;
        if pushed < state.interleaved.len() {
            break;
        }
    }
    worked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audio::decode::ClipReader;
    use crate::modules::project::document::{
        AudioMaterial, CanvasConfig, Segment, TimeRange, Track, TrackKind, Transform,
        MICROS_PER_SECOND,
    };

    /// A factory whose readers answer with a constant, so no test needs a file
    /// or a device to exercise the transport.
    #[derive(Debug)]
    struct Tone;

    struct Constant;

    impl ClipReader for Constant {
        fn read(
            &mut self,
            _at: i64,
            frames: usize,
            out: &mut [f32],
        ) -> crate::modules::audio::Result<()> {
            for sample in out.iter_mut().take(frames * MIX_CHANNELS) {
                *sample = 0.5;
            }
            Ok(())
        }
    }

    impl ClipFactory for Tone {
        fn open(
            &self,
            _path: &str,
            _rate: u32,
            _channels: u16,
        ) -> crate::modules::audio::Result<Box<dyn ClipReader>> {
            Ok(Box::new(Constant))
        }
    }

    fn project() -> Arc<Project> {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.materials.audios.push(AudioMaterial {
            id: "a1".into(),
            path: "/tmp/a1.wav".into(),
            duration: 10 * MICROS_PER_SECOND,
            sample_rate: 48_000,
            channels: 2,
        });
        let mut track = Track::new(TrackKind::Audio, "A1");
        track.segments.push(Segment {
            id: "s1".into(),
            material_id: "a1".into(),
            target_range: TimeRange::new(0, 5 * MICROS_PER_SECOND),
            source_range: TimeRange::new(0, 5 * MICROS_PER_SECOND),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        project.tracks.push(track);
        Arc::new(project)
    }

    #[test]
    fn an_engine_without_a_device_still_starts_and_still_keeps_time() {
        let engine = AudioEngine::with_factory(Arc::new(Tone));
        engine.set_project(project());

        let source = engine.time_source();
        let first = source.now();
        engine.play(0);
        assert!(engine.is_playing());
        std::thread::sleep(Duration::from_millis(20));

        // Whether or not this machine has a sound card, the clock moves and
        // the transport answers. On a silent machine that is wall time; the
        // editor must be usable either way.
        assert!(source.now() >= first);
        engine.pause();
        assert!(!engine.is_playing());
        engine.shutdown();
    }

    /// The end-to-end check: a real device, the real ring, the real callback,
    /// and a clock that has to come out of it running at real time.
    ///
    /// Opt-in, because it opens the machine's sound card. It plays a constant
    /// at half scale for a third of a second, which is a quiet click on
    /// speakers that are on.
    #[test]
    fn with_a_real_device_the_clock_runs_at_the_speed_of_sound() {
        if std::env::var("CHUKCUT_AUDIO_DEVICE_TESTS").is_err()
            || !crate::modules::audio::has_output_device()
        {
            eprintln!("skipped: set CHUKCUT_AUDIO_DEVICE_TESTS=1 with an output device present");
            return;
        }

        let engine = AudioEngine::with_factory(Arc::new(Tone));
        engine.set_project(project());
        engine.play(0);

        // Let the device open and the first buffers go out.
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            engine.is_clock_master(),
            "the device is open but is not driving the clock: {:?}",
            engine.status()
        );

        let source = engine.time_source();
        let before = source.now();
        let wall = Instant::now();
        std::thread::sleep(Duration::from_millis(300));
        let advanced = source.now() - before;
        let elapsed = wall.elapsed().as_micros() as i64;

        // Sample-counted time and wall time agree to within a few buffers.
        // They are not the same clock, which is the entire point; what matters
        // is that the audio one is not stalled, doubled or running backwards.
        assert!(
            (advanced - elapsed).abs() < 60_000,
            "audio time moved {advanced} µs while {elapsed} µs passed"
        );
        engine.shutdown();
    }

    #[test]
    fn the_volume_is_clamped_and_readable() {
        let engine = AudioEngine::with_factory(Arc::new(Tone));
        assert_eq!(engine.volume(), 1.0);
        engine.set_volume(0.25);
        assert_eq!(engine.volume(), 0.25);
        engine.set_volume(-3.0);
        assert_eq!(engine.volume(), 0.0);
        engine.set_volume(f32::NAN);
        assert_eq!(engine.volume(), 1.0);
        engine.shutdown();
    }

    #[test]
    fn transport_calls_return_immediately() {
        // Every one of these is called from an IPC command, on a thread the UI
        // is waiting on.
        let engine = AudioEngine::with_factory(Arc::new(Tone));
        engine.set_project(project());

        let start = Instant::now();
        for _ in 0..200 {
            engine.play(0);
            engine.seek(MICROS_PER_SECOND);
            engine.set_volume(0.5);
            engine.pause();
        }
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "the transport blocked: {:?}",
            start.elapsed()
        );
        engine.shutdown();
    }

    #[test]
    fn a_seek_moves_the_mix_position_without_waiting_for_the_mixer() {
        let engine = AudioEngine::with_factory(Arc::new(Tone));
        engine.set_project(project());
        engine.play(0);
        engine.seek(3 * MICROS_PER_SECOND);
        // The generation is what the fill thread acts on; the point of the
        // test is that publishing it is all the caller does.
        assert!(engine.shared.seek_generation.load(Ordering::Relaxed) >= 2);
        assert_eq!(
            engine.shared.seek_target.load(Ordering::Relaxed),
            3 * MICROS_PER_SECOND
        );
        engine.shutdown();
    }

    #[test]
    fn status_describes_a_machine_with_no_device_honestly() {
        let engine = AudioEngine::with_factory(Arc::new(Tone));
        let status = engine.status();
        // Nothing has asked for a device yet, so there is none open and the
        // clock is wall time. Neither is an error state.
        assert!(!status.clock_master || status.available);
        assert_eq!(status.volume, 1.0);
        assert!(!status.playing);
        engine.shutdown();
    }

    #[test]
    fn stopping_releases_the_project_and_the_device() {
        let engine = AudioEngine::with_factory(Arc::new(Tone));
        engine.set_project(project());
        engine.play(0);
        engine.stop();
        assert!(!engine.is_playing());
        assert!(engine.shared.project.lock().is_none());
        // Give the fill thread a chance to notice and let go.
        std::thread::sleep(Duration::from_millis(20));
        assert!(engine.shared.output.lock().is_none());
        engine.shutdown();
    }
}
