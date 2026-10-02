//! Where the preview's wall clock actually goes, per thread.
//!
//! This exists because the per-stage numbers and the observed frame rate did
//! not add up. `chukcut-bench --filter preview-frame` measures one frame's
//! *latency* through the same calls the server makes — 10.7 ms at 1920×1080 —
//! and the owner's playback was 11–20 fps, i.e. 50–90 ms a frame. A latency
//! number cannot explain a throughput number: the missing question is how much
//! of each thread's wall time is spent doing which stage, and how much of it is
//! spent waiting for another thread.
//!
//! So this counts **occupancy**, not duration. Over a run of wall time `W`, a
//! stage that totals `W` on one thread is that thread's whole life; two stages
//! that each total `W` on *different* threads are perfectly pipelined, and two
//! that each total `W/2` on the *same* thread are serialised and the frame is
//! their sum. That distinction is the entire question and no per-frame timing
//! can answer it.
//!
//! Everything here is a relaxed atomic add on a path that already takes a
//! mutex and copies megabytes, so it is not measurement that changes what it
//! measures. It is always on, for the same reason [`super::stats`] is: the
//! numbers are only useful when the thing being investigated is the thing that
//! happened, and nobody reproduces a stutter on request.
//!
//! Read it with [`Probe::snapshot`]; [`Probe::reset`] between measured phases.
//! `examples/preview_pipeline.rs` is the harness that does both.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// The process-wide probe. One preview server per process, so one of these.
pub static PROBE: Probe = Probe::new();

#[derive(Default)]
pub struct Probe {
    // --- the render thread -------------------------------------------------
    /// Blocked in `next_job`: no session, not playing, or read-ahead reached.
    /// High during playback means the renderer is *ahead* and the frame rate is
    /// somebody else's fault.
    pub render_wait_ns: AtomicU64,
    /// `Compositor::render_frame` — decode, upload, composite, read back.
    pub composite_ns: AtomicU64,
    /// Handing the encode to the encode thread: the closure, the atomic, the
    /// `try_send`. Should be nothing; measured so that "should be" is not an
    /// assumption.
    pub dispatch_ns: AtomicU64,
    /// Encodes the render thread had to do itself because the queue was full.
    /// **This is the serialisation that matters**: an inline encode makes the
    /// frame cost composite + encode instead of max(composite, encode).
    pub inline_encode_ns: AtomicU64,
    pub composites: AtomicU64,
    pub inline_encodes: AtomicU64,
    /// Jobs that came in as scrubs rather than as playback frames.
    pub scrub_jobs: AtomicU64,

    // --- the encode thread -------------------------------------------------
    /// Blocked in `recv`. High means the encoder is not the bottleneck.
    pub encode_idle_ns: AtomicU64,
    /// `encode_preview_jpeg`, wherever it ran.
    pub encode_ns: AtomicU64,
    pub encodes: AtomicU64,
    /// `FrameCache::insert` — the ring's mutex plus the condvar wake.
    pub insert_ns: AtomicU64,
    /// JPEG bytes produced, so the protocol's byte rate is a measurement and
    /// not an estimate.
    pub encoded_bytes: AtomicU64,

    // --- the request side --------------------------------------------------
    /// Whole `serve_uri` calls, and how each ended.
    pub serve_ns: AtomicU64,
    pub serve_calls: AtomicU64,
    /// The frame was in the ring on the first look. The only outcome that costs
    /// nothing.
    pub serve_hit: AtomicU64,
    /// It was not, and the handler blocked for it. `serve_wait_ns` is the time
    /// the webview spent waiting on a frame that did not exist yet.
    pub serve_wait_ns: AtomicU64,
    pub serve_wait_hit: AtomicU64,
    /// The wait timed out and a neighbouring frame was served instead.
    pub serve_nearest: AtomicU64,
    /// Nothing within tolerance: 204, and the viewer keeps the old picture.
    pub serve_empty: AtomicU64,
    /// Superseded session: 410.
    pub serve_stale: AtomicU64,

    /// `Channel::send` for one [`super::server::PreviewEvent`].
    ///
    /// This is the Rust half of the position update only. In the real app the
    /// same call serialises to JSON and hands it to the webview's IPC, and how
    /// long the *frontend* then takes to notice is not observable from here at
    /// all — see the note in `docs/research/preview-performance.md`.
    pub emit_ns: AtomicU64,
    pub emits: AtomicU64,
}

impl Probe {
    pub const fn new() -> Self {
        Self {
            render_wait_ns: AtomicU64::new(0),
            composite_ns: AtomicU64::new(0),
            dispatch_ns: AtomicU64::new(0),
            inline_encode_ns: AtomicU64::new(0),
            composites: AtomicU64::new(0),
            inline_encodes: AtomicU64::new(0),
            scrub_jobs: AtomicU64::new(0),
            encode_idle_ns: AtomicU64::new(0),
            encode_ns: AtomicU64::new(0),
            encodes: AtomicU64::new(0),
            insert_ns: AtomicU64::new(0),
            encoded_bytes: AtomicU64::new(0),
            serve_ns: AtomicU64::new(0),
            serve_calls: AtomicU64::new(0),
            serve_hit: AtomicU64::new(0),
            serve_wait_ns: AtomicU64::new(0),
            serve_wait_hit: AtomicU64::new(0),
            serve_nearest: AtomicU64::new(0),
            serve_empty: AtomicU64::new(0),
            serve_stale: AtomicU64::new(0),
            emit_ns: AtomicU64::new(0),
            emits: AtomicU64::new(0),
        }
    }

    pub fn reset(&self) {
        for counter in self.counters() {
            counter.store(0, Ordering::Relaxed);
        }
    }

    fn counters(&self) -> [&AtomicU64; 22] {
        [
            &self.render_wait_ns,
            &self.composite_ns,
            &self.dispatch_ns,
            &self.inline_encode_ns,
            &self.composites,
            &self.inline_encodes,
            &self.scrub_jobs,
            &self.encode_idle_ns,
            &self.encode_ns,
            &self.encodes,
            &self.insert_ns,
            &self.encoded_bytes,
            &self.serve_ns,
            &self.serve_calls,
            &self.serve_hit,
            &self.serve_wait_ns,
            &self.serve_wait_hit,
            &self.serve_nearest,
            &self.serve_empty,
            &self.serve_stale,
            &self.emit_ns,
            &self.emits,
        ]
    }

    pub fn snapshot(&self) -> Counts {
        Counts {
            render_wait_ns: self.render_wait_ns.load(Ordering::Relaxed),
            composite_ns: self.composite_ns.load(Ordering::Relaxed),
            dispatch_ns: self.dispatch_ns.load(Ordering::Relaxed),
            inline_encode_ns: self.inline_encode_ns.load(Ordering::Relaxed),
            composites: self.composites.load(Ordering::Relaxed),
            inline_encodes: self.inline_encodes.load(Ordering::Relaxed),
            scrub_jobs: self.scrub_jobs.load(Ordering::Relaxed),
            encode_idle_ns: self.encode_idle_ns.load(Ordering::Relaxed),
            encode_ns: self.encode_ns.load(Ordering::Relaxed),
            encodes: self.encodes.load(Ordering::Relaxed),
            insert_ns: self.insert_ns.load(Ordering::Relaxed),
            encoded_bytes: self.encoded_bytes.load(Ordering::Relaxed),
            serve_ns: self.serve_ns.load(Ordering::Relaxed),
            serve_calls: self.serve_calls.load(Ordering::Relaxed),
            serve_hit: self.serve_hit.load(Ordering::Relaxed),
            serve_wait_ns: self.serve_wait_ns.load(Ordering::Relaxed),
            serve_wait_hit: self.serve_wait_hit.load(Ordering::Relaxed),
            serve_nearest: self.serve_nearest.load(Ordering::Relaxed),
            serve_empty: self.serve_empty.load(Ordering::Relaxed),
            serve_stale: self.serve_stale.load(Ordering::Relaxed),
            emit_ns: self.emit_ns.load(Ordering::Relaxed),
            emits: self.emits.load(Ordering::Relaxed),
        }
    }
}

/// A plain snapshot, so a reader is not holding atomics that keep moving.
#[derive(Debug, Clone, Copy, Default)]
pub struct Counts {
    pub render_wait_ns: u64,
    pub composite_ns: u64,
    pub dispatch_ns: u64,
    pub inline_encode_ns: u64,
    pub composites: u64,
    pub inline_encodes: u64,
    pub scrub_jobs: u64,
    pub encode_idle_ns: u64,
    pub encode_ns: u64,
    pub encodes: u64,
    pub insert_ns: u64,
    pub encoded_bytes: u64,
    pub serve_ns: u64,
    pub serve_calls: u64,
    pub serve_hit: u64,
    pub serve_wait_ns: u64,
    pub serve_wait_hit: u64,
    pub serve_nearest: u64,
    pub serve_empty: u64,
    pub serve_stale: u64,
    pub emit_ns: u64,
    pub emits: u64,
}

impl Counts {
    /// Everything the render thread did that was not waiting.
    pub fn render_busy_ns(&self) -> u64 {
        self.composite_ns + self.dispatch_ns + self.inline_encode_ns
    }

    /// Mean milliseconds per occurrence, or 0.0 when nothing happened.
    pub fn per(total_ns: u64, count: u64) -> f64 {
        if count == 0 {
            0.0
        } else {
            total_ns as f64 / count as f64 / 1e6
        }
    }
}

/// Add the time since `started` to `counter`.
///
/// Free function rather than a method so the call sites read as one line and
/// the `Instant` stays where the thing being timed is.
#[inline]
pub fn add(counter: &AtomicU64, started: Instant) {
    counter.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

#[inline]
pub fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reset_probe_reads_zero_everywhere() {
        let probe = Probe::new();
        bump(&probe.composites);
        probe.composite_ns.fetch_add(1234, Ordering::Relaxed);
        assert_eq!(probe.snapshot().composites, 1);
        probe.reset();
        let counts = probe.snapshot();
        assert_eq!(counts.composites, 0);
        assert_eq!(counts.composite_ns, 0);
    }

    #[test]
    fn render_busy_is_the_sum_of_the_stages_that_are_not_waiting() {
        let probe = Probe::new();
        probe.composite_ns.fetch_add(10, Ordering::Relaxed);
        probe.dispatch_ns.fetch_add(1, Ordering::Relaxed);
        probe.inline_encode_ns.fetch_add(5, Ordering::Relaxed);
        probe.render_wait_ns.fetch_add(99, Ordering::Relaxed);
        assert_eq!(probe.snapshot().render_busy_ns(), 16);
    }
}
