// The control channel's design — one JSON request per line, one reply per
// line, only well-formed requests read, bounded lines and connections, the
// `engine.*` / `ui.*` method split — is adapted from FilmCraft's
// `apps/filmcraft/src/control_server.rs` and `docs/control-protocol.md`:
//
//   Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
//   Licensed under the Apache License, Version 2.0 (or the MIT License, at
//   your option). https://github.com/storytold/filmcraft
//
// Changes for chukcut: a Unix socket in $XDG_RUNTIME_DIR instead of a
// loopback TCP port, chukcut's operations as the commands, progress lines
// and cancellation for long commands. NOTICE.md lists what was taken.

//! The running app's control socket: `chukcut --control`.
//!
//! The app serves its open project over a Unix socket so `chukcut-cli mcp
//! --bridge` — and any script — can drive the window a person is looking at:
//! run every operation against the app's own document (the edits land in
//! the app's undo history and on screen at once), look at the picture, and
//! work the UI with keys and clicks. This is FilmCraft's bridge mode, built
//! for chukcut's architecture: the operations run here, in the app process,
//! through the same `ops` the CLI runs, against the app's `AppState`.
//!
//! ## Protocol
//!
//! One JSON object per line in each direction:
//!
//! ```text
//! → {"id": 1, "method": "engine.execute", "params": {"command": "split", "params": {"at": 2}}}
//! ← {"id": 1, "progress": 0.4, "label": "…"}           (long commands only, any number)
//! ← {"id": 1, "ok": true, "result": {"message": "…", "data": {…}, "changed": true}}
//! ← {"id": 2, "ok": false, "error": "nothing to split: …"}
//! ```
//!
//! A line that is not a JSON object with a string `method` gets one error
//! reply and the connection is closed; so does a line over [`MAX_LINE`].
//! At most [`MAX_CONNECTIONS`] connections are served at once. The socket
//! is created mode 0600 in a 0700 directory, so only the user can connect.
//!
//! Methods: `ping`, `app.info`, `engine.commands`, `engine.execute`,
//! `engine.cancel`, `doc.inspect`, `project.inspect`, `sequence.inspect`,
//! `frame.render`; every `ui.*` method goes to the app's [`UiHost`].
//! `docs/cli.md`, "The control socket", is the full table.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine as _;
use chukcut_engine::state::AppState;
use serde_json::{json, Value};

use crate::commands;
use crate::inspect;
use crate::ops::Ctx;
use crate::session::Session;

/// The longest request line accepted, in bytes. Requests are small JSON
/// objects; anything longer is refused and the connection closed.
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// Connections served at once; more get an error line and are closed.
pub const MAX_CONNECTIONS: usize = 16;
/// The protocol's version, in `app.info`. Raised when a method changes
/// incompatibly.
pub const PROTOCOL: u32 = 1;

/// What the app does for the `ui.*` methods: they need the window, so they
/// run on its UI thread. The control server calls this from its connection
/// thread and waits for the answer.
pub trait UiHost: Send + Sync + 'static {
    fn call(&self, method: &str, params: Value) -> Result<Value, String>;
}

/// A UI host for a process without a window: every `ui.*` method fails
/// with a sentence that says so.
pub struct NoUi;

impl UiHost for NoUi {
    fn call(&self, method: &str, _: Value) -> Result<Value, String> {
        Err(format!(
            "{method} needs the app's window, and this process has none"
        ))
    }
}

/// The folder the app's sockets live in: `$XDG_RUNTIME_DIR/chukcut`, which
/// is the user's alone and emptied at logout; the cache folder when there
/// is no runtime folder.
pub fn socket_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("chukcut"),
        None => chukcut_engine::modules::workspace::paths::cache_root().join("control"),
    }
}

/// Where this process puts its socket unless told otherwise.
pub fn default_socket_path() -> PathBuf {
    socket_dir().join(format!("control-{}.sock", std::process::id()))
}

/// The socket of a running app: the newest `control-*.sock` that answers.
/// Sockets left behind by an app that ended are removed on the way.
pub fn discover() -> Option<PathBuf> {
    let mut sockets: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(socket_dir())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("control-") && n.ends_with(".sock"))
        })
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    sockets.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, path) in sockets {
        match UnixStream::connect(&path) {
            Ok(_) => return Some(path),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => {}
        }
    }
    None
}

/// A running control server. The socket file goes when it is dropped.
pub struct ControlServer {
    path: PathBuf,
}

impl ControlServer {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// What every connection shares.
struct Shared {
    state: Arc<AppState>,
    ui: Arc<dyn UiHost>,
    on_change: Arc<dyn Fn() + Send + Sync>,
    /// One command at a time across all connections, as in the app's own
    /// UI: two agents' edits interleaved inside one command would be a
    /// document neither of them asked for.
    exec: Mutex<()>,
    /// The cancel flag of every command running now, by its request id.
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

/// Start serving `state` on `path`. `ui` answers the `ui.*` methods;
/// `on_change` is called after every command that changed the document, so
/// the app can redraw.
pub fn start(
    state: Arc<AppState>,
    path: PathBuf,
    ui: Arc<dyn UiHost>,
    on_change: Arc<dyn Fn() + Send + Sync>,
) -> std::io::Result<ControlServer> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    // A socket file left by an app that ended refuses connections; one that
    // answers belongs to a running app and is not taken over.
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("{} is in use by another chukcut", path.display()),
            ));
        }
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let shared = Arc::new(Shared {
        state,
        ui,
        on_change,
        exec: Mutex::new(()),
        running: Mutex::new(HashMap::new()),
    });
    std::thread::Builder::new()
        .name("chukcut-control".into())
        .spawn(move || accept_loop(listener, shared))?;
    Ok(ControlServer { path })
}

fn accept_loop(listener: UnixListener, shared: Arc<Shared>) {
    let open = Arc::new(AtomicUsize::new(0));
    for mut stream in listener.incoming().flatten() {
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            let _ = writeln!(
                stream,
                "{}",
                json!({"ok": false, "error": "too many control connections"})
            );
            continue;
        }
        let shared = Arc::clone(&shared);
        let slot = Slot(Arc::clone(&open));
        let spawned = std::thread::Builder::new()
            .name("chukcut-control-conn".into())
            .spawn(move || {
                let _slot = slot;
                serve(stream, &shared);
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "the control server could not start a connection thread");
        }
    }
}

/// One counted connection; gives its place back when dropped, also when the
/// connection's thread panics.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

type Writer = Arc<Mutex<UnixStream>>;

fn write_line(out: &Writer, value: &Value) -> bool {
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    writeln!(out, "{value}").and_then(|_| out.flush()).is_ok()
}

/// Read one line of at most [`MAX_LINE`] bytes. `Ok(None)` at the end of
/// the stream; an error for a line that is too long or not UTF-8.
fn read_line(reader: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut buf = Vec::new();
    let n = reader
        .by_ref()
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', &mut buf)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > MAX_LINE {
        return Err(format!("a request line is longer than {MAX_LINE} bytes"));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| "a request line is not UTF-8".to_string())
}

fn serve(stream: UnixStream, shared: &Shared) {
    let Ok(write) = stream.try_clone() else {
        return;
    };
    let out: Writer = Arc::new(Mutex::new(write));
    let mut reader = BufReader::new(stream);
    loop {
        let line = match read_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(error) => {
                write_line(
                    &out,
                    &json!({"id": null, "ok": false, "error": format!("{error}; closing the connection")}),
                );
                return;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        // Only requests are read. Anything else — an HTTP request line, a
        // stray `nc`, a JSON array — ends the connection, so nothing sent
        // after it on this connection runs.
        let request = serde_json::from_str::<Value>(&line)
            .ok()
            .filter(|v| v.get("method").is_some_and(Value::is_string));
        let Some(request) = request else {
            write_line(
                &out,
                &json!({"id": null, "ok": false, "error": "not a request (a JSON object with a string \"method\"); closing the connection"}),
            );
            return;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request["method"].as_str().unwrap_or_default().to_string();
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = chukcut_engine::lifecycle::contained("That control request", || {
            handle(shared, &id, &method, params, &out)
        });
        let reply = match result {
            Ok(result) => json!({"id": id, "ok": true, "result": result}),
            Err(error) => json!({"id": id, "ok": false, "error": error}),
        };
        if !write_line(&out, &reply) {
            return;
        }
    }
}

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

fn handle(
    shared: &Shared,
    id: &Value,
    method: &str,
    params: Value,
    out: &Writer,
) -> Result<Value, String> {
    match method {
        "ping" => Ok(json!({})),
        "app.info" => Ok(json!({
            "app": "chukcut",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": PROTOCOL,
            "pid": std::process::id(),
            "project": chukcut_engine::modules::project::commands::project_path(&shared.state),
            "open": shared.state.project.read().is_some(),
        })),
        "engine.commands" => Ok(commands::list(
            str_param(&params, "filter"),
            params.get("enabled_only").and_then(Value::as_bool) == Some(true),
            shared.state.project.read().is_some(),
        )),
        "engine.execute" => execute(shared, id, params, out),
        "engine.cancel" => {
            let key = params
                .get("request")
                .map(Value::to_string)
                .unwrap_or_default();
            let flag = shared
                .running
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .cloned();
            if let Some(flag) = &flag {
                flag.store(true, Ordering::Release);
            }
            Ok(json!({"cancelled": flag.is_some()}))
        }
        "doc.inspect" => {
            let session = Session::attach(Arc::clone(&shared.state)).map_err(|e| e.message)?;
            let mut doc = inspect::document(&session);
            add_ui(shared, &mut doc["sequence"]);
            Ok(doc)
        }
        "project.inspect" => Session::attach(Arc::clone(&shared.state))
            .map(|s| inspect::project(&s))
            .map_err(|e| e.message),
        "sequence.inspect" => {
            let session = Session::attach(Arc::clone(&shared.state)).map_err(|e| e.message)?;
            let mut sequence = inspect::sequence(&session);
            add_ui(shared, &mut sequence);
            Ok(sequence)
        }
        "frame.render" => render(shared, &params),
        m if m.starts_with("ui.") => shared.ui.call(m, params),
        other => Err(format!(
            "unknown method {other:?}; the methods are ping, app.info, engine.commands, \
             engine.execute, engine.cancel, doc.inspect, project.inspect, sequence.inspect, \
             frame.render and ui.*"
        )),
    }
}

/// The playhead and the selection, which live in the window, added to a
/// sequence answer when the window can say them.
fn add_ui(shared: &Shared, sequence: &mut Value) {
    if let (Ok(ui), Some(map)) = (
        shared.ui.call("ui.inspect", json!({})),
        sequence.as_object_mut(),
    ) {
        for key in ["playhead", "playhead_us", "selection", "playing"] {
            if let Some(value) = ui.get(key) {
                map.insert(key.into(), value.clone());
            }
        }
    }
}

fn execute(shared: &Shared, id: &Value, params: Value, out: &Writer) -> Result<Value, String> {
    let command = str_param(&params, "command")
        .ok_or("engine.execute needs \"command\", a command id from engine.commands")?
        .to_string();
    let mut args = params.get("params").cloned().unwrap_or_else(|| json!({}));
    if !commands::is_command(&command) {
        return Err(format!(
            "there is no command called {command:?}; engine.commands lists them"
        ));
    }
    if commands::CREATES_PROJECT.contains(&command.as_str())
        && !(command == "template_apply" && args.get("into").and_then(Value::as_bool) == Some(true))
    {
        return Err(format!(
            "{command} writes a new project file; in the app, use File › New or the start \
             screen, or run it headless with chukcut-cli"
        ));
    }
    // The app's document is the target; a project path in the arguments
    // means nothing here.
    if let Some(map) = args.as_object_mut() {
        map.remove("project");
    }
    let progress_out = Arc::clone(out);
    let progress_id = id.clone();
    let ctx = Ctx::with_progress(Box::new(move |label: &str, fraction: Option<f32>| {
        write_line(
            &progress_out,
            &json!({"id": progress_id, "progress": fraction, "label": label}),
        );
    }));
    let key = id.to_string();
    shared
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.clone(), Arc::clone(&ctx.cancel));
    let result = {
        let _one_at_a_time = shared.exec.lock().unwrap_or_else(|e| e.into_inner());
        if ctx.cancelled() {
            Err("cancelled".to_string())
        } else {
            match commands::run_without_project(&command, args.clone()) {
                Some(result) => result.map_err(|e| e.message),
                None => Session::attach(Arc::clone(&shared.state))
                    .and_then(|mut session| commands::run_on(&command, args, &mut session, &ctx))
                    .map_err(|e| e.message),
            }
        }
    };
    shared
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&key);
    let outcome = result?;
    if outcome.mutated {
        (shared.on_change)();
    }
    Ok(commands::outcome_json(&outcome))
}

fn render(shared: &Shared, params: &Value) -> Result<Value, String> {
    let max_side = params
        .get("max_side")
        .and_then(Value::as_u64)
        .unwrap_or(960)
        .min(u32::MAX as u64) as u32;
    // The frame on the app's screen unless a time is named: the playhead.
    let time = match params.get("seconds").and_then(Value::as_f64) {
        Some(s) if s.is_finite() => (s * 1_000_000.0).round() as i64,
        Some(_) => return Err("seconds is not a finite number".into()),
        None => shared
            .ui
            .call("ui.inspect", json!({}))
            .ok()
            .and_then(|ui| ui.get("playhead_us").and_then(Value::as_i64))
            .unwrap_or(0),
    };
    let png = pollster::block_on(
        chukcut_engine::modules::export::commands::export_preview_png(
            &shared.state,
            time,
            max_side,
        ),
    )?;
    Ok(json!({
        "png": base64::engine::general_purpose::STANDARD.encode(png),
        "at": time as f64 / 1_000_000.0,
        "at_us": time,
    }))
}

/// The app's control server, once started; the process keeps it for its
/// lifetime.
static SERVER: OnceLock<ControlServer> = OnceLock::new();

/// Start the control server for the app process and keep it: at `path`, or
/// at [`default_socket_path`]. Returns the socket's path.
pub fn start_for_process(
    state: Arc<AppState>,
    path: Option<PathBuf>,
    ui: Arc<dyn UiHost>,
    on_change: Arc<dyn Fn() + Send + Sync>,
) -> std::io::Result<PathBuf> {
    let server = start(
        state,
        path.unwrap_or_else(default_socket_path),
        ui,
        on_change,
    )?;
    let path = server.path().to_path_buf();
    SERVER
        .set(server)
        .map_err(|_| std::io::Error::other("the control server is already running"))?;
    Ok(path)
}
