//! "This took longer than it may": one helper for every timing check, and the
//! rate limit that keeps a slow loop from filling the log.
//!
//! Every over-budget line in the log has the same shape, so one grep finds
//! them all and one reading rule explains them:
//!
//! ```text
//! WARN chukcut_engine::modules::diag::budget: over budget what="decoder seek" ms=212.4 budget_ms=150 detail="clip.mp4 to 3.000 s, cuda"
//! ```
//!
//! `what` is a fixed name for the place (it is also the rate-limit key),
//! `ms` is what the work took, `budget_ms` is what it may take, and `detail`
//! says which instance it was. A line that follows suppressed ones carries
//! `suppressed=N worst_suppressed_ms=…`: N more were over budget in the
//! window and were not written, and the worst of them took that long.
//!
//! ## The rate limit
//!
//! One line per `what` per [`WINDOW`]. A seek that is slow on every frame for
//! an hour writes 360 lines, not 100 000, and none of the information is lost:
//! the count and the worst case ride on the next line, and [`flush`] (called
//! by the resource sampler every 30 s) writes the count out when no next line
//! comes.
//!
//! ## Cost
//!
//! A check that is under budget is one `Instant` subtraction and a compare. The
//! lock and the formatting happen only for work that was already slow.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// At most one line per `what` in this window.
pub const WINDOW: Duration = Duration::from_secs(10);

/// One key's state: when it last wrote a line, and what it has held back
/// since.
#[derive(Debug, Clone, Copy)]
struct Entry {
    last: Instant,
    suppressed: u64,
    worst_ms: f64,
}

/// What [`RateLimiter::check`] decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Write the line. `suppressed` lines were held back before it, the
    /// slowest of which took `worst_ms`.
    Emit { suppressed: u64, worst_ms: f64 },
    /// Do not write it; it is counted.
    Suppress,
}

/// A per-key "at most one per window" gate that counts what it holds back.
///
/// Keys are `&'static str` on purpose: the set of keys is the set of places in
/// the code, which is finite, so the map cannot grow without bound however
/// long the app runs.
pub struct RateLimiter {
    window: Duration,
    entries: Mutex<BTreeMap<&'static str, Entry>>,
}

impl RateLimiter {
    pub const fn new(window: Duration) -> Self {
        Self {
            window,
            entries: parking_lot::const_mutex(BTreeMap::new()),
        }
    }

    /// Decide about one event of `key` at `now` with magnitude `value_ms`
    /// (only used to remember the worst suppressed one).
    pub fn check(&self, key: &'static str, now: Instant, value_ms: f64) -> Verdict {
        let mut entries = self.entries.lock();
        match entries.get_mut(key) {
            Some(entry) if now.saturating_duration_since(entry.last) < self.window => {
                entry.suppressed += 1;
                entry.worst_ms = entry.worst_ms.max(value_ms);
                Verdict::Suppress
            }
            Some(entry) => {
                let verdict = Verdict::Emit {
                    suppressed: entry.suppressed,
                    worst_ms: entry.worst_ms,
                };
                *entry = Entry {
                    last: now,
                    suppressed: 0,
                    worst_ms: 0.0,
                };
                verdict
            }
            None => {
                entries.insert(
                    key,
                    Entry {
                        last: now,
                        suppressed: 0,
                        worst_ms: 0.0,
                    },
                );
                Verdict::Emit {
                    suppressed: 0,
                    worst_ms: 0.0,
                }
            }
        }
    }

    /// Take the counts of keys that held lines back and whose window has
    /// passed, so they can be written out. Each one taken counts as a line
    /// written at `now`.
    pub fn drain(&self, now: Instant) -> Vec<(&'static str, u64, f64)> {
        let mut entries = self.entries.lock();
        let mut out = Vec::new();
        for (key, entry) in entries.iter_mut() {
            if entry.suppressed > 0 && now.saturating_duration_since(entry.last) >= self.window {
                out.push((*key, entry.suppressed, entry.worst_ms));
                *entry = Entry {
                    last: now,
                    suppressed: 0,
                    worst_ms: 0.0,
                };
            }
        }
        out
    }
}

/// The limiter every over-budget line goes through.
static LIMITER: RateLimiter = RateLimiter::new(WINDOW);

fn round1(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Write an over-budget line when `took` is longer than `budget`, rate-limited
/// per `what`. `detail` is only called when a line is written. Answers whether
/// the work was over budget, written or not.
pub fn check(
    what: &'static str,
    took: Duration,
    budget: Duration,
    detail: impl FnOnce() -> String,
) -> bool {
    if took <= budget {
        return false;
    }
    let ms = round1(millis(took));
    match LIMITER.check(what, Instant::now(), ms) {
        Verdict::Suppress => {}
        Verdict::Emit {
            suppressed: 0,
            worst_ms: _,
        } => tracing::warn!(
            what,
            ms,
            budget_ms = round1(millis(budget)),
            detail = %detail(),
            "over budget"
        ),
        Verdict::Emit {
            suppressed,
            worst_ms,
        } => tracing::warn!(
            what,
            ms,
            budget_ms = round1(millis(budget)),
            detail = %detail(),
            suppressed,
            worst_suppressed_ms = worst_ms,
            "over budget"
        ),
    }
    true
}

/// Write out what the rate limit held back and no later line reported.
///
/// Called by the resource sampler; a test or a shutdown may call it too.
pub fn flush() {
    for (what, suppressed, worst_ms) in LIMITER.drain(Instant::now()) {
        tracing::warn!(
            what,
            suppressed,
            worst_suppressed_ms = worst_ms,
            "over budget, held back by the rate limit"
        );
    }
}

/// A guard that times the scope it lives in and calls [`check`] on drop.
///
/// ```ignore
/// let _budget = diag::budget("project save", Duration::from_millis(200), || path.display().to_string());
/// ```
///
/// `detail` runs only when the line is written, so it may format freely.
#[must_use = "the budget is measured until the guard is dropped"]
pub struct Budget<F: FnOnce() -> String> {
    what: &'static str,
    budget: Duration,
    started: Instant,
    detail: Option<F>,
}

/// Start timing `what` against `budget`.
pub fn budget<F: FnOnce() -> String>(what: &'static str, budget: Duration, detail: F) -> Budget<F> {
    Budget {
        what,
        budget,
        started: Instant::now(),
        detail: Some(detail),
    }
}

impl<F: FnOnce() -> String> Budget<F> {
    /// How long the guarded work has run so far.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl<F: FnOnce() -> String> Drop for Budget<F> {
    fn drop(&mut self) {
        let took = self.started.elapsed();
        if took <= self.budget {
            return;
        }
        if let Some(detail) = self.detail.take() {
            check(self.what, took, self.budget, detail);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;
        fn make_writer(&'a self) -> Captured {
            self.clone()
        }
    }

    fn capture(body: impl FnOnce()) -> String {
        let out = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(out.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, body);
        let text = String::from_utf8_lossy(&out.0.lock().unwrap()).into_owned();
        text
    }

    #[test]
    fn one_line_per_window_and_the_rest_are_counted() {
        let limiter = RateLimiter::new(Duration::from_secs(10));
        let start = Instant::now();
        assert_eq!(
            limiter.check("seek", start, 60.0),
            Verdict::Emit {
                suppressed: 0,
                worst_ms: 0.0
            }
        );
        for (i, ms) in [70.0, 300.0, 80.0].into_iter().enumerate() {
            let at = start + Duration::from_secs(1 + i as u64);
            assert_eq!(limiter.check("seek", at, ms), Verdict::Suppress);
        }
        // Another key has its own window.
        assert!(matches!(
            limiter.check("open", start + Duration::from_secs(2), 1.0),
            Verdict::Emit { .. }
        ));
        assert_eq!(
            limiter.check("seek", start + Duration::from_secs(10), 55.0),
            Verdict::Emit {
                suppressed: 3,
                worst_ms: 300.0
            },
            "the next line carries the count and the worst of the held-back ones"
        );
        assert_eq!(
            limiter.check("seek", start + Duration::from_secs(25), 55.0),
            Verdict::Emit {
                suppressed: 0,
                worst_ms: 0.0
            },
            "and the count starts again after it"
        );
    }

    #[test]
    fn drain_reports_what_no_later_line_did() {
        let limiter = RateLimiter::new(Duration::from_secs(10));
        let start = Instant::now();
        let _ = limiter.check("seek", start, 60.0);
        let _ = limiter.check("seek", start + Duration::from_secs(1), 90.0);
        let _ = limiter.check("open", start, 10.0);
        assert!(
            limiter.drain(start + Duration::from_secs(5)).is_empty(),
            "nothing is drained inside the window"
        );
        assert_eq!(
            limiter.drain(start + Duration::from_secs(11)),
            vec![("seek", 1, 90.0)],
            "only keys that held something back"
        );
        assert!(limiter.drain(start + Duration::from_secs(30)).is_empty());
    }

    /// A flood stays a trickle: a thousand slow events of one kind write one
    /// line, which says how many there were.
    #[test]
    fn a_flood_of_slow_work_writes_one_line() {
        let text = capture(|| {
            for _ in 0..1000 {
                check(
                    "test flood",
                    Duration::from_millis(80),
                    Duration::from_millis(50),
                    || "detail".into(),
                );
            }
        });
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("over budget"), "{text}");
        assert!(text.contains("what=\"test flood\""), "{text}");
        assert!(text.contains("ms=80"), "{text}");
        assert!(text.contains("budget_ms=50"), "{text}");
    }

    #[test]
    fn work_inside_its_budget_writes_nothing() {
        let mut called = false;
        let text = capture(|| {
            let over = check(
                "test inside",
                Duration::from_millis(10),
                Duration::from_millis(50),
                || {
                    called = true;
                    String::new()
                },
            );
            assert!(!over);
        });
        assert!(text.is_empty(), "{text}");
        assert!(!called, "the detail is not even formatted");
    }

    #[test]
    fn the_guard_measures_its_scope() {
        let text = capture(|| {
            {
                let _guard = budget("test guard slow", Duration::from_millis(5), || {
                    "the slow one".into()
                });
                std::thread::sleep(Duration::from_millis(20));
            }
            {
                let _guard = budget("test guard fast", Duration::from_secs(5), || {
                    "the fast one".into()
                });
            }
        });
        assert!(text.contains("the slow one"), "{text}");
        assert!(!text.contains("the fast one"), "{text}");
        let ms: f64 = text
            .split("ms=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .expect("a measured ms");
        assert!(ms >= 20.0, "it measured the sleep: {ms}");
    }
}
