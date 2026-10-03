//! Starting, talking to and supervising `chukcut-ml-worker`.
//!
//! One worker process serves the whole editor. Callers on any thread send a
//! request and wait for its answer; a reader thread routes the worker's
//! replies to the waiting caller by request id. The worker runs requests one
//! at a time, so two jobs that both need it simply take turns.
//!
//! **Crashes.** When the worker's stdout closes, every waiting caller gets
//! [`MlError::Crashed`] and the process is reaped. The next request starts a
//! fresh worker — at most [`MAX_STARTS`] times per [`START_WINDOW`], after
//! which requests fail with a message instead of spawning a process that dies
//! again. State the worker held (open tracks) is gone with it; callers that
//! keep such state restart it (the face detector simply asks again).
//!
//! **Hangs.** Every request has a deadline. A worker that misses it is killed
//! and counts as crashed, so a wedged GPU driver costs one job, not the
//! editor.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use chukcut_ml_worker::protocol::{
    read_frame, write_frame, ErrorKind, Message, Outcome, Reply, Request, RequestBody,
    PROTOCOL_VERSION,
};
use parking_lot::Mutex;

/// The binary's name, next to the editor's own.
pub const BINARY: &str = "chukcut-ml-worker";
/// Starts allowed within [`START_WINDOW`] before the supervisor gives up.
pub const MAX_STARTS: usize = 3;
pub const START_WINDOW: Duration = Duration::from_secs(60);
/// How long the worker may take to say hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// Why an ML request did not produce a result. The `Display` text is
/// user-facing.
#[derive(Debug, Clone, PartialEq)]
pub enum MlError {
    /// No worker binary on this machine, or ML is switched off.
    Unavailable(String),
    /// The worker runs but has no ONNX Runtime to load.
    RuntimeMissing(String),
    /// The model is not downloaded.
    ModelMissing(String),
    /// The worker died or hung; the next request starts a new one.
    Crashed(String),
    Cancelled,
    /// Anything else the worker or a download reported.
    Failed(String),
}

impl std::fmt::Display for MlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MlError::Unavailable(why) => write!(f, "machine learning is not available: {why}"),
            MlError::RuntimeMissing(why) => write!(f, "ONNX Runtime is not installed: {why}"),
            MlError::ModelMissing(why) => write!(f, "{why}"),
            MlError::Crashed(why) => write!(f, "the ML worker stopped: {why}"),
            MlError::Cancelled => f.write_str("cancelled"),
            MlError::Failed(why) => f.write_str(why),
        }
    }
}

impl From<MlError> for String {
    fn from(e: MlError) -> String {
        e.to_string()
    }
}

/// Where the worker binary is: `CHUKCUT_ML_WORKER` if set (an empty value or
/// `off` switches ML off), else next to the running executable (or one
/// directory up, which is where cargo puts binaries relative to test
/// executables in `deps/`), else on `PATH`.
pub fn binary() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("CHUKCUT_ML_WORKER") {
        let value = PathBuf::from(value);
        if value.as_os_str().is_empty() || value.as_os_str() == "off" {
            return None;
        }
        return value.is_file().then_some(value);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for candidate in [dir.join(BINARY), dir.parent()?.join(BINARY)] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|d| d.join(BINARY))
            .find(|p| p.is_file())
    })
}

/// A reply and the payload that came with it (empty for most).
type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<(Reply, Vec<u8>)>>>>;

/// One running worker process.
pub struct Client {
    child: Mutex<Child>,
    stdin: Mutex<BufWriter<ChildStdin>>,
    pending: Pending,
    alive: Arc<AtomicBool>,
    next_id: AtomicU64,
    pub pid: u32,
}

impl Client {
    fn spawn(binary: &PathBuf) -> Result<Client, MlError> {
        let mut child = Command::new(binary)
            .arg("--root")
            .arg(super::root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| MlError::Unavailable(format!("cannot start {}: {e}", binary.display())))?;
        let pid = child.id();
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        {
            let (pending, alive) = (Arc::clone(&pending), Arc::clone(&alive));
            std::thread::Builder::new()
                .name("ml-worker-reader".into())
                .spawn(move || {
                    let mut stdout = BufReader::new(stdout);
                    loop {
                        match read_frame::<Message>(&mut stdout) {
                            Ok(Some((message, payload))) => {
                                let last = !matches!(message.body, Reply::Progress { .. });
                                let mut pending = pending.lock();
                                if let Some(tx) = pending.get(&message.id) {
                                    let _ = tx.send((message.body, payload));
                                }
                                if last {
                                    pending.remove(&message.id);
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!("ML worker {pid}: unreadable reply: {e}");
                                break;
                            }
                        }
                    }
                    alive.store(false, Ordering::SeqCst);
                    // Dropping the senders wakes every waiting caller with
                    // a disconnect, which they report as a crash.
                    pending.lock().clear();
                })
                .map_err(|e| MlError::Failed(e.to_string()))?;
        }
        std::thread::Builder::new()
            .name("ml-worker-log".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    tracing::info!("ML worker {pid}: {line}");
                }
            })
            .map_err(|e| MlError::Failed(e.to_string()))?;
        let client = Client {
            child: Mutex::new(child),
            stdin: Mutex::new(BufWriter::new(stdin)),
            pending,
            alive,
            next_id: AtomicU64::new(1),
            pid,
        };
        match client.call_with(RequestBody::Hello, &[], &|_, _| {}, None, HELLO_TIMEOUT)? {
            Outcome::Hello { protocol, .. } if protocol == PROTOCOL_VERSION => Ok(client),
            Outcome::Hello { protocol, .. } => {
                client.kill();
                Err(MlError::Unavailable(format!(
                    "the ML worker speaks protocol {protocol}, the editor {PROTOCOL_VERSION}; reinstall chukcut"
                )))
            }
            other => {
                client.kill();
                Err(MlError::Failed(format!(
                    "the ML worker answered hello with {other:?}"
                )))
            }
        }
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn kill(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let mut child = self.child.lock();
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Send `body` with `payload` and wait for the answer, reporting progress
    /// and sending a cancel once `cancel` is set. Fails with
    /// [`MlError::Crashed`] if the worker dies or misses `timeout`.
    pub fn call_with(
        &self,
        body: RequestBody,
        payload: &[u8],
        progress: &dyn Fn(f32, &str),
        cancel: Option<&AtomicBool>,
        timeout: Duration,
    ) -> Result<Outcome, MlError> {
        self.call_for_payload(body, payload, progress, cancel, timeout)
            .map(|(outcome, _)| outcome)
    }

    /// [`Self::call_with`], keeping the payload the answer carried (a
    /// matte's alpha bytes).
    pub fn call_for_payload(
        &self,
        body: RequestBody,
        payload: &[u8],
        progress: &dyn Fn(f32, &str),
        cancel: Option<&AtomicBool>,
        timeout: Duration,
    ) -> Result<(Outcome, Vec<u8>), MlError> {
        if !self.alive() {
            return Err(MlError::Crashed("the worker is not running".into()));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().insert(id, tx);
        if let Err(e) = write_frame(&mut *self.stdin.lock(), &Request { id, body }, payload) {
            self.pending.lock().remove(&id);
            self.kill();
            return Err(MlError::Crashed(format!("cannot write to the worker: {e}")));
        }
        let deadline = Instant::now() + timeout;
        let mut cancel_sent = false;
        loop {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok((Reply::Progress { fraction, stage }, _)) => progress(fraction, &stage),
                Ok((Reply::Done { outcome }, payload)) => return Ok((outcome, payload)),
                Ok((Reply::Error { kind, message }, _)) => {
                    return Err(match kind {
                        ErrorKind::RuntimeMissing => MlError::RuntimeMissing(message),
                        ErrorKind::ModelMissing => MlError::ModelMissing(message),
                        ErrorKind::Cancelled => MlError::Cancelled,
                        ErrorKind::BadRequest | ErrorKind::Inference => MlError::Failed(message),
                    })
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.reap();
                    return Err(MlError::Crashed("the worker exited".into()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !cancel_sent && cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                        cancel_sent = true;
                        let target = id;
                        let cancel_id = self.next_id.fetch_add(1, Ordering::Relaxed);
                        let _ = write_frame(
                            &mut *self.stdin.lock(),
                            &Request {
                                id: cancel_id,
                                body: RequestBody::Cancel { target },
                            },
                            &[],
                        );
                    }
                    if Instant::now() > deadline {
                        self.pending.lock().remove(&id);
                        self.kill();
                        return Err(MlError::Crashed(format!(
                            "no answer within {} s",
                            timeout.as_secs()
                        )));
                    }
                }
            }
        }
    }

    /// Collect a dead worker's exit status, for the log.
    fn reap(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let mut child = self.child.lock();
        match child.try_wait() {
            Ok(Some(status)) => tracing::warn!("ML worker {} exited: {status}", self.pid),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// Ask the worker to finish and wait for it, briefly.
    pub fn shutdown(&self) {
        if self.alive() {
            let _ = self.call_with(
                RequestBody::Shutdown,
                &[],
                &|_, _| {},
                None,
                Duration::from_secs(2),
            );
        }
        self.kill();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Default)]
struct Supervisor {
    client: Option<Arc<Client>>,
    starts: VecDeque<Instant>,
}

static SUPERVISOR: Mutex<Supervisor> = Mutex::new(Supervisor {
    client: None,
    starts: VecDeque::new(),
});

/// The running worker, started (or restarted) if needed.
pub fn client() -> Result<Arc<Client>, MlError> {
    let mut sup = SUPERVISOR.lock();
    if let Some(client) = &sup.client {
        if client.alive() {
            return Ok(Arc::clone(client));
        }
        tracing::warn!("ML worker {} is gone; starting a new one", client.pid);
        sup.client = None;
    }
    let binary = binary().ok_or_else(|| {
        MlError::Unavailable(format!("{BINARY} was not found next to chukcut or on PATH"))
    })?;
    let now = Instant::now();
    while sup
        .starts
        .front()
        .is_some_and(|t| now.duration_since(*t) > START_WINDOW)
    {
        sup.starts.pop_front();
    }
    if sup.starts.len() >= MAX_STARTS {
        return Err(MlError::Crashed(format!(
            "it stopped {MAX_STARTS} times within a minute; not starting it again yet"
        )));
    }
    sup.starts.push_back(now);
    let client = Arc::new(Client::spawn(&binary)?);
    tracing::info!("ML worker {} started from {}", client.pid, binary.display());
    sup.client = Some(Arc::clone(&client));
    Ok(client)
}

/// The worker, only if one is running now.
pub fn running() -> Option<Arc<Client>> {
    SUPERVISOR
        .lock()
        .client
        .as_ref()
        .filter(|c| c.alive())
        .cloned()
}

/// Send one request to the worker, starting it if needed. A request that
/// meets a dead worker is tried once more on a fresh one: the requests that
/// go through here carry everything they need, so repeating one is safe.
pub fn request(
    body: RequestBody,
    payload: &[u8],
    progress: &dyn Fn(f32, &str),
    cancel: Option<&AtomicBool>,
    timeout: Duration,
) -> Result<Outcome, MlError> {
    match client()?.call_with(body.clone(), payload, progress, cancel, timeout) {
        Err(MlError::Crashed(why)) => {
            tracing::warn!("ML request failed ({why}); retrying on a new worker");
            client()?.call_with(body, payload, progress, cancel, timeout)
        }
        other => other,
    }
}

/// [`request`], keeping the answer's payload.
pub fn request_payload(
    body: RequestBody,
    payload: &[u8],
    progress: &dyn Fn(f32, &str),
    cancel: Option<&AtomicBool>,
    timeout: Duration,
) -> Result<(Outcome, Vec<u8>), MlError> {
    match client()?.call_for_payload(body.clone(), payload, progress, cancel, timeout) {
        Err(MlError::Crashed(why)) => {
            tracing::warn!("ML request failed ({why}); retrying on a new worker");
            client()?.call_for_payload(body, payload, progress, cancel, timeout)
        }
        other => other,
    }
}

/// Stop the worker, if one runs. The editor calls this on exit; the worker
/// also exits on its own when its stdin closes.
pub fn shutdown() {
    let client = SUPERVISOR.lock().client.take();
    if let Some(client) = client {
        client.shutdown();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The real worker binary, if this build has one; tests that need it
    /// skip with a note otherwise (`cargo build -p chukcut-ml-worker`).
    pub(crate) fn worker_or_skip() -> Option<PathBuf> {
        let found = binary();
        if found.is_none() {
            eprintln!("skipped: no {BINARY} built next to the test binary");
        }
        found
    }

    /// Serialises tests that start, kill and count workers through the one
    /// global supervisor.
    pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn a_missing_binary_is_unavailable_not_an_error_loop() {
        let _serial = SERIAL.lock();
        let result = Client::spawn(&PathBuf::from("/nonexistent/chukcut-ml-worker"));
        assert!(matches!(result, Err(MlError::Unavailable(_))));
    }

    #[test]
    fn the_worker_says_hello_and_comes_back_after_a_crash() {
        let Some(binary) = worker_or_skip() else {
            return;
        };
        let _serial = SERIAL.lock();
        let client = Arc::new(Client::spawn(&binary).unwrap());
        assert!(client.alive());
        let first = client.pid;
        // Kill it the way a native crash would.
        // SAFETY: plain syscall on a pid this test started.
        unsafe { libc::kill(first as i32, libc::SIGKILL) };
        let error = client
            .call_with(
                RequestBody::Hello,
                &[],
                &|_, _| {},
                None,
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(matches!(error, MlError::Crashed(_)), "{error:?}");
        assert!(!client.alive());

        // Through the supervisor: a dead client is replaced on the next call.
        {
            let mut sup = SUPERVISOR.lock();
            sup.client = Some(Arc::clone(&client));
            sup.starts.clear();
        }
        let outcome = request(
            RequestBody::Hello,
            &[],
            &|_, _| {},
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        let Outcome::Hello { pid, .. } = outcome else {
            panic!("{outcome:?}")
        };
        assert_ne!(pid, first);
        shutdown();
    }

    #[test]
    fn a_bad_frame_is_a_clear_error_and_the_worker_keeps_running() {
        let Some(binary) = worker_or_skip() else {
            return;
        };
        let _serial = SERIAL.lock();
        let client = Client::spawn(&binary).unwrap();
        let error = client
            .call_with(
                RequestBody::DetectFaces {
                    model: "yunet".into(),
                    width: 4,
                    height: 4,
                    score_threshold: 0.6,
                },
                &[0; 10],
                &|_, _| {},
                None,
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(
            matches!(&error, MlError::Failed(m) if m.contains("RGBA")),
            "{error:?}"
        );
        assert!(client
            .call_with(
                RequestBody::Hello,
                &[],
                &|_, _| {},
                None,
                Duration::from_secs(5)
            )
            .is_ok());
        client.shutdown();
    }
}
