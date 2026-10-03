//! An MCP session over a pipe, the way Claude Code drives the server:
//! initialize, list the tools, then a sequence of calls that edit, undo and
//! look at a project.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

use serde_json::{json, Value};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Client {
    fn start(dir: &std::path::Path) -> Self {
        let mut child = common::cli(dir)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the server starts");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next: 1,
        }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Send a request and return its response, collecting the notifications
    /// that came before it.
    fn request(&mut self, method: &str, params: Value) -> (Value, Vec<Value>) {
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut notifications = Vec::new();
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).unwrap();
            assert!(n > 0, "the server closed the pipe during {method}");
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("not JSON-RPC ({e}): {line}"));
            assert_eq!(message["jsonrpc"], "2.0");
            if message.get("id") == Some(&json!(id)) {
                return (message, notifications);
            }
            notifications.push(message);
        }
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let (reply, _) = self.request("tools/call", json!({"name": tool, "arguments": arguments}));
        assert!(reply.get("error").is_none(), "{tool}: {reply}");
        reply["result"].clone()
    }

    /// Call a tool that must succeed; returns its structured data.
    fn ok(&mut self, tool: &str, arguments: Value) -> Value {
        let result = self.call(tool, arguments);
        assert_eq!(result["isError"], false, "{tool} failed: {result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            parsed, result["structuredContent"],
            "text and structure agree"
        );
        parsed["data"].clone()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn an_mcp_session_edits_undoes_and_looks() {
    require_ffmpeg!();
    let dir = common::scratch("mcp");
    let (card, _) = common::media(&dir);
    let project = dir.join("agent.chukcut");
    let p = project.to_str().unwrap();
    let mut client = Client::start(&dir);

    let (init, _) = client.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"},
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "chukcut");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let (listed, _) = client.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
        "new_project",
        "info",
        "import",
        "split",
        "grade",
        "title_add",
        "export",
        "render_frame",
        "undo",
        "redo",
        "view_frame",
        "batch",
        "catalog",
    ] {
        assert!(names.contains(&expected), "{expected} is offered");
    }
    let split = tools.iter().find(|t| t["name"] == "split").unwrap();
    assert_eq!(split["inputSchema"]["required"], json!(["project", "at"]));

    client.ok(
        "new_project",
        json!({"project": p, "width": 1080, "height": 1920}),
    );
    let imported = client.ok(
        "import",
        json!({"project": p, "files": [card], "append": true}),
    );
    assert_eq!(imported["materials"][0]["kind"], "video");

    let split = client.ok("split", json!({"project": p, "at": "2s"}));
    let right = split["created"][0]["id"].as_str().unwrap().to_string();
    client.ok(
        "grade",
        json!({"project": p, "clip": right, "set": {"exposure": 0.7}}),
    );
    client.ok(
        "title_add",
        json!({"project": p, "text": "From an agent", "at": 0.5}),
    );

    // Saved after every edit: a separate process sees it.
    let on_disk = common::ok(&dir, &["info", p]);
    assert_eq!(on_disk["tracks"][0]["clips"].as_array().unwrap().len(), 2);
    assert_eq!(on_disk["tracks"][2]["clips"][0]["source"], "From an agent");

    // Undo walks back across calls, and is saved too.
    let undone = client.ok("undo", json!({"project": p}));
    assert!(undone["undone"].is_string());
    let info = client.ok("info", json!({"project": p}));
    assert_eq!(info["history"]["undo"], "Adjust colour");
    assert!(info["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["kind"] != "text" || t["clips"].as_array().unwrap().is_empty()));
    client.ok("redo", json!({"project": p}));

    // A refused edit is a tool error, not a protocol error, and changes nothing.
    let before = std::fs::read(&project).unwrap();
    let refused = client.call("split", json!({"project": p, "at": 60}));
    assert_eq!(refused["isError"], true);
    assert!(refused["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("split"));
    assert_eq!(std::fs::read(&project).unwrap(), before);

    // An unknown tool is a protocol error.
    let (unknown, _) = client.request("tools/call", json!({"name": "teleport", "arguments": {}}));
    assert_eq!(unknown["error"]["code"], -32602);

    // The catalog needs no project.
    let effects = client.ok("catalog", json!({"kind": "effects"}));
    assert!(effects.as_array().unwrap().len() > 10);

    // Resources: the summary of every project this connection opened.
    let (resources, _) = client.request("resources/list", json!({}));
    let uri = resources["result"]["resources"][0]["uri"]
        .as_str()
        .unwrap()
        .to_string();
    let (read, _) = client.request("resources/read", json!({"uri": uri}));
    let summary: Value =
        serde_json::from_str(read["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(summary["canvas"]["width"], 1920);

    // A frame as an image, when this machine can render one.
    let frame = client.call("view_frame", json!({"project": p, "at": 1}));
    if frame["isError"] == true {
        let text = frame["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("no GPU"), "{text}");
        eprintln!("skipping the frame check: no GPU on this machine");
    } else {
        let image = &frame["content"][1];
        assert_eq!(image["type"], "image");
        assert_eq!(image["mimeType"], "image/png");
        assert!(
            image["data"].as_str().unwrap().starts_with("iVBORw0KGgo"),
            "a PNG"
        );
    }

    // Progress notifications arrive before the reply when a token is sent.
    let (exported, notes) = client.request(
        "tools/call",
        json!({
            "name": "export",
            "arguments": {"project": p, "output": dir.join("agent.mp4"), "crf": 30},
            "_meta": {"progressToken": "export-1"},
        }),
    );
    if exported["result"]["isError"] == false {
        assert!(
            notes.iter().any(|n| n["method"] == "notifications/progress"
                && n["params"]["progressToken"] == "export-1"),
            "progress was reported"
        );
        assert!(dir.join("agent.mp4").is_file());
    }

    let (pong, _) = client.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));
}

#[test]
fn an_unknown_protocol_version_gets_the_newest() {
    let dir = common::scratch("mcp-version");
    let mut client = Client::start(&dir);
    let (init, _) = client.request(
        "initialize",
        json!({"protocolVersion": "1999-01-01", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    let (missing, _) = client.request("no/such/method", json!({}));
    assert_eq!(missing["error"]["code"], -32601);
}
