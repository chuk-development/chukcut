//! What the log file has to contain for a stutter to be diagnosable afterwards.
//!
//! The owner reports that playback stutters. Everything that would explain it —
//! `preview frame ready`, the dropped-frame line, the slow frame request — is at
//! DEBUG, and the file layer in `workspace::logging` is fixed at INFO. So a
//! stutter he reports leaves *no evidence on disk*, and the only way to
//! investigate was to ask him to reproduce it under `RUST_LOG`, which is a
//! different question from the one he asked.
//!
//! This module closes that. It aggregates the per-frame numbers and emits an
//! INFO line about once a second while playback runs, plus one when it stops.
//! A stutter is then a line in the file with a p99 far above the mean, or a
//! `shown` count with `rendered=0` next to it, or a `dropped` that is not zero.
//!
//! ## What lands in the file
//!
//! ```text
//! 2026-07-26T19:03:17.923585Z  INFO chukcut_engine::modules::preview::stats: preview playback \
//!   shown=30 dropped=2 discarded=1 rendered=30 scrubs=0 over_budget=1 mean_ms=12.7 \
//!   p99_ms=120.25 max_ms=120.0 budget_ms=33.333 window_ms=1000.0 decode="vaapi" \
//!   encode="vaapi" width=1080 height=1920 downscaled=false rung=1 ladder="three-quarter" \
//!   render_width=810 render_height=1440 render_quality=80
//! ```
//!
//! (One line in the file; wrapped here.) How to read it:
//!
//! - `shown` — frames the frontend was told to display, counted by the pacer.
//!   This is what the user saw.
//! - `dropped` — frames abandoned because the renderer fell behind the clock.
//! - `discarded` — frames that *were* composited and then thrown away without
//!   a JPEG, because the playhead had passed them by the time the compositor
//!   finished. Deliberately a separate number from `dropped`: a dropped frame
//!   cost nothing, a discarded one cost a whole composite. `discarded` far
//!   above zero means the renderer is finishing work that is already useless.
//! - `rendered` — frames composited and encoded. **`shown` far above
//!   `rendered` is a stalled renderer**: the clock moved and the picture did
//!   not.
//! - `scrubs` — frames rendered because the playhead moved. A timeline the user
//!   is dragging shows up here rather than in `rendered`.
//! - `over_budget` — how many of `rendered` took longer than `budget_ms`.
//! - `mean_ms` / `p99_ms` / `max_ms` — frame cost, where a frame's cost is the
//!   slower of compositing and encoding, since the two overlap on different
//!   threads. **`mean_ms` inside the budget with `max_ms` far outside it is the
//!   shape of a stutter**; the mean alone is the number that already looked
//!   fine while it stuttered.
//! - `window_ms` — how much wall time this line covers. Nominally 1000; a
//!   larger number is itself evidence that the pacer was blocked.
//! - `decode` — `vaapi`, `software`, or `unknown` before the render thread has
//!   a device. `software` is a 20× per-frame regression and the reason is on
//!   the separate `preview decode path` line.
//! - `encode` — `vaapi` or `libjpeg-turbo`, the JPEG encoder in use.
//! - `width`/`height`/`downscaled` — the session's size: what a *paused* frame
//!   is rendered at, and whether that is below the project's canvas.
//! - `rung`/`ladder`/`render_width`/`render_height`/`render_quality` — the
//!   quality ladder ([`super::ladder`]). `rung=0` is playback at the session's
//!   own size and quality; a higher rung is what playback gave up to keep up,
//!   and `render_*` is what a playback frame was actually made of. A summary
//!   whose `render_width` is below `width` is the ladder working; one that sits
//!   at rung 2 for a whole session is a machine that cannot hold this project.
//!
//! Two more lines, each emitted once per occurrence rather than periodically:
//! `preview seek was slow` (a seek that cost more than [`SLOW_SEEK`], with the
//! direction, because backwards is the expensive one) and `preview JPEG encoder
//! changed backend` (with the reason). Playback stopping produces
//! `preview playback stopped`, whose fields are identical, so grepping for
//! `preview playback` catches the periodic lines and the final one together.
//!
//! ## Why a summary and not a per-frame INFO line
//!
//! Promoting `preview frame ready` to INFO would be one file write per frame —
//! 30 a second, and `logging.rs` does not buffer, so each is a `write` syscall
//! on the encode thread. That is measurement that changes what it measures, on
//! the exact path being measured. A summary costs one line a second and the
//! arithmetic that feeds it is a handful of integer adds per frame.
//!
//! ## Why a histogram
//!
//! The mean is the number that already looked fine — the last DEBUG session had
//! composite 5–10 ms and encode 6–19 ms against a 33 ms budget with
//! `over_budget=false` throughout, and it stuttered anyway. Whatever is wrong is
//! in the tail, so the tail is what has to be reported, and a percentile needs
//! the distribution. Keeping the samples in a `Vec` would grow for as long as
//! playback runs; [`Histogram`] is a fixed array that is reset every window, so
//! a ten-minute session costs exactly as much as a ten-second one.
//!
//! ## The rule this module holds itself to
//!
//! **The measurement must not perturb what it measures.** Concretely:
//!
//! - Nothing here allocates on the per-frame path.
//! - No lock is held across a `tracing` call. [`PlaybackStats::tick`] takes the
//!   lock, folds the window into a small `Copy` [`Summary`], resets, *drops the
//!   lock*, and returns the summary for the caller to emit.
//! - Nothing is formatted unless a line is actually emitted: the per-frame path
//!   only ever increments integers, and the divisions that turn them into
//!   milliseconds happen once per window.
//! - The mutex is a leaf. Nothing under it takes another lock, so taking it
//!   from inside the render thread's `work` guard cannot invert an order.

use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::clock::frame_interval;
use super::encoder::Backend;

/// How often a summary goes out while playback runs.
///
/// One a second: often enough that a two-second stall is bracketed by lines
/// either side of it, rare enough that the file stays readable and the writes
/// stay invisible.
pub const SUMMARY_INTERVAL: Duration = Duration::from_secs(1);

/// A seek slower than this is logged on its own.
///
/// Backward seeks were measured at 130–227 ms (`docs/STATUS.md`, "Seeking
/// backwards during playback stalls briefly"), which is four to seven frames at
/// 30 fps and is visible. 50 ms is comfortably above an ordinary forward seek
/// and comfortably below the ones that hurt.
pub const SLOW_SEEK: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------
// The distribution
// ---------------------------------------------------------------------------

/// Width of one histogram bucket, in microseconds.
pub const BUCKET_MICROS: i64 = 250;

/// How many buckets. 512 × 250 µs covers 0–128 ms, which is four frame budgets
/// at 30 fps; anything past that is an outlier and lands in the overflow, where
/// the exactly-tracked maximum describes it better than a bucket would.
pub const BUCKETS: usize = 512;

/// Frame times, at 0.25 ms resolution, in a fixed 2 KB array.
///
/// Percentiles are reported as the **upper** edge of the bucket a sample landed
/// in, so the answer is never optimistic: a reported p99 of 14.5 ms means the
/// 99th percentile is at most 14.5 ms and at least 14.25 ms.
#[derive(Debug)]
pub struct Histogram {
    counts: [u32; BUCKETS],
    /// Samples past the last bucket. Counted, not bucketed — [`Self::max`]
    /// describes them exactly and a coarser bucket would not.
    overflow: u32,
    count: u64,
    /// Sum of every sample, for the mean. `u64` of microseconds overflows after
    /// half a million years of frame times.
    total: u64,
    max: i64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            counts: [0; BUCKETS],
            overflow: 0,
            count: 0,
            total: 0,
            max: 0,
        }
    }
}

impl Histogram {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one sample. Negative durations cannot happen but clamp rather than
    /// panic, because a monotonic clock going backwards is not worth a crash in
    /// the preview.
    pub fn record(&mut self, micros: i64) {
        let micros = micros.max(0);
        self.count += 1;
        self.total += micros as u64;
        if micros > self.max {
            self.max = micros;
        }
        let bucket = (micros / BUCKET_MICROS) as usize;
        if bucket < BUCKETS {
            self.counts[bucket] += 1;
        } else {
            self.overflow += 1;
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn max(&self) -> i64 {
        self.max
    }

    /// Arithmetic mean, in microseconds. Zero when nothing was recorded — the
    /// division is guarded here rather than at every call site.
    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.total as f64 / self.count as f64
    }

    /// The `p`-th percentile in microseconds, by the nearest-rank method.
    ///
    /// Nearest rank rather than interpolation because the buckets are already
    /// the quantisation and interpolating between two of them would invent
    /// precision the data does not have. `p` is a fraction: 0.99 for p99.
    pub fn percentile(&self, p: f64) -> i64 {
        if self.count == 0 {
            return 0;
        }
        let p = p.clamp(0.0, 1.0);
        // Rank counts from 1: the 99th percentile of 100 samples is the 99th
        // smallest, not the 98th.
        let rank = ((p * self.count as f64).ceil() as u64).clamp(1, self.count);
        let mut seen: u64 = 0;
        for (index, hits) in self.counts.iter().enumerate() {
            seen += *hits as u64;
            if seen >= rank {
                return (index as i64 + 1) * BUCKET_MICROS;
            }
        }
        // The rank fell in the overflow, where the exact maximum is the best
        // answer available and is never an understatement.
        self.max
    }

    pub fn reset(&mut self) {
        // Written rather than reassigned so the 2 KB array is not rebuilt on
        // the stack and moved.
        self.counts.fill(0);
        self.overflow = 0;
        self.count = 0;
        self.total = 0;
        self.max = 0;
    }
}

// ---------------------------------------------------------------------------
// What the preview is doing, for the line that says so
// ---------------------------------------------------------------------------

/// Which decoder the preview's pixels are coming from.
///
/// **Inferred, not asked.** `media::provider::acceleration` is private and
/// `MediaSourceProvider` reaches the preview as an opaque `dyn SourceProvider`,
/// so there is nothing to ask. [`decode_path`] reproduces that function's rule
/// from the same three public inputs it uses — `CHUKCUT_DECODE`, the "Video
/// decoding" setting (`provider::decode_preference`) and
/// `RenderContext::can_import_dmabuf` — which is exact today and is the one
/// thing in this module that could silently go stale if that rule changes.
/// It is logged with its reason, so a log that disagrees with reality says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodePath {
    Vaapi,
    Software,
}

impl DecodePath {
    pub fn label(self) -> &'static str {
        match self {
            DecodePath::Vaapi => "vaapi",
            DecodePath::Software => "software",
        }
    }
}

/// The decode path this process will take, and why.
///
/// The reason is the useful half. "software" on a machine that has a working
/// VAAPI decoder is a 20× regression per frame (`docs/research/hardware-decode.md`)
/// and the difference between "the user asked for it" and "the GPU cannot
/// import a decoded surface" is the difference between a setting and a bug.
pub fn decode_path(can_import_dmabuf: bool) -> (DecodePath, &'static str) {
    use crate::modules::media::decoder::Acceleration;
    let setting = crate::modules::media::provider::decode_preference();
    match std::env::var("CHUKCUT_DECODE").as_deref() {
        Ok("software") => (DecodePath::Software, "CHUKCUT_DECODE=software"),
        Ok("vaapi") => (DecodePath::Vaapi, "CHUKCUT_DECODE=vaapi"),
        Ok("auto") if can_import_dmabuf => (DecodePath::Vaapi, "CHUKCUT_DECODE=auto"),
        Ok("auto") => (
            DecodePath::Vaapi,
            "CHUKCUT_DECODE=auto, and the GPU cannot import a decoded surface, \
             so every frame is copied out of tiled memory",
        ),
        _ if setting == Some(Acceleration::Software) => (
            DecodePath::Software,
            "Settings › Performance › Video decoding is Software",
        ),
        _ if setting == Some(Acceleration::Vaapi) => (
            DecodePath::Vaapi,
            "Settings › Performance › Video decoding is VAAPI",
        ),
        _ if can_import_dmabuf => (DecodePath::Vaapi, "the GPU can import a decoded surface"),
        _ => (
            DecodePath::Software,
            "the GPU cannot import a decoded surface (no VK_EXT_external_memory_dma_buf), \
             and hardware decode that has to reach system memory is slower than software",
        ),
    }
}

/// The facts about a session that every summary line repeats.
///
/// The decode path is deliberately *not* here. It is a property of the process
/// rather than of the session, it is only knowable once the render thread has a
/// device, and that thread starts in parallel with the first session — so it
/// arrives through [`PlaybackStats::set_decode_path`] whenever it is learnt,
/// and a line raised before then says `unknown` rather than guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionFacts {
    /// What the preview actually renders at.
    pub width: u32,
    pub height: u32,
    /// What the project says it is. Differs from the above when the proxy table
    /// or the device capped it.
    pub canvas: (u32, u32),
}

impl SessionFacts {
    /// Whether the preview is rendering below the project's canvas.
    ///
    /// Reported because a preview at half the canvas and a preview at the
    /// canvas are two different performance regimes, and "it stutters" means
    /// something different in each.
    ///
    /// Deliberately *not* called `proxy`: `modules::proxy` is proxy **media**,
    /// small stand-in files for footage this machine cannot decode at full
    /// resolution, and it is not wired into the preview at all yet. This is
    /// only the render resolution against the canvas. When proxy media does
    /// reach the preview it belongs in [`DecodePath`] as a third value, not
    /// here.
    pub fn downscaled(&self) -> bool {
        (self.width, self.height) != self.canvas
    }
}

/// One window of playback, folded into the numbers that go on one line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// Frames the frontend was told to display. The pacer's count, so this is
    /// what the user saw and not what the renderer made.
    pub shown: u64,
    /// Frames abandoned because the renderer fell behind the clock.
    ///
    /// Counted in `pace`, before anything was spent on them.
    pub dropped: u64,
    /// Frames composited and then thrown away without a JPEG, because the
    /// playhead had already passed them by the time the compositor was done.
    ///
    /// **Not the same as `dropped` and deliberately not folded into it.** A
    /// dropped frame cost nothing; a discarded one cost a whole composite and
    /// saved only the encode. `discarded` climbing is the renderer finishing
    /// frames that are already too late to show, which is a different failure
    /// from never starting them.
    pub discarded: u64,
    /// Frames composited and encoded for playback in this window.
    ///
    /// `shown` far above `rendered` is the signature of a stalled renderer: the
    /// clock kept moving and the same picture stayed on screen.
    pub rendered: u64,
    /// Frames rendered because the playhead moved — a scrub, a seek, a pause.
    /// Not part of the frame-time distribution; they are a different workload
    /// at a different quality.
    pub scrubs: u64,
    /// How many of `rendered` took longer than the frame budget.
    pub over_budget: u64,
    pub mean_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub budget_ms: f64,
    /// Wall time this window covers. Not assumed to be [`SUMMARY_INTERVAL`]: a
    /// window that ran long is itself the evidence.
    pub window_ms: f64,
    /// `None` until the render thread has a device to ask.
    pub decode: Option<DecodePath>,
    /// `None` when nothing was encoded in this window.
    pub encode: Option<Backend>,
    pub width: u32,
    pub height: u32,
    /// The preview is rendering below the canvas. See
    /// [`SessionFacts::downscaled`] for why it is not called `proxy`.
    pub downscaled: bool,
    /// Where playback is on the quality ladder: 0 is the paused frame's own
    /// size and quality, higher rungs are what was given up to keep up. See
    /// [`super::ladder`].
    pub rung: u8,
    /// What a playback frame was actually rendered at in this window, which is
    /// `width`/`height` shrunk by the rung. Equal to them at rung 0.
    pub render_width: u32,
    pub render_height: u32,
    /// The JPEG quality playback frames were encoded at.
    pub render_quality: u8,
}

impl Summary {
    /// Whether this window is worth a line at all. A window in which nothing
    /// happened is noise, and one that never existed is a division by zero
    /// waiting to be printed.
    pub fn is_empty(&self) -> bool {
        self.shown == 0
            && self.rendered == 0
            && self.dropped == 0
            && self.discarded == 0
            && self.scrubs == 0
    }
}

/// Whether a line is a periodic one or the last one of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Tick,
    Stopped,
}

/// The one place a summary becomes a log line.
///
/// Called with no lock held, always — see the module header.
pub fn emit(summary: &Summary, reason: Reason) {
    let message = match reason {
        Reason::Tick => "preview playback",
        Reason::Stopped => "preview playback stopped",
    };
    tracing::info!(
        shown = summary.shown,
        dropped = summary.dropped,
        discarded = summary.discarded,
        rendered = summary.rendered,
        scrubs = summary.scrubs,
        over_budget = summary.over_budget,
        mean_ms = summary.mean_ms,
        p99_ms = summary.p99_ms,
        max_ms = summary.max_ms,
        budget_ms = summary.budget_ms,
        window_ms = summary.window_ms,
        decode = summary.decode.map(DecodePath::label).unwrap_or("unknown"),
        encode = summary.encode.map(Backend::label).unwrap_or("none"),
        width = summary.width,
        height = summary.height,
        downscaled = summary.downscaled,
        rung = summary.rung,
        ladder = super::ladder::rung_label(summary.rung as usize),
        render_width = summary.render_width,
        render_height = summary.render_height,
        render_quality = summary.render_quality,
        "{message}"
    );
}

/// What the quality ladder turned one playback frame into.
///
/// Reported per frame rather than read from the ladder when a line is emitted,
/// because the ladder can move between the frame and the line and the line has
/// to describe frames that were actually rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rendered {
    pub rung: u8,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
}

/// The JPEG encoder changed backend mid-session.
///
/// Worth one line because the software path is 5× the hardware one on a
/// 1080x1920 frame (31 ms against 6.2 ms) and `encode_preview_jpeg` falls back
/// *silently*: an odd frame size takes it on every machine, since NV12 cannot
/// represent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeFallback {
    pub from: Backend,
    pub to: Backend,
}

// ---------------------------------------------------------------------------
// The accumulator
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Inner {
    window_started: Instant,
    shown: u64,
    dropped: u64,
    discarded: u64,
    scrubs: u64,
    /// The ladder rung and what it turned the session's size and quality into,
    /// as of the last playback frame. Zero-sized until one is rendered, which
    /// is why [`Inner::summarise`] falls back to the session's own size.
    rung: u8,
    render: (u32, u32),
    render_quality: u8,
    over_budget: u64,
    frames: Histogram,
    budget_micros: i64,
    facts: SessionFacts,
    decode: Option<DecodePath>,
    encode: Option<Backend>,
    /// A backend change is worth one line, not one per frame.
    fallback_reported: bool,
    /// [`SUMMARY_INTERVAL`], except in a test that needs one window to hold
    /// a whole run (see [`PlaybackStats::set_summary_interval`]).
    interval: Duration,
}

impl Inner {
    fn summarise(&self, now: Instant) -> Summary {
        Summary {
            shown: self.shown,
            dropped: self.dropped,
            discarded: self.discarded,
            rendered: self.frames.count(),
            scrubs: self.scrubs,
            over_budget: self.over_budget,
            mean_ms: self.frames.mean() / 1000.0,
            p99_ms: self.frames.percentile(0.99) as f64 / 1000.0,
            max_ms: self.frames.max() as f64 / 1000.0,
            budget_ms: self.budget_micros as f64 / 1000.0,
            window_ms: now
                .saturating_duration_since(self.window_started)
                .as_secs_f64()
                * 1000.0,
            decode: self.decode,
            encode: self.encode,
            width: self.facts.width,
            height: self.facts.height,
            downscaled: self.facts.downscaled(),
            rung: self.rung,
            // Before the first playback frame of a window there is no rung to
            // report, and the session's own size is the truthful answer.
            render_width: if self.render.0 > 0 {
                self.render.0
            } else {
                self.facts.width
            },
            render_height: if self.render.1 > 0 {
                self.render.1
            } else {
                self.facts.height
            },
            render_quality: self.render_quality,
        }
    }

    fn roll(&mut self, now: Instant) {
        self.window_started = now;
        self.shown = 0;
        self.dropped = 0;
        self.discarded = 0;
        self.scrubs = 0;
        self.over_budget = 0;
        self.frames.reset();
        // `rung`, `render` and `render_quality` deliberately survive a window,
        // for the same reason `encode` does: they describe what playback is
        // doing, not what happened in one second of it.
        // `encode` and `fallback_reported` deliberately survive a window: which
        // encoder is in use is a property of the session, not of the second.
    }
}

/// Everything the three preview threads count, behind one leaf mutex.
///
/// One mutex and not three atomics because a summary has to be a *consistent*
/// window — counts read one at a time from atomics can straddle a reset and
/// report a p99 that belongs to a different second than the frame count next to
/// it. The lock is taken for a few integer adds at 30 Hz from three threads,
/// which is not a contention story.
#[derive(Debug)]
pub struct PlaybackStats {
    inner: Mutex<Inner>,
}

impl Default for PlaybackStats {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackStats {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                window_started: Instant::now(),
                shown: 0,
                dropped: 0,
                discarded: 0,
                scrubs: 0,
                rung: 0,
                render: (0, 0),
                render_quality: 0,
                over_budget: 0,
                frames: Histogram::new(),
                budget_micros: frame_interval(30.0),
                facts: SessionFacts::default(),
                decode: None,
                encode: None,
                fallback_reported: false,
                interval: SUMMARY_INTERVAL,
            }),
        }
    }

    /// Record which decoder the pixels are coming from.
    ///
    /// Called once, by the render thread, as soon as it has a device. Kept
    /// across sessions because it is a process-wide choice: `media` opens
    /// decoders per material but decides how to open them once.
    pub fn set_decode_path(&self, path: DecodePath) {
        self.inner.lock().decode = Some(path);
    }

    /// Adopt a session's facts and start a fresh window.
    ///
    /// `now` is injected so the cadence is testable without sleeping. A test
    /// that has to sleep for a second to check a one-second cadence is a test
    /// nobody runs.
    pub fn begin_session(&self, facts: SessionFacts, fps: f64, now: Instant) {
        let mut inner = self.inner.lock();
        inner.facts = facts;
        inner.budget_micros = frame_interval(fps);
        inner.encode = None;
        inner.fallback_reported = false;
        // A new session is a new ladder — the server resets it — so a line
        // raised before the first frame of it must not claim the old rung.
        inner.rung = 0;
        inner.render = (0, 0);
        inner.render_quality = 0;
        inner.roll(now);
    }

    /// Throw the current window away and start a new one.
    ///
    /// Called when playback starts. A session can sit parked for a minute
    /// before anyone presses play, and without this the first line of that
    /// playback would claim to cover the minute of sitting still.
    pub fn start_window(&self, now: Instant) {
        self.inner.lock().roll(now);
    }

    /// The session's facts, for a caller that wants to log them itself.
    pub fn facts(&self) -> SessionFacts {
        self.inner.lock().facts
    }

    /// One frame composited and encoded.
    ///
    /// `micros` is the cost of the slowest of the two halves, not their sum:
    /// compositing and encoding run on different threads and overlap, so what
    /// has to fit in the budget is the larger of them.
    ///
    /// Returns the backend change to log, if this frame is where it happened.
    /// Returned rather than logged so the `tracing` call happens after the lock
    /// is dropped.
    ///
    /// `rendered` is what the ladder turned this frame into, and is `None` for
    /// a scrub because a scrub never goes through the ladder. Passed here
    /// rather than through a setter of its own so a frame costs one lock.
    #[must_use]
    pub fn record_frame(
        &self,
        scrub: bool,
        micros: i64,
        backend: Backend,
        rendered: Option<Rendered>,
    ) -> Option<EncodeFallback> {
        let mut inner = self.inner.lock();
        let change = match inner.encode {
            Some(previous) if previous != backend && !inner.fallback_reported => {
                inner.fallback_reported = true;
                Some(EncodeFallback {
                    from: previous,
                    to: backend,
                })
            }
            _ => None,
        };
        inner.encode = Some(backend);

        if scrub {
            inner.scrubs += 1;
            return change;
        }
        if let Some(rendered) = rendered {
            inner.rung = rendered.rung;
            inner.render = (rendered.width, rendered.height);
            inner.render_quality = rendered.quality;
        }
        inner.frames.record(micros);
        if micros > inner.budget_micros {
            inner.over_budget += 1;
        }
        change
    }

    /// Frames the renderer abandoned because the clock had passed them.
    ///
    /// Counted where `pace` decides, i.e. before anything has been spent on
    /// them. A frame that was composited and *then* found to be too late is
    /// [`Self::record_discarded`] instead, so the two never count the same
    /// frame twice.
    pub fn record_dropped(&self, frames: i64) {
        if frames <= 0 {
            return;
        }
        self.inner.lock().dropped += frames as u64;
    }

    /// Frames composited and then thrown away without being encoded.
    ///
    /// Distinct from [`Self::record_dropped`] because they cost different
    /// things and mean different things: a dropped frame was never started, a
    /// discarded one paid for a whole composite and saved only the JPEG.
    pub fn record_discarded(&self, frames: i64) {
        if frames <= 0 {
            return;
        }
        self.inner.lock().discarded += frames as u64;
    }

    /// One frame announced to the frontend.
    pub fn record_shown(&self) {
        self.inner.lock().shown += 1;
    }

    /// A summary if the window is up, and a fresh window if so.
    ///
    /// Returns `None` — and does not roll the window — when nothing has
    /// happened, so a session that is open but idle does not write a line a
    /// second saying nothing.
    #[must_use]
    pub fn tick(&self, now: Instant) -> Option<Summary> {
        let mut inner = self.inner.lock();
        if now.saturating_duration_since(inner.window_started) < inner.interval {
            return None;
        }
        let summary = inner.summarise(now);
        if summary.is_empty() {
            // Still roll: an idle window that stays open would make the next
            // real one look like it covered ten seconds.
            inner.roll(now);
            return None;
        }
        inner.roll(now);
        // The lock is dropped by returning. Nothing is formatted here.
        Some(summary)
    }

    /// How often [`Self::tick`] closes a window; [`SUMMARY_INTERVAL`] unless
    /// changed.
    ///
    /// For tests that count every frame of a run in one summary. The pacer
    /// ticks on the wall clock, so on a slow adapter (lavapipe renders a few
    /// frames a second under load) a run of twelve frames spans more than one
    /// interval, a tick drains the first frames into a log line, and the
    /// final summary sees only the rest: "counted 8 frames against 12".
    #[cfg(test)]
    pub fn set_summary_interval(&self, interval: Duration) {
        self.inner.lock().interval = interval;
    }

    /// The last summary of a run, whatever the window looks like.
    ///
    /// `None` when nothing happened — a session that opened and closed without
    /// rendering a frame has nothing to say, and saying it would mean printing
    /// a mean of zero over zero frames as though it were a measurement.
    #[must_use]
    pub fn finish(&self, now: Instant) -> Option<Summary> {
        let mut inner = self.inner.lock();
        let summary = inner.summarise(now);
        inner.roll(now);
        if summary.is_empty() {
            return None;
        }
        Some(summary)
    }
}

// ---------------------------------------------------------------------------
// Seeks
// ---------------------------------------------------------------------------

/// Why the playhead moved. A cold session and a seek cost different things and
/// only one of them is a bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekKind {
    /// A new session on a document, i.e. a cold decoder.
    SessionStart,
    /// The playhead moved within a document already open.
    Seek,
}

impl SeekKind {
    pub fn label(self) -> &'static str {
        match self {
            SeekKind::SessionStart => "session start",
            SeekKind::Seek => "seek",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    session: u64,
    frame: i64,
    from: i64,
    to: i64,
    kind: SeekKind,
    started: Instant,
}

/// How long the picture took to catch up with a seek.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlowSeek {
    pub kind: SeekKind,
    /// Where the playhead was, in microseconds.
    pub from: i64,
    pub to: i64,
    /// Negative when the seek went backwards, which is the expensive direction:
    /// the ring is discarded and the decoder does a real seek.
    pub delta: i64,
    pub took: Duration,
}

impl SlowSeek {
    pub fn backwards(&self) -> bool {
        self.delta < 0
    }
}

/// The clock on "the user moved the playhead" → "there is a picture there".
///
/// This, and not the duration of `PreviewServer::seek`, is what the user
/// experiences. The command itself returns in microseconds — it supersedes the
/// session and queues a frame — while the cost that shows up as a freeze is the
/// decoder seek that follows, on another thread. Measuring the command would
/// have reported that seeking is instant.
#[derive(Debug, Default)]
pub struct SeekWatch {
    pending: Mutex<Option<Pending>>,
}

impl SeekWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start the clock. Replaces any seek still outstanding: the user moved
    /// again, so the previous one's frame will never be shown and timing it
    /// would measure something nobody waited for.
    pub fn arm(&self, session: u64, frame: i64, from: i64, to: i64, kind: SeekKind, now: Instant) {
        *self.pending.lock() = Some(Pending {
            session,
            frame,
            from,
            to,
            kind,
            started: now,
        });
    }

    /// A frame landed. Returns a [`SlowSeek`] if it is the one being waited on
    /// and it took longer than [`SLOW_SEEK`].
    ///
    /// Returned rather than logged, so the `tracing` call is outside the lock.
    #[must_use]
    pub fn complete(&self, session: u64, frame: i64, now: Instant) -> Option<SlowSeek> {
        let mut pending = self.pending.lock();
        let waiting = (*pending)?;
        if waiting.session != session || waiting.frame != frame {
            return None;
        }
        *pending = None;
        drop(pending);

        let took = now.saturating_duration_since(waiting.started);
        if took < SLOW_SEEK {
            return None;
        }
        Some(SlowSeek {
            kind: waiting.kind,
            from: waiting.from,
            to: waiting.to,
            delta: waiting.to - waiting.from,
            took,
        })
    }

    pub fn clear(&self) {
        *self.pending.lock() = None;
    }
}

/// The one place a slow seek becomes a log line.
pub fn emit_slow_seek(slow: &SlowSeek) {
    tracing::info!(
        kind = slow.kind.label(),
        from_ms = slow.from / 1000,
        to_ms = slow.to / 1000,
        backwards = slow.backwards(),
        took_ms = slow.took.as_secs_f64() * 1000.0,
        "preview seek was slow"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `MakeWriter` that keeps what was written, so a test can assert on the
    /// line a user would actually read rather than on the struct behind it.
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for Captured {
        type Writer = Captured;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `body` against a subscriber filtered exactly as the log **file** is,
    /// and return what reached it. If a line does not appear here it does not
    /// appear on the owner's disk either, which is the whole point.
    fn through_the_file_filter(body: impl FnOnce()) -> String {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::Layer;

        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(captured.clone())
                // The constant from `workspace::logging::FILE_FILTER`, spelled
                // out rather than imported so this test fails loudly if that
                // module ever narrows it.
                .with_filter(tracing_subscriber::EnvFilter::new("chukcut=info,warn")),
        );
        tracing::subscriber::with_default(subscriber, body);
        captured.text()
    }

    fn facts() -> SessionFacts {
        SessionFacts {
            width: 1080,
            height: 1920,
            canvas: (1080, 1920),
        }
    }

    // -----------------------------------------------------------------------
    // The distribution
    // -----------------------------------------------------------------------

    /// A known distribution with a known answer: 1 ms through 100 ms, one
    /// sample each. The 99th percentile of a hundred samples is the 99th
    /// smallest, which is 99 ms.
    #[test]
    fn percentiles_are_right_for_a_distribution_whose_answer_is_known() {
        let mut hist = Histogram::new();
        for ms in 1..=100 {
            hist.record(ms * 1000);
        }

        assert_eq!(hist.count(), 100);
        assert_eq!(hist.max(), 100_000);
        // 1+2+…+100 = 5050, over 100 samples.
        assert_eq!(hist.mean(), 50_500.0);

        // Reported as the bucket's upper edge, so the answer is the smallest
        // multiple of BUCKET_MICROS strictly above the true value.
        assert_eq!(hist.percentile(0.99), 99_250, "p99 of 1..=100 ms is 99 ms");
        assert_eq!(hist.percentile(0.50), 50_250, "p50 is 50 ms");
        assert_eq!(hist.percentile(1.0), 100_250, "p100 is the largest sample");
        assert_eq!(hist.percentile(0.0), 1_250, "p0 is the smallest sample");

        // Never optimistic, and never off by more than one bucket.
        for (p, truth) in [(0.99, 99_000.0), (0.5, 50_000.0), (0.9, 90_000.0)] {
            let got = hist.percentile(p) as f64;
            assert!(
                got >= truth && got - truth <= BUCKET_MICROS as f64,
                "p{p} reported {got} against a true {truth}"
            );
        }
    }

    /// The case the whole histogram exists for: a mean well inside the budget
    /// with a tail well outside it. Ninety-nine frames at 8 ms and one at
    /// 400 ms is a mean of 11.9 ms — "fine" — and a stutter the user saw.
    #[test]
    fn a_tail_the_mean_hides_is_visible_in_p99_and_max() {
        let mut hist = Histogram::new();
        for _ in 0..99 {
            hist.record(8_000);
        }
        hist.record(400_000);

        assert!(
            hist.mean() < 12_000.0,
            "the mean looks healthy: {}",
            hist.mean()
        );
        assert_eq!(hist.max(), 400_000, "the outlier is reported exactly");
        // The outlier is past the last bucket, so the rank falls in the
        // overflow and the exact maximum is the answer.
        assert_eq!(hist.percentile(0.995), 400_000);
    }

    #[test]
    fn an_empty_histogram_answers_zero_rather_than_dividing_by_zero() {
        let hist = Histogram::new();
        assert_eq!(hist.count(), 0);
        assert_eq!(hist.mean(), 0.0);
        assert!(hist.mean().is_finite(), "not a NaN");
        assert_eq!(hist.percentile(0.99), 0);
        assert_eq!(hist.max(), 0);
    }

    #[test]
    fn samples_past_the_last_bucket_are_counted_and_the_maximum_is_exact() {
        let mut hist = Histogram::new();
        let past_the_end = BUCKETS as i64 * BUCKET_MICROS + 5_000;
        hist.record(1_000);
        hist.record(past_the_end);

        assert_eq!(hist.count(), 2, "an overflowing sample still counts");
        assert_eq!(hist.max(), past_the_end, "and is measured exactly");
        assert_eq!(hist.mean(), (1_000 + past_the_end) as f64 / 2.0);
        assert_eq!(hist.percentile(1.0), past_the_end);
    }

    #[test]
    fn resetting_clears_every_number_and_not_only_the_count() {
        let mut hist = Histogram::new();
        hist.record(5_000);
        hist.record(50_000);
        hist.reset();
        assert_eq!(hist.count(), 0);
        assert_eq!(hist.max(), 0);
        assert_eq!(hist.mean(), 0.0);
        assert_eq!(hist.percentile(0.99), 0);
        // And it still works afterwards.
        hist.record(2_000);
        assert_eq!(hist.count(), 1);
        assert_eq!(hist.percentile(0.5), 2_250);
    }

    // -----------------------------------------------------------------------
    // Cadence
    // -----------------------------------------------------------------------

    #[test]
    fn a_summary_goes_out_once_per_interval_and_not_before() {
        let start = Instant::now();
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, start);
        stats.record_shown();
        assert!(stats
            .record_frame(false, 10_000, Backend::Vaapi, None)
            .is_none());

        assert!(stats.tick(start).is_none(), "nothing at the very start");
        assert!(
            stats.tick(start + Duration::from_millis(999)).is_none(),
            "nothing a millisecond early"
        );

        let first = stats
            .tick(start + Duration::from_millis(1_001))
            .expect("a summary once the interval is up");
        assert_eq!(first.shown, 1);
        assert_eq!(first.rendered, 1);
        assert!(
            (first.window_ms - 1_001.0).abs() < 1.0,
            "the window is the one that just closed: {}",
            first.window_ms
        );

        // The window restarted, so the clock is measured from the tick and not
        // from the session.
        stats.record_shown();
        assert!(
            stats.tick(start + Duration::from_millis(1_900)).is_none(),
            "the second window is not up yet"
        );
        let second = stats
            .tick(start + Duration::from_millis(2_100))
            .expect("a second summary one interval later");
        assert_eq!(second.shown, 1, "counters reset with the window");
        assert_eq!(second.rendered, 0);
        assert!(
            (second.window_ms - 1_099.0).abs() < 1.0,
            "measured from the previous tick: {}",
            second.window_ms
        );
    }

    /// An open session that is doing nothing writes nothing. A line a second
    /// saying "0 frames" would bury the ones that mean something.
    #[test]
    fn an_idle_window_produces_no_line_but_still_rolls() {
        let start = Instant::now();
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, start);

        assert!(stats.tick(start + Duration::from_millis(1_100)).is_none());
        assert!(stats.tick(start + Duration::from_millis(2_200)).is_none());

        // Rolling matters: the next real window must not claim to cover the
        // three seconds of silence before it.
        stats.record_shown();
        let summary = stats
            .tick(start + Duration::from_millis(3_300))
            .expect("a window with something in it");
        assert!(
            summary.window_ms < 1_200.0,
            "the idle windows were rolled, not accumulated: {}",
            summary.window_ms
        );
    }

    // -----------------------------------------------------------------------
    // The numbers on the line
    // -----------------------------------------------------------------------

    #[test]
    fn the_summary_counts_what_the_line_claims_it_counts() {
        let start = Instant::now();
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, start);
        stats.set_decode_path(DecodePath::Vaapi);

        // 30 fps: the budget is 33333 µs.
        for _ in 0..8 {
            let _ = stats.record_frame(false, 10_000, Backend::Vaapi, None);
        }
        let _ = stats.record_frame(false, 40_000, Backend::Vaapi, None);
        let _ = stats.record_frame(false, 90_000, Backend::Vaapi, None);
        // Scrubs are a different workload and stay out of the distribution.
        let _ = stats.record_frame(true, 200_000, Backend::Vaapi, None);
        for _ in 0..12 {
            stats.record_shown();
        }
        stats.record_dropped(3);
        stats.record_dropped(0);

        let summary = stats
            .tick(start + Duration::from_millis(1_000))
            .expect("a summary");

        assert_eq!(summary.rendered, 10, "the scrub is not a playback frame");
        assert_eq!(summary.scrubs, 1);
        assert_eq!(summary.shown, 12);
        assert_eq!(summary.dropped, 3);
        assert_eq!(summary.over_budget, 2, "40 ms and 90 ms exceed 33.3 ms");
        assert!(
            (summary.budget_ms - 33.333).abs() < 0.01,
            "budget {}",
            summary.budget_ms
        );
        // (8×10 + 40 + 90) / 10 = 21 ms.
        assert!(
            (summary.mean_ms - 21.0).abs() < 0.001,
            "mean {}",
            summary.mean_ms
        );
        assert!(
            (summary.max_ms - 90.0).abs() < 0.001,
            "the worst frame is reported exactly: {}",
            summary.max_ms
        );
        // The 10th of 10 samples.
        assert!(
            summary.p99_ms >= 90.0 && summary.p99_ms <= 90.25,
            "p99 {}",
            summary.p99_ms
        );
        assert_eq!(summary.width, 1080);
        assert_eq!(summary.height, 1920);
        assert!(!summary.downscaled, "canvas and preview are the same size");
        assert_eq!(summary.decode, Some(DecodePath::Vaapi));
        assert_eq!(summary.encode, Some(Backend::Vaapi));
    }

    #[test]
    fn a_preview_below_the_canvas_says_so() {
        let stats = PlaybackStats::new();
        stats.begin_session(
            SessionFacts {
                width: 1920,
                height: 1080,
                canvas: (3840, 2160),
            },
            30.0,
            Instant::now(),
        );
        stats.set_decode_path(DecodePath::Software);
        stats.record_shown();
        let summary = stats.finish(Instant::now()).expect("a summary");
        assert!(summary.downscaled, "1920x1080 of a 4K canvas is downscaled");
        assert_eq!(summary.decode, Some(DecodePath::Software));
    }

    #[test]
    fn the_line_says_what_the_quality_ladder_did() {
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, Instant::now());

        // Before the ladder has done anything the line describes the session
        // itself rather than guessing.
        stats.record_shown();
        let idle = stats.finish(Instant::now()).expect("a summary");
        assert_eq!(idle.rung, 0);
        assert_eq!(
            (idle.render_width, idle.render_height),
            (idle.width, idle.height)
        );

        stats.begin_session(facts(), 30.0, Instant::now());
        let _ = stats.record_frame(
            false,
            10_000,
            Backend::Vaapi,
            Some(Rendered {
                rung: 1,
                width: 810,
                height: 1440,
                quality: 80,
            }),
        );
        // A scrub is not a playback frame and must not be able to claim the
        // paused frame was rendered at a lower rung.
        let _ = stats.record_frame(true, 20_000, Backend::Vaapi, None);

        let summary = stats.finish(Instant::now()).expect("a summary");
        assert_eq!(summary.rung, 1);
        assert_eq!((summary.render_width, summary.render_height), (810, 1440));
        assert_eq!(summary.render_quality, 80);
        assert_eq!(
            (summary.width, summary.height),
            (facts().width, facts().height),
            "the session's own size is still what a paused frame gets"
        );
    }

    #[test]
    fn a_discarded_frame_is_counted_apart_from_a_dropped_one() {
        // They cost different things — a dropped frame was never started, a
        // discarded one paid for a whole composite — so folding them together
        // would hide which of the two is happening.
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, Instant::now());
        stats.record_dropped(4);
        stats.record_discarded(1);
        stats.record_discarded(0);
        stats.record_discarded(-3);

        let summary = stats.finish(Instant::now()).expect("a summary");
        assert_eq!(summary.dropped, 4);
        assert_eq!(summary.discarded, 1);
        assert_eq!(summary.rendered, 0, "neither of them was ever encoded");
        assert!(
            !summary.is_empty(),
            "a window of nothing but discards still says so"
        );
    }

    /// The decode path outlives a session, because it is a property of the
    /// process; the frame counts do not.
    #[test]
    fn the_decode_path_survives_a_new_session_and_the_counters_do_not() {
        let start = Instant::now();
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, start);
        assert_eq!(
            stats.finish(start).map(|s| s.decode),
            None,
            "nothing happened, so no line at all"
        );

        stats.set_decode_path(DecodePath::Vaapi);
        stats.record_shown();
        stats.begin_session(facts(), 30.0, start);
        stats.record_shown();

        let summary = stats.finish(start).expect("a summary");
        assert_eq!(summary.shown, 1, "a new session starts the counters again");
        assert_eq!(
            summary.decode,
            Some(DecodePath::Vaapi),
            "but not the decode path"
        );
    }

    // -----------------------------------------------------------------------
    // The empty session
    // -----------------------------------------------------------------------

    /// The case that would otherwise print `NaN`: a session that opened, played
    /// nothing, and stopped.
    #[test]
    fn a_session_with_no_frames_emits_nothing_and_divides_by_nothing() {
        let start = Instant::now();
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, start);

        assert!(stats.finish(start + Duration::from_secs(5)).is_none());
        assert!(
            stats.tick(start + Duration::from_secs(6)).is_none(),
            "and a tick over an empty window says nothing either"
        );
    }

    /// Belt and braces: even if a summary over nothing were built, every field
    /// on it is a finite number rather than a NaN or an infinity.
    #[test]
    fn a_summary_over_no_frames_is_finite_in_every_field() {
        let start = Instant::now();
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, start);
        // One shown frame and no rendered ones — a stalled renderer, which is
        // exactly the state the summary exists to make visible.
        stats.record_shown();

        let summary = stats
            .finish(start + Duration::from_secs(1))
            .expect("a summary");
        assert_eq!(summary.rendered, 0);
        assert_eq!(summary.shown, 1);
        for (name, value) in [
            ("mean", summary.mean_ms),
            ("p99", summary.p99_ms),
            ("max", summary.max_ms),
            ("budget", summary.budget_ms),
            ("window", summary.window_ms),
        ] {
            assert!(value.is_finite(), "{name}_ms is {value}");
        }
        assert_eq!(summary.mean_ms, 0.0);
        assert_eq!(
            summary.encode, None,
            "nothing encoded, so no encoder to name"
        );
    }

    #[test]
    fn a_nonsense_frame_rate_does_not_produce_an_infinite_budget() {
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 0.0, Instant::now());
        let _ = stats.record_frame(false, 1_000, Backend::Software, None);
        let summary = stats.finish(Instant::now()).expect("a summary");
        assert!(summary.budget_ms.is_finite());
        assert!(
            (summary.budget_ms - 33.333).abs() < 0.01,
            "falls back to 30 fps"
        );
    }

    // -----------------------------------------------------------------------
    // Fallbacks
    // -----------------------------------------------------------------------

    #[test]
    fn an_encoder_that_falls_back_is_reported_once_and_not_per_frame() {
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, Instant::now());

        assert_eq!(
            stats.record_frame(false, 6_000, Backend::Vaapi, None),
            None,
            "the first frame establishes the backend rather than reporting a change"
        );
        assert_eq!(
            stats.record_frame(false, 31_000, Backend::Software, None),
            Some(EncodeFallback {
                from: Backend::Vaapi,
                to: Backend::Software,
            })
        );
        assert_eq!(
            stats.record_frame(false, 31_000, Backend::Software, None),
            None,
            "one line, not one per frame"
        );
        assert_eq!(
            stats.record_frame(false, 6_000, Backend::Vaapi, None),
            None,
            "and not again when it recovers"
        );
    }

    #[test]
    fn a_new_session_may_report_a_fallback_again() {
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, Instant::now());
        let _ = stats.record_frame(false, 6_000, Backend::Vaapi, None);
        let _ = stats.record_frame(false, 31_000, Backend::Software, None);

        stats.begin_session(facts(), 30.0, Instant::now());
        let _ = stats.record_frame(false, 6_000, Backend::Vaapi, None);
        assert!(
            stats
                .record_frame(false, 31_000, Backend::Software, None)
                .is_some(),
            "a fresh session has not reported anything yet"
        );
    }

    #[test]
    fn the_decode_path_is_derived_the_way_the_media_module_derives_it() {
        // The device answer, with the environment silent. `CHUKCUT_DECODE` is
        // read here rather than cached, so a test process that has it set would
        // see its own environment — assert on the pair rather than the value.
        let set = std::env::var("CHUKCUT_DECODE").is_ok();
        if !set {
            assert_eq!(decode_path(true).0, DecodePath::Vaapi);
            assert_eq!(
                decode_path(false).0,
                DecodePath::Software,
                "a GPU that cannot import a surface means software decode"
            );
            assert!(
                decode_path(false).1.contains("import"),
                "and the reason says why: {}",
                decode_path(false).1
            );
        }
    }

    // -----------------------------------------------------------------------
    // The line itself
    // -----------------------------------------------------------------------

    /// The requirement this module exists for: the summary reaches a log file
    /// filtered at INFO, carrying every number needed to characterise a
    /// stutter, without anybody having set `RUST_LOG`.
    #[test]
    fn the_summary_reaches_a_log_filtered_the_way_the_file_is() {
        let start = Instant::now();
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, start);
        stats.set_decode_path(DecodePath::Vaapi);
        for _ in 0..29 {
            let _ = stats.record_frame(false, 9_000, Backend::Vaapi, None);
        }
        let _ = stats.record_frame(false, 120_000, Backend::Vaapi, None);
        for _ in 0..30 {
            stats.record_shown();
        }
        stats.record_dropped(2);
        let summary = stats
            .tick(start + Duration::from_millis(1_000))
            .expect("a summary");

        let written = through_the_file_filter(|| emit(&summary, Reason::Tick));

        assert!(
            written.contains("preview playback"),
            "the message: {written:?}"
        );
        assert!(written.contains("INFO"), "at INFO, not DEBUG: {written:?}");
        for field in [
            "shown=30",
            "dropped=2",
            "rendered=30",
            "over_budget=1",
            "budget_ms=33.333",
            "decode=\"vaapi\"",
            "encode=\"vaapi\"",
            "width=1080",
            "height=1920",
            "downscaled=false",
        ] {
            assert!(written.contains(field), "missing {field} in {written:?}");
        }
        // The tail is the number the mean was hiding, and it has to be on the
        // line: 29 frames at 9 ms and one at 120 ms is a mean of 12.7 ms.
        assert!(
            written.contains("max_ms=120"),
            "the worst frame: {written:?}"
        );
        assert!(written.contains("mean_ms=12.7"), "the mean: {written:?}");
        assert!(written.contains("p99_ms=120"), "the tail: {written:?}");
    }

    #[test]
    fn a_final_summary_is_distinguishable_from_a_periodic_one() {
        let stats = PlaybackStats::new();
        stats.begin_session(facts(), 30.0, Instant::now());
        stats.record_shown();
        let summary = stats.finish(Instant::now()).expect("a summary");

        let written = through_the_file_filter(|| emit(&summary, Reason::Stopped));
        assert!(written.contains("preview playback stopped"), "{written:?}");
        // A grep for the periodic message finds the final one too, which is
        // what makes "paste every `preview playback` line" a usable request.
        assert!(written.contains("preview playback"), "{written:?}");
    }

    #[test]
    fn a_slow_seek_reaches_the_file_too() {
        let slow = SlowSeek {
            kind: SeekKind::Seek,
            from: 8_000_000,
            to: 2_000_000,
            delta: -6_000_000,
            took: Duration::from_millis(214),
        };
        let written = through_the_file_filter(|| emit_slow_seek(&slow));
        assert!(written.contains("preview seek was slow"), "{written:?}");
        assert!(written.contains("INFO"), "{written:?}");
        assert!(written.contains("backwards=true"), "{written:?}");
        assert!(written.contains("took_ms=214"), "{written:?}");
        assert!(written.contains("from_ms=8000"), "{written:?}");
        assert!(written.contains("to_ms=2000"), "{written:?}");
    }

    // -----------------------------------------------------------------------
    // Seeks
    // -----------------------------------------------------------------------

    #[test]
    fn a_slow_seek_is_reported_and_a_fast_one_is_not() {
        let start = Instant::now();
        let watch = SeekWatch::new();

        watch.arm(7, 30, 2_000_000, 1_000_000, SeekKind::Seek, start);
        assert!(
            watch
                .complete(7, 30, start + Duration::from_millis(20))
                .is_none(),
            "a 20 ms seek is not worth a line"
        );

        watch.arm(8, 30, 2_000_000, 1_000_000, SeekKind::Seek, start);
        let slow = watch
            .complete(8, 30, start + Duration::from_millis(180))
            .expect("180 ms is worth a line");
        assert_eq!(slow.kind, SeekKind::Seek);
        assert_eq!(slow.delta, -1_000_000);
        assert!(slow.backwards(), "backwards is the expensive direction");
        assert!((slow.took.as_secs_f64() * 1000.0 - 180.0).abs() < 1.0);
    }

    #[test]
    fn a_seek_is_timed_once_and_only_against_the_frame_it_asked_for() {
        let start = Instant::now();
        let watch = SeekWatch::new();
        watch.arm(7, 30, 0, 1_000_000, SeekKind::SessionStart, start);

        let late = start + Duration::from_millis(200);
        assert!(
            watch.complete(7, 31, late).is_none(),
            "a different frame of the same session is not the one waited on"
        );
        assert!(
            watch.complete(6, 30, late).is_none(),
            "nor the same frame of a superseded session"
        );
        assert!(watch.complete(7, 30, late).is_some());
        assert!(
            watch.complete(7, 30, late).is_none(),
            "and it is reported once, not on every frame that lands after it"
        );
    }

    /// The user moved again before the first seek's frame arrived. Timing the
    /// abandoned one would measure a wait nobody was doing.
    #[test]
    fn a_superseded_seek_is_forgotten_rather_than_timed() {
        let start = Instant::now();
        let watch = SeekWatch::new();
        watch.arm(7, 30, 0, 1_000_000, SeekKind::Seek, start);
        watch.arm(
            8,
            60,
            1_000_000,
            2_000_000,
            SeekKind::Seek,
            start + Duration::from_millis(10),
        );

        assert!(watch
            .complete(7, 30, start + Duration::from_secs(1))
            .is_none());
        let slow = watch
            .complete(8, 60, start + Duration::from_millis(110))
            .expect("the live seek is still timed");
        assert!((slow.took.as_secs_f64() * 1000.0 - 100.0).abs() < 1.0);
        assert!(!slow.backwards());
    }
}
