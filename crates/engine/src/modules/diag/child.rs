//! Helper processes write into the same log as the app.
//!
//! A helper (the ML worker today, a transcription helper tomorrow) has no log
//! file of its own: what it prints on stderr is its log. [`forward_stderr`]
//! reads that pipe on a thread and writes each line into this process's log,
//! tagged with the helper's name and pid:
//!
//! ```text
//! INFO chukcut_engine::modules::diag::child: [ml-worker 41207] chukcut-ml-worker: rvm on CUDA in 412 ms
//! ```
//!
//! It also registers the pid, so the resource sampler reports the helper's
//! memory and CPU next to the app's own. A new helper needs one call:
//!
//! ```ignore
//! let stderr = child.stderr.take().expect("piped");
//! diag::child::forward_stderr("whisper", child.id(), stderr)?;
//! ```
//!
//! ## Levels
//!
//! A helper prints plain text, so the level is read from the text: a line
//! that starts with `panic` is an ERROR, one that says it failed or could not
//! do something is a WARN, everything else is INFO.
//!
//! ## The rate limit
//!
//! A helper stuck in a loop that prints must not push a week of history out of
//! the log. Each helper may write [`LINES_PER_MINUTE`] lines a minute; the rest
//! are counted, and one line says how many were dropped.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Lines one helper may write in a minute. A healthy ML worker writes a few
/// per model load; a hundred a minute is already something going wrong, and
/// the first hundred of it are enough to say what.
pub const LINES_PER_MINUTE: u32 = 100;

/// The helpers alive now, by pid, for the resource sampler.
static CHILDREN: Mutex<BTreeMap<u32, &'static str>> = parking_lot::const_mutex(BTreeMap::new());

/// Register `pid` as a helper called `name` for the resource sampler, without
/// forwarding anything. [`forward_stderr`] does this itself.
pub fn register(name: &'static str, pid: u32) {
    CHILDREN.lock().insert(pid, name);
}

/// Forget `pid`. The sampler also forgets a pid whose `/proc` entry is gone.
pub fn unregister(pid: u32) {
    CHILDREN.lock().remove(&pid);
}

/// The helpers registered now: `(pid, name)`.
pub fn registered() -> Vec<(u32, &'static str)> {
    CHILDREN
        .lock()
        .iter()
        .map(|(pid, name)| (*pid, *name))
        .collect()
}

/// Read `stderr` line by line on a thread named `<name>-log` and write each
/// line into the log, tagged `[<name> <pid>]`. The pid is registered for the
/// resource sampler until the pipe closes.
pub fn forward_stderr(
    name: &'static str,
    pid: u32,
    stderr: impl Read + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    register(name, pid);
    // The caller's subscriber, carried onto the thread: the global one in the
    // app, a test's own in a test.
    let dispatch = tracing::dispatcher::get_default(|current| current.clone());
    std::thread::Builder::new()
        .name(format!("{name}-log"))
        .spawn(move || {
            let _default = tracing::dispatcher::set_default(&dispatch);
            let mut gate = LineGate::new(LINES_PER_MINUTE, Duration::from_secs(60));
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let line = line.trim_end();
                if line.is_empty() {
                    continue;
                }
                match gate.admit(Instant::now()) {
                    Admit::Yes => log_line(name, pid, line),
                    Admit::YesAfterDropping(dropped) => {
                        tracing::warn!(
                            "[{name} {pid}] dropped {dropped} lines over the limit of \
                             {LINES_PER_MINUTE} a minute"
                        );
                        log_line(name, pid, line);
                    }
                    Admit::No => {}
                }
            }
            let dropped = gate.dropped();
            if dropped > 0 {
                tracing::warn!(
                    "[{name} {pid}] dropped {dropped} lines over the limit of \
                     {LINES_PER_MINUTE} a minute"
                );
            }
            tracing::info!("[{name} {pid}] stderr closed");
            unregister(pid);
        })
}

/// How serious a helper's line reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
    Info,
}

/// The level for one line of a helper's stderr, from its text.
pub fn severity(line: &str) -> Severity {
    let lower = line.to_ascii_lowercase();
    // Our helpers prefix their lines with their binary's name; a panic report
    // starts after it.
    let body = lower
        .split_once(": ")
        .filter(|(head, _)| head.starts_with("chukcut-"))
        .map(|(_, body)| body)
        .unwrap_or(&lower);
    if body.starts_with("panic") || lower.starts_with("panic") {
        Severity::Error
    } else if ["error", "failed", "fail ", "cannot", "could not", "unable"]
        .iter()
        .any(|word| body.contains(word))
    {
        Severity::Warn
    } else {
        Severity::Info
    }
}

fn log_line(name: &str, pid: u32, line: &str) {
    match severity(line) {
        Severity::Error => tracing::error!("[{name} {pid}] {line}"),
        Severity::Warn => tracing::warn!("[{name} {pid}] {line}"),
        Severity::Info => tracing::info!("[{name} {pid}] {line}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Admit {
    Yes,
    /// Yes, and this many were dropped in the window before.
    YesAfterDropping(u64),
    No,
}

/// At most `limit` lines per `window`, counting what it turns away.
pub(super) struct LineGate {
    limit: u32,
    window: Duration,
    started: Option<Instant>,
    used: u32,
    dropped: u64,
}

impl LineGate {
    pub(super) fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            started: None,
            used: 0,
            dropped: 0,
        }
    }

    pub(super) fn admit(&mut self, now: Instant) -> Admit {
        let fresh = self
            .started
            .is_none_or(|started| now.saturating_duration_since(started) >= self.window);
        if fresh {
            self.started = Some(now);
            self.used = 1;
            let dropped = std::mem::take(&mut self.dropped);
            return if dropped > 0 {
                Admit::YesAfterDropping(dropped)
            } else {
                Admit::Yes
            };
        }
        if self.used < self.limit {
            self.used += 1;
            Admit::Yes
        } else {
            self.dropped += 1;
            Admit::No
        }
    }

    fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_read_from_the_text() {
        assert_eq!(
            severity("panic: called `Option::unwrap()` on a `None` value"),
            Severity::Error
        );
        assert_eq!(
            severity("chukcut-ml-worker: panic at src/main.rs:12"),
            Severity::Error
        );
        assert_eq!(
            severity("chukcut-ml-worker: rvm on TensorRT failed, staying on CUDA: x"),
            Severity::Warn
        );
        assert_eq!(
            severity("chukcut-ml-worker: could not preload libcudnn.so.9"),
            Severity::Warn
        );
        assert_eq!(
            severity("chukcut-ml-worker: rvm on CUDA in 412 ms"),
            Severity::Info
        );
    }

    #[test]
    fn the_gate_holds_a_flood_to_its_limit_and_counts_the_rest() {
        let mut gate = LineGate::new(3, Duration::from_secs(60));
        let start = Instant::now();
        let admitted: Vec<Admit> = (0..10).map(|_| gate.admit(start)).collect();
        assert_eq!(
            admitted.iter().filter(|a| **a != Admit::No).count(),
            3,
            "{admitted:?}"
        );
        assert_eq!(gate.dropped(), 7);
        assert_eq!(
            gate.admit(start + Duration::from_secs(61)),
            Admit::YesAfterDropping(7),
            "the next window starts by saying what the last one dropped"
        );
        assert_eq!(gate.dropped(), 0);
    }

    /// A real child's stderr reaches the log, tagged, and the pid is known to
    /// the sampler only while the pipe is open.
    #[test]
    fn a_child_s_stderr_is_forwarded_and_its_pid_registered() {
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("echo 'model loaded in 12 ms' >&2; echo 'chukcut-x: could not open it' >&2")
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("sh runs");
        let pid = child.id();
        let stderr = child.stderr.take().unwrap();
        let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let writer = {
            let out = std::sync::Arc::clone(&out);
            move || CapturedWriter(std::sync::Arc::clone(&out))
        };
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(writer)
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let handle = tracing::dispatcher::with_default(&dispatch, || {
            forward_stderr("test-helper", pid, stderr).unwrap()
        });
        handle.join().unwrap();
        let _ = child.wait();
        assert!(
            !registered().iter().any(|(p, _)| *p == pid),
            "unregistered when the pipe closed"
        );
        let text = String::from_utf8_lossy(&out.lock().unwrap()).into_owned();
        let tag = format!("[test-helper {pid}]");
        let lines: Vec<&str> = text.lines().filter(|l| l.contains(&tag)).collect();
        assert_eq!(lines.len(), 3, "two lines and the close: {text}");
        assert!(
            lines[0].contains("INFO") && lines[0].contains("model loaded in 12 ms"),
            "{text}"
        );
        assert!(
            lines[1].contains("WARN") && lines[1].contains("could not open it"),
            "{text}"
        );
    }

    struct CapturedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
