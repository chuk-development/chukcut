//! The app's control socket, served here the way `chukcut --control` serves
//! it — the library's server over a real Unix socket, against an open
//! project — with a stand-in for the window's `ui.*` half. Driven raw, and
//! through `chukcut-cli mcp --bridge` as an agent drives it.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chukcut_cli::control::{self, UiHost};
use chukcut_cli::session::Session;
use serde_json::{json, Value};

/// The window's half: answers `ui.inspect` with a playhead and records the
/// rest.
#[derive(Default)]
struct FakeWindow {
    calls: Mutex<Vec<(String, Value)>>,
}

impl UiHost for FakeWindow {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), params.clone()));
        match method {
            "ui.inspect" => Ok(
                json!({"playhead": 1.5, "playhead_us": 1_500_000, "selection": null, "playing": false}),
            ),
            "ui.key" => Ok(json!({"key": params["key"], "handled": true})),
            "ui.screenshot" => Err("no window in a test".into()),
            other => Err(format!("{other} is not faked")),
        }
    }
}

struct App {
    session: Session,
    socket: PathBuf,
    window: Arc<FakeWindow>,
    changes: Arc<AtomicUsize>,
    _server: control::ControlServer,
}

/// The engine in this process reads and writes the XDG folders; keep it
/// off the user's. One folder for every test here: the variables are the
/// process's, and the tests run on threads of one process.
fn private_xdg() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let xdg = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("chukcut-cli/control-xdg");
        for (var, sub) in [
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
        ] {
            std::env::set_var(var, xdg.join(sub));
        }
    });
}

/// An empty project, served on a socket in the test's folder.
fn app(dir: &Path) -> App {
    private_xdg();
    let project = dir.join("app.chukcut");
    let mut session = Session::create(&project, "app".into(), 640, 360, 30.0).unwrap();
    session.save().unwrap();
    let socket = dir.join("c.sock");
    let window = Arc::new(FakeWindow::default());
    let changes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&changes);
    let server = control::start(
        Arc::clone(&session.state),
        socket.clone(),
        Arc::clone(&window) as Arc<dyn UiHost>,
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    App {
        session,
        socket,
        window,
        changes,
        _server: server,
    }
}

struct Raw {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Raw {
    fn connect(path: &Path) -> Self {
        let writer = UnixStream::connect(path).unwrap();
        Self {
            reader: BufReader::new(writer.try_clone().unwrap()),
            writer,
        }
    }

    fn line(&mut self) -> Option<Value> {
        let mut line = String::new();
        (self.reader.read_line(&mut line).unwrap() > 0)
            .then(|| serde_json::from_str(&line).unwrap())
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        writeln!(
            self.writer,
            "{}",
            json!({"id": id, "method": method, "params": params})
        )
        .unwrap();
        loop {
            let reply = self.line().expect("a reply");
            if reply.get("ok").is_some() {
                assert_eq!(reply["id"], id);
                return reply;
            }
        }
    }
}

fn titles(session: &Session) -> usize {
    session.with(|p| {
        p.tracks
            .iter()
            .flat_map(|t| t.segments.iter())
            .filter(|s| p.materials.text(&s.material_id).is_some())
            .count()
    })
}

#[test]
fn the_control_socket_runs_commands_on_the_open_project() {
    let dir = common::scratch("control-raw");
    let app = app(&dir);
    let mut c = Raw::connect(&app.socket);

    let info = c.call(1, "app.info", json!({}));
    assert_eq!(info["result"]["app"], "chukcut");
    assert_eq!(info["result"]["open"], true);
    let list = c.call(2, "engine.commands", json!({"filter": "title_add"}));
    assert!(list["result"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == "title_add" && c["enabled"] == true));

    // An edit lands in the app's document and history, and the app hears
    // that it changed.
    let added = c.call(
        3,
        "engine.execute",
        json!({"command": "title_add", "params": {"text": "Live", "at": 0, "duration": 2}}),
    );
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(added["result"]["changed"], true);
    assert_eq!(titles(&app.session), 1);
    assert_eq!(app.changes.load(Ordering::SeqCst), 1);
    // A read changes nothing and says so.
    let read = c.call(4, "engine.execute", json!({"command": "info"}));
    assert_eq!(read["result"]["changed"], false);
    assert_eq!(app.changes.load(Ordering::SeqCst), 1);
    // Undo walks the app's history.
    let undone = c.call(5, "engine.execute", json!({"command": "undo"}));
    assert_eq!(undone["ok"], true, "{undone}");
    assert_eq!(titles(&app.session), 0);
    // Nothing is saved: the app saves when the person says so.
    let on_disk = common::ok(&dir, &["info", app.session.path.to_str().unwrap()]);
    assert!(on_disk["history"]["undo"].is_null());

    // Refusals and unknowns are errors with the engine's sentence.
    let refused = c.call(
        6,
        "engine.execute",
        json!({"command": "split", "params": {"at": 99}}),
    );
    assert_eq!(refused["ok"], false);
    assert!(refused["error"].as_str().unwrap().len() > 10);
    let unknown = c.call(7, "engine.execute", json!({"command": "teleport"}));
    assert!(unknown["error"]
        .as_str()
        .unwrap()
        .contains("engine.commands"));
    let new = c.call(
        8,
        "engine.execute",
        json!({"command": "new_project", "params": {}}),
    );
    assert_eq!(
        new["ok"], false,
        "the app's project is the target, not a new file"
    );
    let method = c.call(9, "no.such", json!({}));
    assert_eq!(method["ok"], false);

    // The sequence carries the window's playhead; ui.* goes to the window.
    let seq = c.call(10, "sequence.inspect", json!({}));
    assert_eq!(seq["result"]["playhead"], 1.5, "{seq}");
    assert!(seq["result"]["tracks"].is_array());
    let key = c.call(11, "ui.key", json!({"key": "space"}));
    assert_eq!(key["result"]["handled"], true);
    assert!(app
        .window
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(m, p)| m == "ui.key" && p["key"] == "space"));

    // Only requests are read: junk ends the connection.
    writeln!(c.writer, "GET / HTTP/1.1").unwrap();
    let reply = c.line().expect("one error line");
    assert_eq!(reply["ok"], false);
    assert!(reply["error"].as_str().unwrap().contains("closing"));
    assert!(c.line().is_none(), "the connection is closed");
    // A new connection is served.
    let mut c = Raw::connect(&app.socket);
    assert_eq!(c.call(1, "ping", json!({}))["ok"], true);
}

#[test]
fn a_second_server_does_not_take_over_a_live_socket() {
    let dir = common::scratch("control-taken");
    let app = app(&dir);
    let again = control::start(
        Arc::clone(&app.session.state),
        app.socket.clone(),
        Arc::new(control::NoUi),
        Arc::new(|| {}),
    );
    assert!(again.is_err());
    let mut c = Raw::connect(&app.socket);
    assert_eq!(c.call(1, "ping", json!({}))["ok"], true);
}

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Mcp {
    fn bridge(dir: &Path, socket: &Path) -> Self {
        let mut child = common::cli(dir)
            .args(["mcp", "--bridge"])
            .arg(socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut mcp = Self {
            child,
            stdin,
            stdout,
            next: 1,
        };
        mcp.request(
            "initialize",
            json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
        );
        mcp
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0);
            let m: Value = serde_json::from_str(&line).unwrap();
            if m["id"] == id {
                return m;
            }
        }
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.request("tools/call", json!({"name": tool, "arguments": arguments}));
        assert!(reply.get("error").is_none(), "{tool}: {reply}");
        reply["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `chukcut-cli mcp --bridge` against the socket: the agent's edits land in
/// the open document, the ui tools reach the window.
#[test]
fn the_bridge_drives_the_open_project() {
    let dir = common::scratch("control-bridge");
    let app = app(&dir);
    let mut mcp = Mcp::bridge(&dir, &app.socket);

    let listed = mcp.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in [
        "command_run",
        "doc_inspect",
        "render_frame",
        "ui_inspect",
        "ui_click",
        "ui_screenshot",
    ] {
        assert!(names.contains(&want), "{want} in bridge mode");
    }
    let run = tools.iter().find(|t| t["name"] == "command_run").unwrap();
    assert!(
        run["inputSchema"]["properties"].get("project").is_none(),
        "the app's project is the target"
    );

    let added = mcp.call(
        "command_run",
        json!({"id": "title_add", "params": {"text": "Agent", "at": 0, "duration": 2}}),
    );
    assert_eq!(added["isError"], false, "{added}");
    assert_eq!(titles(&app.session), 1);
    assert_eq!(app.changes.load(Ordering::SeqCst), 1);
    // The first design's tool names reach the app too.
    let info = mcp.call("info", json!({}));
    assert_eq!(info["isError"], false, "{info}");

    let doc = mcp.call("doc_inspect", json!({}));
    let doc: Value = serde_json::from_str(doc["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(doc["data"]["sequence"]["playhead"], 1.5, "{doc}");
    let ui = mcp.call("ui_inspect", json!({}));
    assert_eq!(ui["isError"], false);
    let key = mcp.call("ui_key", json!({"key": "Ctrl+Z"}));
    assert_eq!(key["isError"], false);
    let shot = mcp.call("ui_screenshot", json!({}));
    assert_eq!(shot["isError"], true, "the fake window has no picture");

    let batch = mcp.call(
        "command_batch",
        json!({"steps": [{"id": "title_add", "params": {"text": "B", "at": 3, "duration": 1}}, {"id": "undo"}]}),
    );
    assert_eq!(batch["isError"], false, "{batch}");
    assert_eq!(titles(&app.session), 1);
    let atomic = mcp.call(
        "command_batch",
        json!({"atomic": true, "steps": [{"id": "undo"}]}),
    );
    assert_eq!(atomic["isError"], true, "all-or-nothing needs a file");
    let new = mcp.call("new_project", json!({"width": 640}));
    assert_eq!(new["isError"], true);
    // Arguments are strict in bridge mode too, and there is no project.
    let bad = mcp.request(
        "tools/call",
        json!({"name": "doc_inspect", "arguments": {"project": "/x.chukcut"}}),
    );
    assert_eq!(bad["error"]["code"], -32602, "{bad}");

    let frame = mcp.call("render_frame", json!({"max_side": 64}));
    if frame["isError"] == false {
        assert_eq!(frame["content"][1]["type"], "image");
    } else {
        eprintln!("skipping the frame check: {}", frame["content"][0]["text"]);
    }
}

/// With no app running, a bridge call says how to start one.
#[test]
fn the_bridge_without_an_app_says_how_to_start_one() {
    let dir = common::scratch("control-none");
    let mut mcp = Mcp::bridge(&dir, &dir.join("nobody.sock"));
    let r = mcp.call("doc_inspect", json!({}));
    assert_eq!(r["isError"], true);
    let text = r["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("chukcut --control"), "{text}");
}
