// Adapted from FilmCraft's `crates/automation/src/bridge.rs` (the client of
// the app's JSON-lines control channel: reconnect on failure, long commands
// followed to their end, a request id per call):
//
//   Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
//   Licensed under the Apache License, Version 2.0 (or the MIT License, at
//   your option). https://github.com/storytold/filmcraft
//
// Changes for chukcut: a blocking Unix-socket client instead of tokio TCP,
// progress lines forwarded as they come instead of polling a job list, and
// cancellation through `engine.cancel`. NOTICE.md lists what was taken.

//! The client side of the app's control socket ([`crate::control`]): what
//! `chukcut-cli mcp --bridge` talks to the running app with.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// How long a request without progress may go unanswered. Long commands
/// (an export, a transcription) send progress lines, and each one restarts
/// the clock; a UI request that takes this long has hung.
const QUIET_LIMIT: Duration = Duration::from_secs(600);
/// How often a waiting call looks at its cancel flag.
const POLL: Duration = Duration::from_millis(100);

type Conn = (BufReader<UnixStream>, UnixStream);

pub struct Bridge {
    /// The socket named on the command line, or `None` to find the running
    /// app's ([`crate::control::discover`]) at each connect, so an agent can
    /// start before the app and the app can restart under it.
    explicit: Option<PathBuf>,
    conn: Mutex<Option<Conn>>,
    next_id: AtomicU64,
}

impl Bridge {
    pub fn new(explicit: Option<PathBuf>) -> Self {
        Self {
            explicit,
            conn: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }

    fn path(&self) -> Result<PathBuf, String> {
        match &self.explicit {
            Some(path) => Ok(path.clone()),
            None => crate::control::discover().ok_or_else(|| {
                format!(
                    "no running chukcut to drive: start the app with `chukcut --control` \
                     (looked in {})",
                    crate::control::socket_dir().display()
                )
            }),
        }
    }

    fn connect(&self) -> Result<Conn, String> {
        let path = self.path()?;
        let stream = UnixStream::connect(&path).map_err(|e| {
            format!(
                "cannot connect to {} ({e}); start the app with `chukcut --control`",
                path.display()
            )
        })?;
        let read = stream.try_clone().map_err(|e| e.to_string())?;
        read.set_read_timeout(Some(POLL))
            .map_err(|e| e.to_string())?;
        Ok((BufReader::new(read), stream))
    }

    /// Call `method` and return its result, or the app's error.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.call_with(method, params, &|_, _| {}, &AtomicBool::new(false))
    }

    /// Call `method`, passing each progress line to `progress` as it comes.
    /// When `cancel` is set while waiting, the app is asked to stop the
    /// command (`engine.cancel`) and the call answers once it has.
    pub fn call_with(
        &self,
        method: &str,
        params: Value,
        progress: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<Value, String> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        for attempt in 0..2 {
            // A retry gets a fresh connection: the old one may belong to an
            // app that has since restarted.
            let mut conn = match guard.take() {
                Some(conn) if attempt == 0 => conn,
                _ => self.connect()?,
            };
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let line = format!(
                "{}\n",
                json!({"id": id, "method": method, "params": params})
            );
            if conn.1.write_all(line.as_bytes()).is_err() {
                continue;
            }
            match self.wait(&mut conn, id, progress, cancel) {
                Ok(reply) => {
                    *guard = Some(conn);
                    return if reply.get("ok").and_then(Value::as_bool) == Some(true) {
                        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(reply
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("the app answered an error without a message")
                            .to_string())
                    };
                }
                // The connection went away before anything was answered:
                // the app restarted. Once more on a new connection.
                Err(Failure::Closed) if attempt == 0 => continue,
                Err(Failure::Closed) => return Err("the app closed the connection".into()),
                Err(Failure::Other(e)) => return Err(e),
            }
        }
        Err("the app closed the connection".into())
    }

    fn wait(
        &self,
        conn: &mut Conn,
        id: u64,
        progress: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<Value, Failure> {
        let mut buf = Vec::new();
        let mut heard = Instant::now();
        let mut cancel_sent = false;
        // Once the app has said anything about this call, it is running it,
        // and a closed connection must not make the caller run it twice.
        let mut started = false;
        let closed = |started: bool| {
            if started {
                Failure::Other("the app closed the connection while running the command".into())
            } else {
                Failure::Closed
            }
        };
        loop {
            match conn.0.read_until(b'\n', &mut buf) {
                Ok(0) => return Err(closed(started || !buf.is_empty())),
                Ok(_) if buf.last() == Some(&b'\n') => {
                    let line = std::mem::take(&mut buf);
                    heard = Instant::now();
                    let Ok(message) = serde_json::from_slice::<Value>(&line) else {
                        return Err(Failure::Other(
                            "the app sent a line that is not JSON".into(),
                        ));
                    };
                    if message.get("id") != Some(&json!(id)) {
                        // A late answer to a call given up on; not ours.
                        continue;
                    }
                    started = true;
                    if message.get("ok").is_none() {
                        let label = message.get("label").and_then(Value::as_str).unwrap_or("");
                        let fraction = message
                            .get("progress")
                            .and_then(Value::as_f64)
                            .map(|f| f as f32);
                        progress(label, fraction);
                        continue;
                    }
                    return Ok(message);
                }
                // A partial line stays in `buf`; read_until keeps what it read.
                Ok(_) => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(closed(started || !buf.is_empty())),
            }
            if cancel.load(Ordering::Acquire) && !cancel_sent {
                cancel_sent = true;
                // On a connection of its own: this one is busy with the
                // command it cancels.
                if let Ok((_, mut side)) = self.connect() {
                    let line =
                        json!({"id": 0, "method": "engine.cancel", "params": {"request": id}});
                    let _ = writeln!(side, "{line}");
                }
            }
            if heard.elapsed() > QUIET_LIMIT {
                return Err(Failure::Other(format!(
                    "the app did not answer for {} s",
                    QUIET_LIMIT.as_secs()
                )));
            }
        }
    }
}

enum Failure {
    Closed,
    Other(String),
}
