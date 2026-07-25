//! The audio device as the master clock.
//!
//! `preview::clock::TimeSource` left a seam for exactly this: the playhead
//! reads its time from something, and that something should be the sound card,
//! because the sound card is the only clock in the machine the user can
//! actually hear. A monotonic `Instant` runs at its own rate and the device
//! runs at its own — 48 000 samples per second is only *nominally* a second —
//! and any difference between the two accumulates as picture drifting away
//! from sound. Counting samples the device has consumed makes that difference
//! zero by definition.
//!
//! ## Latency, or the picture leads the sound
//!
//! The callback is handed samples some milliseconds *before* they are audible:
//! they sit in the device buffer first. A clock that counted them as played
//! the moment they were written would run ahead by exactly the buffer depth,
//! which at a typical 20 ms is most of a frame at 30 fps and plainly visible
//! on anything percussive. So the position reported here is the sample count
//! minus the latency the device reports for that very callback.
//!
//! ## Never backwards
//!
//! [`TimeSource`] promises a reading that does not go backwards, and two
//! things here would otherwise break it: the reported latency changes between
//! callbacks, and the whole source can fall back to wall time when there is no
//! device. Both are handled the same way — a monotonic floor, plus an offset
//! that absorbs the discontinuity when the underlying source changes — so a
//! device disappearing mid-playback costs nothing but the sound.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::Mutex;

use crate::modules::preview::clock::TimeSource;
use crate::modules::project::document::{Micros, MICROS_PER_SECOND};

/// The counters the real-time callback writes and everything else reads.
///
/// Every field is a plain atomic written with `Relaxed` ordering: the callback
/// must not be made to wait on a reader, and nobody needs these values to be
/// consistent with each other to the sample — a clock that is one callback
/// stale is a clock that is 10 ms stale, which is below what anyone can see.
#[derive(Debug)]
pub struct DeviceClock {
    /// Sample *frames* handed to the device since the stream opened.
    frames: AtomicU64,
    /// The device's sample rate, or 0 when there is no device.
    rate: AtomicU32,
    /// How far ahead of audibility the callback runs.
    latency: AtomicI64,
    /// Whether a stream is currently running.
    active: AtomicBool,
    /// The frame count at which the audio for the current playhead position
    /// was queued. See [`DeviceClock::mark_queue_origin`].
    queue_origin: AtomicU64,
}

impl Default for DeviceClock {
    fn default() -> Self {
        Self {
            frames: AtomicU64::new(0),
            rate: AtomicU32::new(0),
            latency: AtomicI64::new(0),
            active: AtomicBool::new(false),
            queue_origin: AtomicU64::new(0),
        }
    }
}

impl DeviceClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// A stream has opened at `rate`. Resets the count, because the position
    /// is only meaningful relative to the stream that produced it.
    pub fn attach(&self, rate: u32) {
        self.frames.store(0, Ordering::Relaxed);
        self.latency.store(0, Ordering::Relaxed);
        self.queue_origin.store(0, Ordering::Relaxed);
        self.rate.store(rate.max(1), Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
    }

    /// Record that the audio for the playhead's current position has just been
    /// queued, and that nothing before it will be heard.
    ///
    /// This is what makes a seek land honestly. The samples handed to the
    /// device are heard one buffer later, so a clock that started counting the
    /// moment a seek happened would report a position the user cannot hear yet
    /// and the picture would arrive ahead of the sound by the buffer depth,
    /// every time. Holding the reading at the queue point until the device has
    /// played up to it costs the playhead a few milliseconds of stillness
    /// after a seek and buys exact alignment for everything after.
    pub fn mark_queue_origin(&self) {
        let frames = self.frames.load(Ordering::Relaxed);
        self.queue_origin.store(frames, Ordering::Relaxed);
    }

    /// The stream has stopped or the device has gone away.
    pub fn detach(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn latency(&self) -> Micros {
        self.latency.load(Ordering::Relaxed)
    }

    /// Called from the device callback, once per buffer. Two relaxed stores
    /// and nothing else.
    pub fn advance(&self, frames: u64, latency: Micros) {
        self.frames.fetch_add(frames, Ordering::Relaxed);
        self.latency.store(latency.max(0), Ordering::Relaxed);
    }

    /// Where the sound the user is hearing right now sits in the stream.
    ///
    /// Clamped at zero rather than allowed to go negative: at the very start
    /// of a stream the device has been given a buffer it has not begun to play,
    /// and "the first sample has not been heard yet" is the honest answer.
    fn played(&self) -> Micros {
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 {
            return 0;
        }
        let handed = self.frames.load(Ordering::Relaxed);
        let latency = micros_to_frames(self.latency.load(Ordering::Relaxed), rate) as u64;
        let played = handed.saturating_sub(latency);
        frames_to_micros(played.max(self.queue_origin.load(Ordering::Relaxed)), rate)
    }
}

/// Sample frames as microseconds, without losing a microsecond per second to
/// integer division on rates that do not divide evenly.
pub fn frames_to_micros(frames: u64, rate: u32) -> Micros {
    if rate == 0 {
        return 0;
    }
    ((frames as i128 * MICROS_PER_SECOND as i128) / rate as i128) as Micros
}

/// Microseconds as sample frames, rounded to nearest.
pub fn micros_to_frames(micros: Micros, rate: u32) -> i64 {
    if rate == 0 || micros <= 0 {
        return 0;
    }
    let numerator = micros as i128 * rate as i128;
    let denominator = MICROS_PER_SECOND as i128;
    ((numerator + denominator / 2) / denominator) as i64
}

/// The [`TimeSource`] the preview clock runs on.
#[derive(Debug)]
pub struct AudioTimeSource {
    device: std::sync::Arc<DeviceClock>,
    fallback: Instant,
    state: Mutex<Continuity>,
}

/// What keeps the reading monotonic across a change of underlying source.
#[derive(Debug)]
struct Continuity {
    /// Whether the last reading came from the device.
    from_device: bool,
    /// Added to the raw reading so the first value after a switch matches the
    /// last one before it.
    offset: Micros,
    /// The last value handed out. The floor for the next one.
    last: Micros,
}

impl AudioTimeSource {
    pub fn new(device: std::sync::Arc<DeviceClock>) -> Self {
        Self {
            device,
            fallback: Instant::now(),
            state: Mutex::new(Continuity {
                from_device: false,
                offset: 0,
                last: 0,
            }),
        }
    }

    pub fn device(&self) -> &std::sync::Arc<DeviceClock> {
        &self.device
    }

    /// Whether the reading currently comes from the sound card rather than
    /// from wall time. Reported to the UI so "no audio device" is visible
    /// rather than mysterious.
    pub fn is_device_master(&self) -> bool {
        self.device.is_active()
    }
}

impl TimeSource for AudioTimeSource {
    fn now(&self) -> Micros {
        let from_device = self.device.is_active();
        let raw = if from_device {
            self.device.played()
        } else {
            self.fallback.elapsed().as_micros() as Micros
        };

        let mut state = self.state.lock();
        if from_device != state.from_device {
            // The two sources have unrelated origins. Rebase so the change is
            // invisible to the playhead, which only ever takes differences.
            state.offset = state.last - raw;
            state.from_device = from_device;
        }
        let value = raw + state.offset;
        // The device's own reading can step back when the reported latency
        // grows; the clock contract says it may not.
        state.last = state.last.max(value);
        state.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const RATE: u32 = 48_000;

    #[test]
    fn frames_and_micros_round_trip() {
        assert_eq!(frames_to_micros(48_000, RATE), MICROS_PER_SECOND);
        assert_eq!(micros_to_frames(MICROS_PER_SECOND, RATE), 48_000);
        assert_eq!(frames_to_micros(44_100, 44_100), MICROS_PER_SECOND);
        assert_eq!(micros_to_frames(MICROS_PER_SECOND, 44_100), 44_100);
        // Half a second at a rate that does not divide a microsecond evenly.
        assert_eq!(micros_to_frames(500_000, 44_100), 22_050);
    }

    #[test]
    fn degenerate_rates_and_times_do_not_divide_by_zero() {
        assert_eq!(frames_to_micros(1_000, 0), 0);
        assert_eq!(micros_to_frames(1_000, 0), 0);
        assert_eq!(micros_to_frames(-1, RATE), 0);
    }

    #[test]
    fn long_positions_do_not_overflow() {
        // Three hours of 48 kHz overflows an i64 multiply that is not widened.
        let frames = 3 * 60 * 60 * 48_000u64;
        assert_eq!(frames_to_micros(frames, RATE), 3 * 60 * 60 * MICROS_PER_SECOND);
    }

    #[test]
    fn the_position_is_the_sample_count_less_the_buffer_depth() {
        let device = Arc::new(DeviceClock::new());
        device.attach(RATE);
        // One second written, 20 ms of it still in the device buffer.
        device.advance(48_000, 20_000);
        assert_eq!(device.played(), 980_000);
    }

    #[test]
    fn the_first_buffer_is_not_audible_yet() {
        let device = Arc::new(DeviceClock::new());
        device.attach(RATE);
        device.advance(480, 20_000);
        assert_eq!(device.played(), 0, "10 ms written, 20 ms of latency");
    }

    #[test]
    fn the_playhead_holds_until_the_audio_a_seek_queued_is_audible() {
        let device = Arc::new(DeviceClock::new());
        device.attach(RATE);
        // A second handed over, 20 ms of it not yet heard.
        device.advance(48_000, 20_000);
        // A seek: everything queued is discarded and the new audio goes in
        // behind what the device already holds.
        device.mark_queue_origin();

        let source = AudioTimeSource::new(Arc::clone(&device));
        let at_seek = source.now();

        // The device works through the 20 ms it already had. None of that is
        // the new position, so the playhead must not move.
        device.advance(480, 20_000);
        assert_eq!(source.now(), at_seek, "the picture would lead the sound");
        device.advance(480, 20_000);
        assert_eq!(source.now(), at_seek);

        // And from there it advances sample for sample.
        device.advance(4_800, 20_000);
        assert_eq!(source.now() - at_seek, 100_000);
    }

    #[test]
    fn the_clock_follows_the_device_when_there_is_one() {
        let device = Arc::new(DeviceClock::new());
        let source = AudioTimeSource::new(Arc::clone(&device));
        device.attach(RATE);

        let start = source.now();
        device.advance(48_000, 0);
        assert_eq!(source.now() - start, MICROS_PER_SECOND);
        device.advance(24_000, 0);
        assert_eq!(source.now() - start, 1_500_000);
    }

    #[test]
    fn a_growing_latency_does_not_move_the_clock_backwards() {
        let device = Arc::new(DeviceClock::new());
        let source = AudioTimeSource::new(Arc::clone(&device));
        device.attach(RATE);
        device.advance(48_000, 5_000);
        let before = source.now();

        // The device re-reports its latency as much deeper. The true position
        // is now earlier; the clock may not follow it there.
        device.advance(0, 40_000);
        assert_eq!(source.now(), before);

        // And it resumes advancing once the count has caught up.
        device.advance(48_000, 40_000);
        assert!(source.now() > before);
    }

    #[test]
    fn losing_the_device_mid_playback_does_not_jump_the_playhead() {
        let device = Arc::new(DeviceClock::new());
        let source = AudioTimeSource::new(Arc::clone(&device));
        device.attach(RATE);
        device.advance(48_000 * 10, 0);
        let before = source.now();

        // The device goes away. Wall time has its own, much smaller origin —
        // without rebasing, the playhead would leap back to nearly zero.
        device.detach();
        let after = source.now();
        assert!(after >= before, "{after} < {before}");
        assert!(after - before < MICROS_PER_SECOND, "and it did not leap forward either");
    }

    #[test]
    fn without_a_device_the_clock_still_runs() {
        let device = Arc::new(DeviceClock::new());
        let source = AudioTimeSource::new(Arc::clone(&device));
        assert!(!source.is_device_master());

        let first = source.now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(source.now() > first, "the editor must still play silently");
    }
}
