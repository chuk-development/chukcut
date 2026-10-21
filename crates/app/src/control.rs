//! `chukcut --control`: the app's end of the control socket.
//!
//! The socket itself — protocol, commands, rendering — is
//! `chukcut_cli::control`, the same operations the CLI and the MCP server
//! run, here against the app's own `AppState`. What lives here is the part
//! only a window can do: the `ui.*` requests cross to the UI thread through
//! a queue the editor's tick drains (`editor/control.rs`), and a command
//! that changed the document raises a flag the tick turns into a redraw.
//!
//! Off unless asked for: `chukcut --control` (the socket goes to
//! `$XDG_RUNTIME_DIR/chukcut/control-<pid>.sock`), `--control=PATH`, or
//! `CHUKCUT_CONTROL=1` / `CHUKCUT_CONTROL=PATH`. Only the user can connect
//! (mode 0600), but whoever can is the user: it edits the open project.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

use chukcut_engine::state::AppState;
use serde_json::Value;

/// How long a `ui.*` request waits for the window. The tick runs every few
/// milliseconds; a window that does not answer in this time is on the start
/// screen (no editor drains the queue) or stuck.
const UI_TIMEOUT: Duration = Duration::from_secs(10);

/// One `ui.*` request on its way to the UI thread.
pub struct UiRequest {
    pub method: String,
    pub params: Value,
    pub reply: mpsc::Sender<Result<Value, String>>,
}

static QUEUE: OnceLock<Mutex<mpsc::Receiver<UiRequest>>> = OnceLock::new();
static CHANGED: AtomicBool = AtomicBool::new(false);

struct Host {
    tx: Mutex<mpsc::Sender<UiRequest>>,
}

impl chukcut_cli::control::UiHost for Host {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let (reply, answer) = mpsc::channel();
        let request = UiRequest {
            method: method.to_string(),
            params,
            reply,
        };
        self.tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(request)
            .map_err(|_| "the app's window is gone".to_string())?;
        answer.recv_timeout(UI_TIMEOUT).unwrap_or_else(|_| {
            Err(format!(
                "the window did not answer {method} within {} s; is a project open (not the \
                 start screen)?",
                UI_TIMEOUT.as_secs()
            ))
        })
    }
}

/// Where the command line or the environment asks for the socket:
/// `Some(None)` for the default place, `Some(Some(path))` for a path,
/// `None` when the socket is not wanted.
fn requested() -> Option<Option<PathBuf>> {
    for argument in std::env::args().skip(1) {
        if argument == "--control" {
            return Some(None);
        }
        if let Some(path) = argument.strip_prefix("--control=") {
            return Some(Some(PathBuf::from(path)));
        }
    }
    match std::env::var("CHUKCUT_CONTROL") {
        Ok(v) if v.is_empty() || v == "0" => None,
        Ok(v) if v == "1" => Some(None),
        Ok(v) => Some(Some(PathBuf::from(v))),
        Err(_) => None,
    }
}

/// Open the control socket when it was asked for. Says where on stderr and
/// in the log; a socket that cannot be opened is reported and the app runs
/// without it.
pub fn start(state: &Arc<AppState>) {
    let Some(path) = requested() else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    let _ = QUEUE.set(Mutex::new(rx));
    let host = Arc::new(Host { tx: Mutex::new(tx) });
    let on_change = Arc::new(|| CHANGED.store(true, Ordering::Release));
    match chukcut_cli::control::start_for_process(Arc::clone(state), path, host, on_change) {
        Ok(path) => {
            eprintln!("chukcut: control socket at {}", path.display());
            tracing::info!(path = %path.display(), "control socket open");
        }
        Err(error) => {
            eprintln!("chukcut: the control socket did not open: {error}");
            tracing::warn!(%error, "control socket did not open");
        }
    }
}

/// The `ui.*` requests waiting for the window.
pub fn take_requests() -> Vec<UiRequest> {
    let Some(queue) = QUEUE.get() else {
        return Vec::new();
    };
    let queue = queue.lock().unwrap_or_else(|e| e.into_inner());
    std::iter::from_fn(|| queue.try_recv().ok()).collect()
}

/// Whether a command over the socket changed the document since the last
/// call; the editor then takes a fresh snapshot.
pub fn take_changed() -> bool {
    CHANGED.swap(false, Ordering::AcqRel)
}
