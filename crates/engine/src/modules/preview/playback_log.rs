//! What the native player writes into the log while it plays.
//!
//! The player ([`super::player`]) renders ahead of the clock and the UI takes
//! the frame that is due. A stutter is then one of two things: frames reached
//! the screen at uneven intervals, or the renderer could not keep up and
//! skipped frames to stay in sync with the sound. Both are measured here and
//! written as one line every [`WINDOW`] of playback, and one line for the
//! whole run when playback stops:
//!
//! ```text
//! INFO …playback_log: preview playback window seconds=10.0 fps=30 size=1080x1920 sharing=shared \
//!   shown=300 stalls=0 gap_p95_ms=36.0 gap_max_ms=41.2 rendered=301 render_mean_ms=11.9 \
//!   render_p95_ms=15.5 render_max_ms=22.0 over_budget=0 budget_ms=33.3 skipped=0 late=0 discarded=0
//! WARN …playback_log: preview playback run seconds=42.3 … stalls=7 gap_max_ms=180.4 … skipped=12
//! ```
//!
//! (One line each in the file.) How to read it:
//!
//! - `shown`, `gap_p95_ms`, `gap_max_ms` — frames the UI put on screen and the
//!   time between two of them. At 30 fps a smooth run has gaps around 33 ms;
//!   the UI checks for a due frame every 8 ms, so ±8 ms is normal.
//! - `stalls` — gaps of two frame intervals or more: a picture that stayed on
//!   screen for a whole extra frame. **This is the number a user sees.**
//! - `render_*` — the time from starting a frame to having it ready
//!   (composite plus readback or export). Above `budget_ms` on average, the
//!   renderer cannot keep up; `over_budget` counts the frames that were.
//! - `skipped` — frames the renderer never made because it jumped ahead to
//!   stay in sync. `late` — frames made but passed by the clock before the UI
//!   took them. `discarded` — frames thrown away by an edit or a seek.
//!
//! A line is a WARN when the run had a stall, a skipped frame or a late one,
//! and an INFO otherwise, so `grep WARN` finds the stutters.
//!
//! ## Cost
//!
//! Two histogram adds per frame under a leaf lock; formatting happens once per
//! window, after the lock is dropped.

use std::time::{Duration, Instant};

use super::player::PlayerStats;
use super::stats::Histogram;

/// How much playback one line covers.
pub const WINDOW: Duration = Duration::from_secs(10);

/// Counts over one span of playback: a window or the whole run.
struct Totals {
    started: Instant,
    /// Start-to-ready time of each rendered frame, in microseconds.
    render: Histogram,
    over_budget: u64,
    /// Time between two frames reaching the screen, in microseconds.
    gaps: Histogram,
    shown: u64,
    stalls: u64,
    /// The player's cumulative counters when the span began.
    base: PlayerStats,
}

impl Totals {
    fn new(now: Instant, base: PlayerStats) -> Self {
        Self {
            started: now,
            render: Histogram::new(),
            over_budget: 0,
            gaps: Histogram::new(),
            shown: 0,
            stalls: 0,
            base,
        }
    }
}

struct Run {
    fps: f64,
    size: (u32, u32),
    sharing: &'static str,
    budget_us: i64,
    last_shown: Option<Instant>,
    window: Totals,
    total: Totals,
}

/// One playback run's measurements. Lives behind the player's leaf mutex.
#[derive(Default)]
pub struct PlaybackLog {
    run: Option<Run>,
}

/// One line's numbers, ready to emit with no lock held.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// `window` or `run`.
    pub kind: &'static str,
    pub seconds: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub sharing: &'static str,
    pub shown: u64,
    pub stalls: u64,
    pub gap_p95_ms: f64,
    pub gap_max_ms: f64,
    pub rendered: u64,
    pub render_mean_ms: f64,
    pub render_p95_ms: f64,
    pub render_max_ms: f64,
    pub over_budget: u64,
    pub budget_ms: f64,
    pub skipped: u64,
    pub late: u64,
    pub discarded: u64,
}

impl Report {
    /// Whether the user could have seen something wrong.
    pub fn stuttered(&self) -> bool {
        self.stalls > 0 || self.skipped > 0 || self.late > 0
    }

    pub fn emit(&self) {
        macro_rules! line {
            ($level:ident) => {
                tracing::$level!(
                    seconds = self.seconds,
                    fps = self.fps,
                    size = %format_args!("{}x{}", self.width, self.height),
                    sharing = self.sharing,
                    shown = self.shown,
                    stalls = self.stalls,
                    gap_p95_ms = self.gap_p95_ms,
                    gap_max_ms = self.gap_max_ms,
                    rendered = self.rendered,
                    render_mean_ms = self.render_mean_ms,
                    render_p95_ms = self.render_p95_ms,
                    render_max_ms = self.render_max_ms,
                    over_budget = self.over_budget,
                    budget_ms = self.budget_ms,
                    skipped = self.skipped,
                    late = self.late,
                    discarded = self.discarded,
                    "preview playback {}",
                    self.kind
                )
            };
        }
        if self.stuttered() {
            line!(warn);
        } else {
            line!(info);
        }
    }
}

fn ms(micros: f64) -> f64 {
    (micros / 100.0).round() / 10.0
}

impl PlaybackLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    /// Playback is on. Starts a run if none is going.
    pub fn playing(
        &mut self,
        now: Instant,
        stats: PlayerStats,
        fps: f64,
        size: (u32, u32),
        sharing: &'static str,
    ) {
        if let Some(run) = &mut self.run {
            // A resize or a sharing change mid-run: the next line says so.
            run.size = size;
            run.sharing = sharing;
            return;
        }
        let fps = if fps > 0.0 { fps } else { 30.0 };
        self.run = Some(Run {
            fps,
            size,
            sharing,
            budget_us: (1e6 / fps).round() as i64,
            last_shown: None,
            window: Totals::new(now, stats),
            total: Totals::new(now, stats),
        });
    }

    /// One frame rendered in `micros`, from start to ready.
    pub fn record_render(&mut self, micros: i64) {
        let Some(run) = &mut self.run else {
            return;
        };
        let over = micros > run.budget_us;
        for totals in [&mut run.window, &mut run.total] {
            totals.render.record(micros);
            totals.over_budget += over as u64;
        }
    }

    /// One frame reached the screen at `now`.
    pub fn record_shown(&mut self, now: Instant) {
        let Some(run) = &mut self.run else {
            return;
        };
        let gap = run
            .last_shown
            .map(|last| now.saturating_duration_since(last).as_micros() as i64);
        run.last_shown = Some(now);
        let stall = gap.is_some_and(|gap| gap >= 2 * run.budget_us);
        for totals in [&mut run.window, &mut run.total] {
            totals.shown += 1;
            if let Some(gap) = gap {
                totals.gaps.record(gap);
            }
            totals.stalls += stall as u64;
        }
    }

    /// The window's line, once [`WINDOW`] has passed, and a fresh window.
    pub fn tick(&mut self, now: Instant, stats: PlayerStats) -> Option<Report> {
        let run = self.run.as_mut()?;
        if now.saturating_duration_since(run.window.started) < WINDOW {
            return None;
        }
        let report = report("window", run, &run.window, now, stats);
        run.window = Totals::new(now, stats);
        Some(report)
    }

    /// Playback stopped: the whole run's line.
    pub fn stop(&mut self, now: Instant, stats: PlayerStats) -> Option<Report> {
        let run = self.run.take()?;
        let report = report("run", &run, &run.total, now, stats);
        // A run that showed nothing and rendered nothing has nothing to say.
        (report.shown > 0 || report.rendered > 0).then_some(report)
    }
}

fn report(
    kind: &'static str,
    run: &Run,
    totals: &Totals,
    now: Instant,
    stats: PlayerStats,
) -> Report {
    Report {
        kind,
        seconds: (now.saturating_duration_since(totals.started).as_secs_f64() * 10.0).round()
            / 10.0,
        fps: run.fps,
        width: run.size.0,
        height: run.size.1,
        sharing: run.sharing,
        shown: totals.shown,
        stalls: totals.stalls,
        gap_p95_ms: ms(totals.gaps.percentile(0.95) as f64),
        gap_max_ms: ms(totals.gaps.max() as f64),
        rendered: totals.render.count(),
        render_mean_ms: ms(totals.render.mean()),
        render_p95_ms: ms(totals.render.percentile(0.95) as f64),
        render_max_ms: ms(totals.render.max() as f64),
        over_budget: totals.over_budget,
        budget_ms: ms(run.budget_us as f64),
        skipped: stats.skipped.saturating_sub(totals.base.skipped),
        late: stats.late.saturating_sub(totals.base.late),
        discarded: stats.discarded.saturating_sub(totals.base.discarded),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn nothing_is_measured_while_paused() {
        let mut log = PlaybackLog::new();
        log.record_render(5_000);
        log.record_shown(Instant::now());
        assert!(log
            .tick(Instant::now() + WINDOW, PlayerStats::default())
            .is_none());
        assert!(log.stop(Instant::now(), PlayerStats::default()).is_none());
    }

    #[test]
    fn a_smooth_run_is_an_info_line_with_its_numbers() {
        let start = Instant::now();
        let mut log = PlaybackLog::new();
        log.playing(start, PlayerStats::default(), 30.0, (1080, 1920), "shared");
        for frame in 0..30u64 {
            log.record_render(12_000);
            log.record_shown(at(start, frame * 33));
        }
        let report = log
            .stop(at(start, 1000), PlayerStats::default())
            .expect("a run");
        assert_eq!(report.kind, "run");
        assert_eq!(report.shown, 30);
        assert_eq!(report.rendered, 30);
        assert_eq!(report.stalls, 0);
        assert_eq!(report.over_budget, 0);
        assert_eq!(report.budget_ms, 33.3);
        assert!(report.gap_max_ms >= 33.0 && report.gap_max_ms <= 33.25);
        assert!(!report.stuttered());
    }

    /// A picture held for 200 ms is a stall, and the run says so as a WARN
    /// with the worst gap; the renderer's skips come from the player's
    /// counters, as a difference over the run.
    #[test]
    fn a_held_frame_is_a_stall_and_skips_are_counted_for_the_run() {
        let start = Instant::now();
        let mut log = PlaybackLog::new();
        let before = PlayerStats {
            skipped: 4,
            ..PlayerStats::default()
        };
        log.playing(start, before, 30.0, (1920, 1080), "readback");
        log.record_shown(at(start, 0));
        log.record_shown(at(start, 33));
        log.record_shown(at(start, 233));
        log.record_render(90_000);
        let after = PlayerStats {
            skipped: 10,
            late: 1,
            ..PlayerStats::default()
        };
        let report = log.stop(at(start, 300), after).expect("a run");
        assert_eq!(report.stalls, 1);
        assert_eq!(report.gap_max_ms, 200.0);
        assert_eq!(report.over_budget, 1);
        assert_eq!(report.render_max_ms, 90.0);
        assert_eq!(report.skipped, 6, "10 - 4: only this run's");
        assert_eq!(report.late, 1);
        assert!(report.stuttered());
    }

    #[test]
    fn a_long_run_writes_a_line_per_window() {
        let start = Instant::now();
        let mut log = PlaybackLog::new();
        log.playing(start, PlayerStats::default(), 25.0, (640, 360), "shared");
        let mut windows = 0;
        for frame in 0..(25 * 35) {
            let now = at(start, frame * 40);
            log.record_render(8_000);
            log.record_shown(now);
            if let Some(report) = log.tick(now, PlayerStats::default()) {
                windows += 1;
                assert_eq!(report.kind, "window");
                assert!(
                    (250..=251).contains(&report.shown),
                    "ten seconds at 25 fps: {}",
                    report.shown
                );
            }
        }
        assert_eq!(windows, 3, "35 s is three full windows");
        let run = log
            .stop(at(start, 35_000), PlayerStats::default())
            .expect("a run");
        assert_eq!(run.shown, 875, "the run line covers all of it");
    }
}
