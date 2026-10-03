//! The playback clock, and the pacing decisions that hang off it.
//!
//! ## Time comes from somewhere else
//!
//! The clock does not measure time, it *reads* it, from a [`TimeSource`]. Today
//! that is a monotonic `Instant`. It will be the audio device: audio is mixed in
//! Rust and played through the system, its playback position is the authority
//! for the playhead, and video is matched to it. Humans forgive a dropped frame
//! and never forgive a stutter in audio, so the picture follows the sound and
//! not the other way round.
//!
//! Making the source swappable now costs one trait and means that change is a
//! constructor argument later rather than a rewrite of everything that asks
//! where the playhead is.
//!
//! ## Frames are an edge concept
//!
//! The document stores microseconds; the ring buffer and the frame URL are
//! numbered. [`frame_at`] and [`frame_time`] are the only place the two meet,
//! and they round-trip exactly at every frame rate — including 23.976 and
//! 29.97, where the naive `time * fps / 1e6` lands one frame early often enough
//! to be noticed and never often enough to be reproduced on demand.
//!
//! ## Late frames are dropped, not awaited
//!
//! [`pace`] is the rule from the pipeline document expressed as a pure
//! function: when the renderer falls behind the clock, the frames it missed are
//! abandoned and it resumes at the current position. The alternative —
//! rendering them anyway and showing them late — stretches time, which desyncs
//! from audio and never recovers.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;

use parking_lot::Mutex;

use super::session::sane_fps;
use crate::modules::project::document::{Micros, MICROS_PER_SECOND};

/// How many frames ahead of the playhead the renderer works during playback.
///
/// Deep enough to absorb a slow frame or two, shallow enough that a seek does
/// not throw away much work. The ring holds more than this so that a frame is
/// still there when the clock reaches it.
pub const DEFAULT_READ_AHEAD: usize = 12;

/// Where "now" comes from.
///
/// Implementations return microseconds from an arbitrary, fixed origin, and
/// must never go backwards. The clock only ever takes differences, so the
/// origin does not matter — only that it does not move.
pub trait TimeSource: Send + Sync + std::fmt::Debug {
    fn now(&self) -> Micros;
}

/// Wall time from a monotonic instant. The default, and correct until audio
/// exists to be the master.
#[derive(Debug)]
pub struct MonotonicSource {
    origin: Instant,
}

impl Default for MonotonicSource {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl TimeSource for MonotonicSource {
    fn now(&self) -> Micros {
        self.origin.elapsed().as_micros() as Micros
    }
}

/// A source that only moves when told to.
///
/// Public rather than test-only: it is how the clock gets tested without
/// sleeping, and it is the shape the audio source will take — something
/// external decides what time it is and the clock believes it.
#[derive(Debug, Default)]
pub struct ManualSource {
    now: AtomicI64,
}

impl ManualSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn advance(&self, by: Micros) {
        self.now.fetch_add(by, Ordering::Relaxed);
    }

    pub fn set(&self, to: Micros) {
        self.now.store(to, Ordering::Relaxed);
    }
}

impl TimeSource for ManualSource {
    fn now(&self) -> Micros {
        self.now.load(Ordering::Relaxed)
    }
}

/// The playhead.
///
/// Position is not stored and ticked — it is derived, from an anchor (where the
/// playhead was at the last transition) plus however much the time source has
/// moved since. Nothing accumulates, so nothing drifts, and a render thread
/// that stalls for 200 ms comes back to the right position rather than 200 ms
/// behind.
#[derive(Debug)]
pub struct PlaybackClock {
    source: std::sync::Arc<dyn TimeSource>,
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    playing: bool,
    /// Timeline position at the last play/pause/seek.
    anchor: Micros,
    /// Source reading at that same moment.
    origin: Micros,
    fps: f64,
    duration: Micros,
}

impl PlaybackClock {
    pub fn new(fps: f64, duration: Micros) -> Self {
        Self::with_source(
            std::sync::Arc::new(MonotonicSource::default()),
            fps,
            duration,
        )
    }

    pub fn with_source(source: std::sync::Arc<dyn TimeSource>, fps: f64, duration: Micros) -> Self {
        let origin = source.now();
        Self {
            source,
            inner: Mutex::new(Inner {
                playing: false,
                anchor: 0,
                origin,
                fps: sane_fps(fps),
                duration: duration.max(0),
            }),
        }
    }

    pub fn position(&self) -> Micros {
        let inner = self.inner.lock();
        Self::position_of(&inner, self.source.now())
    }

    fn position_of(inner: &Inner, now: Micros) -> Micros {
        let raw = if inner.playing {
            inner.anchor + (now - inner.origin).max(0)
        } else {
            inner.anchor
        };
        if inner.duration > 0 {
            raw.clamp(0, inner.duration)
        } else {
            raw.max(0)
        }
    }

    pub fn is_playing(&self) -> bool {
        self.inner.lock().playing
    }

    /// Whether playback has run off the end of the project.
    pub fn is_at_end(&self) -> bool {
        let inner = self.inner.lock();
        inner.duration > 0 && Self::position_of(&inner, self.source.now()) >= inner.duration
    }

    pub fn fps(&self) -> f64 {
        self.inner.lock().fps
    }

    pub fn duration(&self) -> Micros {
        self.inner.lock().duration
    }

    /// The frame currently under the playhead.
    pub fn frame(&self) -> i64 {
        let inner = self.inner.lock();
        frame_at(Self::position_of(&inner, self.source.now()), inner.fps)
    }

    pub fn play(&self) {
        let now = self.source.now();
        let mut inner = self.inner.lock();
        if inner.playing {
            return;
        }
        // Re-anchor rather than resume: the source may have moved a long way
        // while we were paused, and none of that is playback time.
        inner.anchor = Self::position_of(&inner, now);
        inner.origin = now;
        inner.playing = true;
    }

    pub fn pause(&self) {
        let now = self.source.now();
        let mut inner = self.inner.lock();
        inner.anchor = Self::position_of(&inner, now);
        inner.origin = now;
        inner.playing = false;
    }

    /// Move the playhead, keeping the play/pause state.
    pub fn seek(&self, to: Micros) {
        let now = self.source.now();
        let mut inner = self.inner.lock();
        let clamped = if inner.duration > 0 {
            to.clamp(0, inner.duration)
        } else {
            to.max(0)
        };
        inner.anchor = clamped;
        inner.origin = now;
    }

    /// Adopt a new session's timing. Used when the project snapshot changes
    /// under the preview.
    pub fn retime(&self, fps: f64, duration: Micros) {
        let now = self.source.now();
        let mut inner = self.inner.lock();
        inner.fps = sane_fps(fps);
        inner.duration = duration.max(0);
        inner.anchor = Self::position_of(&inner, now);
        inner.origin = now;
    }
}

// ---------------------------------------------------------------------------
// Frames and time
// ---------------------------------------------------------------------------

/// Start of frame `n`, in microseconds.
///
/// Rounded, not truncated: at 30 fps a frame is 33333.33… µs, and truncating
/// costs a microsecond per frame — a full frame of drift by the thirty-second
/// mark.
pub fn frame_time(frame: i64, fps: f64) -> Micros {
    let fps = sane_fps(fps);
    (frame as f64 * MICROS_PER_SECOND as f64 / fps).round() as Micros
}

/// The frame displayed at `time`: the largest `n` with `frame_time(n) <= time`.
///
/// Deliberately defined as the inverse of [`frame_time`] rather than as its own
/// bit of arithmetic. `floor(time * fps / 1e6)` disagrees with it whenever the
/// rounding in `frame_time` went up — `frame_time(1)` at 30 fps is 33333, which
/// that formula calls frame 0 — and the disagreement shows up as a preview that
/// is one frame behind at some positions and not others.
pub fn frame_at(time: Micros, fps: f64) -> i64 {
    let fps = sane_fps(fps);
    let mut n = (time as f64 * fps / MICROS_PER_SECOND as f64).floor() as i64;
    // The estimate is out by at most one in either direction.
    while frame_time(n + 1, fps) <= time {
        n += 1;
    }
    while n > 0 && frame_time(n, fps) > time {
        n -= 1;
    }
    n.max(0)
}

/// Nominal duration of one frame.
pub fn frame_interval(fps: f64) -> Micros {
    (MICROS_PER_SECOND as f64 / sane_fps(fps)).round() as Micros
}

/// Whether a frame is already too late to be worth showing.
///
/// One frame interval of tolerance: a frame that is a whole interval behind the
/// playhead has been superseded by the next one, so showing it puts the picture
/// permanently behind the sound.
pub fn is_late(frame: i64, position: Micros, fps: f64) -> bool {
    frame < frame_at(position, fps)
}

/// What the render thread should do next during playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pacing {
    /// Render this frame.
    Render(i64),
    /// The renderer fell behind. Frames `[from, to)` are past due and are
    /// abandoned unrendered; resume at `to`.
    Skip { from: i64, to: i64 },
    /// Far enough ahead. Sleep instead of filling the ring with frames that
    /// will be evicted before the clock reaches them.
    Idle,
}

/// The pacing rule: never render into the past, never run more than
/// `read_ahead` frames into the future.
///
/// `cursor` is the next frame the renderer intends to produce and `position` is
/// where the clock says the playhead is. A pure function of the two, so the
/// decision that governs playback smoothness is testable without a GPU, a
/// clock, or a thread.
pub fn pace(cursor: i64, position: Micros, fps: f64, read_ahead: usize) -> Pacing {
    let current = frame_at(position, fps);
    if cursor < current {
        Pacing::Skip {
            from: cursor,
            to: current,
        }
    } else if cursor >= current + read_ahead.max(1) as i64 {
        Pacing::Idle
    } else {
        Pacing::Render(cursor)
    }
}

/// The start of the frame displayed at `time`: where a cut at the playhead
/// goes, so that the frame on screen becomes the first frame after the cut.
/// Rendering then samples it at this time plus `project::SAMPLE_SLACK`.
pub fn frame_start(time: Micros, fps: f64) -> Micros {
    frame_time(frame_at(time.max(0), fps), fps)
}

/// The frame boundary nearest to `time`. Where a click or a drag on the
/// ruler puts the playhead: on the grid, so that a split there cuts exactly
/// between two frames and the timecode reads a whole frame.
pub fn nearest_frame_time(time: Micros, fps: f64) -> Micros {
    let n = frame_at(time.max(0), fps);
    let (here, next) = (frame_time(n, fps), frame_time(n + 1, fps));
    if time.max(0) - here > next - time.max(0) {
        next
    } else {
        here
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_times_land_on_the_frame_grid() {
        // 3.016761 s, the cut QA found: frame 90 starts at 3 000 000 and
        // frame 91 at 3 033 333, so that click goes to 91; one a little
        // earlier goes to 90.
        assert_eq!(nearest_frame_time(3_016_761, 30.0), 3_033_333);
        assert_eq!(nearest_frame_time(3_016_000, 30.0), 3_000_000);
        assert_eq!(frame_start(3_033_332, 30.0), 3_000_000);
        assert_eq!(frame_start(3_033_333, 30.0), 3_033_333);
        // Every result is a frame time, at any rate.
        for fps in [23.976, 25.0, 29.97, 30.0, 59.94, 60.0] {
            for t in (0..2_000_000).step_by(7_919) {
                for snapped in [nearest_frame_time(t, fps), frame_start(t, fps)] {
                    assert_eq!(frame_time(frame_at(snapped, fps), fps), snapped);
                }
                assert!(frame_start(t, fps) <= t);
            }
        }
        assert_eq!(nearest_frame_time(-5, 30.0), 0);
    }
    use std::sync::Arc;

    /// Every frame rate a project is plausibly at, including the two broadcast
    /// rates that are not integers and cause most of the off-by-one bugs.
    const RATES: [f64; 8] = [23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 59.94, 60.0];

    #[test]
    fn frame_numbers_round_trip_through_time_at_every_rate() {
        for fps in RATES {
            for n in [0i64, 1, 2, 29, 30, 899, 900, 1799, 108_000] {
                let t = frame_time(n, fps);
                assert_eq!(frame_at(t, fps), n, "fps {fps}, frame {n}, time {t}");
            }
        }
    }

    #[test]
    fn a_time_inside_a_frame_maps_to_that_frame() {
        for fps in RATES {
            let interval = frame_interval(fps);
            for n in [0i64, 1, 500] {
                let start = frame_time(n, fps);
                assert_eq!(frame_at(start, fps), n);
                assert_eq!(frame_at(start + interval / 2, fps), n);
                // One microsecond before the next frame still belongs to this
                // one: frames are half-open, exactly like segments.
                assert_eq!(frame_at(frame_time(n + 1, fps) - 1, fps), n);
            }
        }
    }

    #[test]
    fn known_frame_boundaries() {
        assert_eq!(frame_time(0, 30.0), 0);
        assert_eq!(frame_time(1, 30.0), 33_333);
        assert_eq!(frame_time(30, 30.0), 1_000_000);
        assert_eq!(frame_time(900, 30.0), 30_000_000);
        assert_eq!(frame_at(1_000_000, 30.0), 30);
        assert_eq!(frame_at(999_999, 30.0), 29);
        assert_eq!(frame_at(33_333, 30.0), 1, "the naive formula says 0 here");
        assert_eq!(frame_at(1_000_000, 25.0), 25);
        // 1e6/23.976 µs per frame: frame 24 starts at 1001001, so a microsecond
        // earlier is still frame 23.
        assert_eq!(frame_at(1_001_001, 23.976), 24);
        assert_eq!(frame_at(1_001_000, 23.976), 23);
    }

    #[test]
    fn a_nonsense_frame_rate_does_not_divide_by_zero() {
        assert_eq!(frame_time(30, 0.0), frame_time(30, 30.0));
        assert_eq!(frame_at(1_000_000, f64::NAN), 30);
        assert_eq!(frame_at(-5, 30.0), 0);
    }

    // -----------------------------------------------------------------------
    // Pacing
    // -----------------------------------------------------------------------

    #[test]
    fn a_renderer_that_is_up_to_date_renders_the_next_frame() {
        assert_eq!(pace(10, frame_time(10, 30.0), 30.0, 12), Pacing::Render(10));
        assert_eq!(pace(11, frame_time(10, 30.0), 30.0, 12), Pacing::Render(11));
    }

    #[test]
    fn a_renderer_that_fell_behind_drops_the_frames_it_missed() {
        // The clock is at frame 40, the renderer was about to produce frame 33.
        let position = frame_time(40, 30.0);
        assert_eq!(
            pace(33, position, 30.0, 12),
            Pacing::Skip { from: 33, to: 40 },
            "time is not stretched to let the renderer catch up"
        );
    }

    #[test]
    fn a_renderer_far_enough_ahead_stops_rendering() {
        let position = frame_time(10, 30.0);
        assert_eq!(pace(22, position, 30.0, 12), Pacing::Idle);
        assert_eq!(pace(21, position, 30.0, 12), Pacing::Render(21));
        // A zero read-ahead still renders the current frame rather than
        // deadlocking on an empty ring.
        assert_eq!(pace(10, position, 30.0, 0), Pacing::Render(10));
    }

    #[test]
    fn lateness_is_measured_against_the_playhead() {
        let position = frame_time(40, 30.0);
        assert!(is_late(39, position, 30.0));
        assert!(!is_late(40, position, 30.0));
        assert!(!is_late(41, position, 30.0));
    }

    // -----------------------------------------------------------------------
    // The clock itself
    // -----------------------------------------------------------------------

    fn clock(duration: Micros) -> (PlaybackClock, Arc<ManualSource>) {
        let source = Arc::new(ManualSource::new());
        let clock = PlaybackClock::with_source(source.clone(), 30.0, duration);
        (clock, source)
    }

    #[test]
    fn a_paused_clock_does_not_move() {
        let (clock, source) = clock(10_000_000);
        source.advance(5_000_000);
        assert_eq!(clock.position(), 0);
        assert!(!clock.is_playing());
    }

    #[test]
    fn playing_advances_with_the_source() {
        let (clock, source) = clock(10_000_000);
        clock.play();
        source.advance(2_000_000);
        assert_eq!(clock.position(), 2_000_000);
        assert_eq!(clock.frame(), 60);
    }

    #[test]
    fn pausing_freezes_the_position_and_resuming_does_not_jump() {
        let (clock, source) = clock(10_000_000);
        clock.play();
        source.advance(1_000_000);
        clock.pause();
        // Time passes while paused; none of it is playback time.
        source.advance(9_000_000);
        assert_eq!(clock.position(), 1_000_000);
        clock.play();
        assert_eq!(clock.position(), 1_000_000);
        source.advance(500_000);
        assert_eq!(clock.position(), 1_500_000);
    }

    #[test]
    fn seeking_while_playing_keeps_playing_from_the_new_position() {
        let (clock, source) = clock(10_000_000);
        clock.play();
        source.advance(1_000_000);
        clock.seek(5_000_000);
        assert!(clock.is_playing());
        assert_eq!(clock.position(), 5_000_000);
        source.advance(200_000);
        assert_eq!(clock.position(), 5_200_000);
    }

    #[test]
    fn the_playhead_stops_at_the_end_of_the_project() {
        let (clock, source) = clock(2_000_000);
        clock.play();
        source.advance(10_000_000);
        assert_eq!(clock.position(), 2_000_000);
        assert!(clock.is_at_end());
    }

    #[test]
    fn seeking_outside_the_project_is_clamped() {
        let (clock, _) = clock(2_000_000);
        clock.seek(-1_000);
        assert_eq!(clock.position(), 0);
        clock.seek(99_000_000);
        assert_eq!(clock.position(), 2_000_000);
    }

    #[test]
    fn an_empty_project_has_no_end_to_clamp_to() {
        let (clock, source) = clock(0);
        clock.play();
        source.advance(1_000_000);
        assert_eq!(clock.position(), 1_000_000);
        assert!(!clock.is_at_end());
    }

    #[test]
    fn retiming_keeps_the_playhead_where_it_was() {
        let (clock, source) = clock(10_000_000);
        clock.play();
        source.advance(1_000_000);
        clock.retime(60.0, 20_000_000);
        assert_eq!(clock.position(), 1_000_000);
        assert_eq!(clock.fps(), 60.0);
        assert_eq!(clock.frame(), 60);
        source.advance(1_000_000);
        assert_eq!(clock.position(), 2_000_000, "still playing across a retime");
    }
}
