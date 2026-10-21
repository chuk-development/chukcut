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
        Self::start_with(dir, &[])
    }

    fn start_with(dir: &std::path::Path, args: &[&str]) -> Self {
        let mut child = common::cli(dir)
            .arg("mcp")
            .args(args)
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

/// The first design's session: one tool per operation (`--op-tools`), every
/// call naming its project.
#[test]
fn an_mcp_session_edits_undoes_and_looks() {
    require_ffmpeg!();
    let dir = common::scratch("mcp");
    let (card, _) = common::media(&dir);
    let project = dir.join("agent.chukcut");
    let p = project.to_str().unwrap();
    let mut client = Client::start_with(&dir, &["--op-tools"]);

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
        "catalog",
        "command_run",
    ] {
        assert!(names.contains(&expected), "{expected} is offered");
    }
    let split = tools.iter().find(|t| t["name"] == "split").unwrap();
    assert_eq!(split["inputSchema"]["required"], json!(["project", "at"]));

    // The operations added after the first CLI, each with an object schema.
    for (name, required) in [
        ("marker_add", json!(["project", "at"])),
        ("marker_list", json!(["project"])),
        ("crop", json!(["project", "clip"])),
        ("curve", json!(["project", "clip"])),
        ("freeze", json!(["project", "clip", "at"])),
        ("speed_curve", json!(["project", "clip"])),
        ("layout_pip", json!(["project", "clip"])),
        ("layout_split", json!(["project", "clips"])),
        ("title_style", json!(["project", "style"])),
        ("title_template", json!(["project", "template"])),
        ("title_position", json!(["project", "clip", "position"])),
        ("title_duplicate", json!(["project", "clip"])),
        ("scenes_detect", json!(["project", "clip"])),
        ("stabilise", json!(["project", "clip"])),
        ("beats_detect", json!(["project", "clip"])),
        ("beats_cut", json!(["project"])),
        ("reframe", json!(["project"])),
        ("analysis", json!(["project", "clip"])),
        ("translate_captions", json!(["project", "to"])),
        ("tts", json!(["project", "text", "voice"])),
        ("stock_search", json!(["project", "query"])),
        ("stock_download", json!(["project", "query", "id"])),
        ("presets", json!(["project"])),
        ("preset_save", json!(["project", "name"])),
        ("preset_remove", json!(["project", "id"])),
        ("estimate", json!(["project"])),
        ("export_queue", json!(["project"])),
        ("export", json!(["project", "output"])),
        // The FilmCraft ports.
        ("export_timeline", json!(["project", "output"])),
        ("export_markers", json!(["project", "output"])),
        ("sync", json!(["project", "reference", "clips"])),
        ("relink", json!(["project"])),
    ] {
        let tool = tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} is offered"));
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        assert_eq!(tool["inputSchema"]["required"], required, "{name}");
    }
    let read_only = |name: &str| {
        tools.iter().find(|t| t["name"] == name).unwrap()["annotations"]["readOnlyHint"]
            .as_bool()
            .unwrap()
    };
    assert!(read_only("marker_list") && read_only("analysis") && read_only("stock_search"));
    assert!(!read_only("marker_add") && !read_only("scenes_detect"));
    assert!(read_only("presets") && read_only("estimate") && !read_only("export_queue"));
    // The export options are one shared set, flattened into each tool.
    for name in ["export", "estimate", "preset_save"] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        for option in ["preset", "crf", "from", "to", "no_loudness"] {
            assert!(
                tool["inputSchema"]["properties"].get(option).is_some(),
                "{name} takes {option}"
            );
        }
    }
    let queue = tools.iter().find(|t| t["name"] == "export_queue").unwrap();
    assert!(queue["inputSchema"]["properties"].get("jobs").is_some());
    assert!(queue["inputSchema"]["properties"]
        .get("jobs_file")
        .is_none());
    let tts = tools.iter().find(|t| t["name"] == "tts").unwrap();
    assert_eq!(tts["annotations"]["openWorldHint"], true);

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
    let uri = resources["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .find(|u| u.starts_with("chukcut://project?path="))
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

fn text_of(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// The width and height a PNG's header says.
fn png_size(base64_png: &str) -> (u32, u32) {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_png)
        .unwrap();
    assert_eq!(&bytes[1..4], b"PNG");
    let be = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap());
    (be(16), be(20))
}

/// FilmCraft's shape, the default: a few tools, the command catalog behind
/// them, strict arguments, the current project remembered between calls.
#[test]
fn the_core_tools_drive_a_project_through_the_catalog() {
    let dir = common::scratch("mcp-core");
    let project = dir.join("core.chukcut");
    let p = project.to_str().unwrap();
    let mut client = Client::start(&dir);
    let (init, _) = client.request(
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    );
    assert!(init["result"]["capabilities"]["resources"].is_object());
    assert!(init["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("command_list"));
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    // Few tools, every one titled and annotated; no op tools, no ui tools.
    let (listed, _) = client.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(tools.len() < 20, "{names:?}");
    for want in [
        "command_list",
        "command_run",
        "command_batch",
        "doc_inspect",
        "render_preview",
        "project_inspect",
        "sequence_inspect",
        "media_import",
        "render_frame",
        "new_project",
    ] {
        assert!(names.contains(&want), "{want} is listed");
    }
    assert!(!names.contains(&"split") && !names.contains(&"ui_click"));
    for tool in tools {
        assert!(tool["title"].is_string(), "{}", tool["name"]);
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(
                tool["annotations"][hint].is_boolean(),
                "{} {hint}",
                tool["name"]
            );
        }
    }
    let hint = |name: &str, h: &str| {
        tools.iter().find(|t| t["name"] == name).unwrap()["annotations"][h].clone()
    };
    assert_eq!(hint("command_list", "readOnlyHint"), true);
    assert_eq!(hint("command_run", "readOnlyHint"), false);
    assert_eq!(hint("command_run", "destructiveHint"), true);

    // The catalog.
    let found = client.ok("command_list", json!({"filter": "SPLIT"}));
    let split = found
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "split")
        .expect("split is a command");
    assert_eq!(split["group"], "timeline");
    assert_eq!(split["params"]["type"], "object");
    // Nothing is open yet: only the commands that need no project can run.
    let enabled = client.ok("command_list", json!({"enabled_only": true}));
    assert!(enabled
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["needsProject"] == false));

    // Strict arguments: a protocol error naming the bad one and the good ones.
    let (bad, _) = client.request(
        "tools/call",
        json!({"name": "command_list", "arguments": {"filtr": "x"}}),
    );
    assert_eq!(bad["error"]["code"], -32602, "{bad}");
    let message = bad["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("\"filtr\"") && message.contains("enabled_only"),
        "{message}"
    );
    let (bad, _) = client.request(
        "tools/call",
        json!({"name": "doc_inspect", "arguments": {"verbose": true}}),
    );
    assert_eq!(bad["error"]["code"], -32602, "{bad}");
    // A call before any project is named says how to name one.
    let none = client.call("doc_inspect", json!({}));
    assert_eq!(none["isError"], true);
    assert!(text_of_err(&none).contains("project"), "{none}");

    // A line that is not JSON: a parse error, and the server goes on.
    writeln!(client.stdin, "{{\"jsonrpc\": \"2.0\", \"id\": 99, BROKEN").unwrap();
    client.stdin.flush().unwrap();
    let mut line = String::new();
    client.stdout.read_line(&mut line).unwrap();
    let parse: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(parse["error"]["code"], -32700, "{parse}");
    assert!(parse["id"].is_null());
    let (pong, _) = client.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));

    // Name the project once; later calls use it.
    client.ok(
        "new_project",
        json!({"project": p, "width": 640, "height": 360}),
    );
    let added = client.ok(
        "command_run",
        json!({"id": "title_add", "params": {"text": "One", "at": 0, "duration": 2}}),
    );
    assert!(added.is_object(), "{added}");
    let doc = client.ok("doc_inspect", json!({}));
    assert_eq!(doc["project"]["canvas"]["width"], 640);
    assert!(!doc["project"]["timelines"].as_array().unwrap().is_empty());
    let lanes = doc["sequence"]["tracks"].as_array().unwrap();
    assert!(lanes
        .iter()
        .any(|t| t["kind"] == "text" && t["clips"].as_array().unwrap().len() == 1));
    // Saved at once.
    let on_disk = common::ok(&dir, &["info", p]);
    assert!(on_disk["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["clips"].as_array().unwrap().len() == 1));

    // A command's own parameters are checked by the command: a tool error.
    let wrong = client.call(
        "command_run",
        json!({"id": "title_add", "params": {"txt": "x"}}),
    );
    assert_eq!(wrong["isError"], true, "{wrong}");
    assert!(text_of_err(&wrong).contains("txt"), "{wrong}");
    let unknown = client.call("command_run", json!({"id": "teleport"}));
    assert_eq!(unknown["isError"], true);

    // A batch: each step its own undo step; it stops at the first failure
    // and reports both.
    let result = client.call(
        "command_batch",
        json!({"steps": [
            {"id": "title_add", "params": {"text": "Two", "at": 3, "duration": 1}},
            {"id": "no_such_command"},
            {"id": "title_add", "params": {"text": "Three", "at": 5, "duration": 1}},
        ]}),
    );
    assert_eq!(result["isError"], true, "{result}");
    let batch = text_of(&result)["data"].clone();
    assert_eq!(batch["completed"], 1, "{batch}");
    assert_eq!(batch["failed"], 1, "{batch}");
    assert_eq!(batch["results"].as_array().unwrap().len(), 2, "{batch}");
    // Undo takes back the batch's one good step.
    let undone = client.ok("command_run", json!({"id": "undo"}));
    assert!(undone["undone"].is_string(), "{undone}");
    // All or nothing: a failing step leaves the file as it was.
    let before = std::fs::read(&project).unwrap();
    let atomic = client.call(
        "command_batch",
        json!({"atomic": true, "steps": [
            {"id": "title_add", "params": {"text": "Four", "at": 6, "duration": 1}},
            {"id": "split", "params": {"at": 999}},
        ]}),
    );
    assert_eq!(atomic["isError"], true, "{atomic}");
    assert_eq!(std::fs::read(&project).unwrap(), before);

    // Resources: the document and the catalog.
    let (resources, _) = client.request("resources/list", json!({}));
    let uris: Vec<&str> = resources["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert_eq!(&uris[..2], ["chukcut://document", "chukcut://commands"]);
    let (read, _) = client.request("resources/read", json!({"uri": "chukcut://document"}));
    assert_eq!(
        read["result"]["contents"][0]["mimeType"],
        "application/json"
    );
    let doc: Value =
        serde_json::from_str(read["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert!(
        doc["project"].is_object() && doc["sequence"].is_object(),
        "{doc}"
    );
    let (read, _) = client.request("resources/read", json!({"uri": "chukcut://commands"}));
    assert!(read["result"]["contents"][0]["text"]
        .as_str()
        .unwrap()
        .contains("\"split\""));
    let (missing, _) = client.request("resources/read", json!({"uri": "chukcut://nope"}));
    assert!(missing["error"]["code"].is_i64(), "{missing}");

    // A frame, small, when this machine can render one.
    let frame = client.call("render_frame", json!({"seconds": 0.5, "max_side": 64}));
    if frame["isError"] == true {
        eprintln!("skipping the frame check: {}", text_of_err(&frame));
    } else {
        let image = &frame["content"][1];
        assert_eq!(image["type"], "image");
        let (w, h) = png_size(image["data"].as_str().unwrap());
        assert_eq!((w, h), (64, 36));
        let preview = client.call("render_preview", json!({"max_side": 32}));
        assert_eq!(preview["content"][1]["type"], "image");
    }
}

fn text_of_err(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// MCP 2026-07-28 clients (current Claude Code) negotiate per request and
/// need `resultType`, `ttlMs` and `cacheScope` on list and read results;
/// sessions that negotiated an older revision get the old shape.
#[test]
fn per_request_clients_get_result_type_and_cache_hints() {
    let dir = common::scratch("mcp-2026");
    let mut client = Client::start(&dir);
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let (found, _) = client.request("server/discover", json!({"_meta": meta}));
    assert!(found["result"]["supportedVersions"]
        .as_array()
        .unwrap()
        .contains(&json!("2026-07-28")));
    for method in ["tools/list", "resources/list"] {
        let (r, _) = client.request(method, json!({"_meta": meta}));
        assert_eq!(r["result"]["resultType"], "complete", "{method}: {r}");
        assert!(r["result"]["ttlMs"].is_u64(), "{method}: {r}");
        assert_eq!(r["result"]["cacheScope"], "private", "{method}: {r}");
    }
    let (old, _) = client.request("tools/list", json!({}));
    assert!(old["result"].get("resultType").is_none() && old["result"].get("ttlMs").is_none());
}

/// A long export reports progress, `ping` is answered while it runs, and
/// `notifications/cancelled` stops it: no response, no file.
#[test]
fn a_cancelled_export_stops_and_answers_nothing() {
    require_ffmpeg!();
    let dir = common::scratch("mcp-cancel");
    let (card, _) = common::media(&dir);
    let project = dir.join("long.chukcut");
    let p = project.to_str().unwrap();
    let mut client = Client::start(&dir);
    client.request(
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    );
    client.ok(
        "new_project",
        json!({"project": p, "width": 640, "height": 360}),
    );
    // A minute of picture, so the export outlasts the cancel.
    let files: Vec<&std::path::Path> = std::iter::repeat_n(card.as_path(), 15).collect();
    client.ok("media_import", json!({"paths": files, "append": true}));
    let out = dir.join("long.mp4");
    let id = 4242;
    client.send(json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {
            "name": "command_run",
            "arguments": {"id": "export", "params": {"output": out, "crf": 35}},
            "_meta": {"progressToken": "x"},
        },
    }));
    let mut pinged = false;
    loop {
        let mut line = String::new();
        assert!(client.stdout.read_line(&mut line).unwrap() > 0);
        let m: Value = serde_json::from_str(&line).unwrap();
        if m["id"] == id {
            // Finished (or failed: no GPU) before it could be cancelled.
            eprintln!("skipping the cancel check: the export answered first: {m}");
            return;
        }
        if m["id"] == 77 {
            pinged = true;
        }
        if m["method"] == "notifications/progress" && m["params"]["progress"].as_f64() > Some(0.0) {
            if !pinged {
                client.send(json!({"jsonrpc": "2.0", "id": 77, "method": "ping"}));
                continue;
            }
            break;
        }
    }
    assert!(pinged, "ping was answered during the export");
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id, "reason": "test"}}));
    // The next request is answered after the cancelled one ended, and the
    // cancelled one is never answered.
    let (after, seen) = client.request(
        "tools/call",
        json!({"name": "project_inspect", "arguments": {}}),
    );
    assert!(after.get("result").is_some(), "{after}");
    assert!(
        seen.iter().all(|m| m["id"] != id),
        "a cancelled request gets no response: {seen:?}"
    );
    let t0 = std::time::Instant::now();
    while out.exists() && t0.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!out.exists(), "the partial export was deleted");
}

/// The commands ported from FilmCraft are in the catalog and run through
/// `command_run`: timeline and marker export, relink, sync by sound.
#[test]
fn the_filmcraft_ports_run_through_the_catalog() {
    require_ffmpeg!();
    let dir = common::scratch("mcp-ports");
    let (card, bars) = common::media(&dir);
    let project = dir.join("ports.chukcut");
    let p = project.to_str().unwrap();
    let mut client = Client::start(&dir);
    client.request(
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    );

    let catalog = client.ok("command_list", json!({}));
    let entry = |id: &str| {
        catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("{id} is in the catalog"))
            .clone()
    };
    for (id, group) in [
        ("export_timeline", "render"),
        ("export_markers", "render"),
        ("relink", "project"),
        ("sync", "analysis"),
    ] {
        let e = entry(id);
        assert_eq!(e["group"], group, "{id}");
        assert_eq!(e["needsProject"], true, "{id}");
        assert_eq!(e["params"]["type"], "object", "{id}");
    }

    client.ok(
        "new_project",
        json!({"project": p, "width": 640, "height": 360}),
    );
    client.ok(
        "media_import",
        json!({"paths": [card, bars], "append": true}),
    );
    for (at, label) in [(0, "Intro"), (2, "Middle")] {
        client.ok(
            "command_run",
            json!({"id": "marker_add", "params": {"at": at, "label": label}}),
        );
    }

    let otio = dir.join("cut.otio");
    let written = client.ok(
        "command_run",
        json!({"id": "export_timeline", "params": {"output": otio}}),
    );
    assert_eq!(written["clips"], 2, "{written}");
    let text = std::fs::read_to_string(&otio).unwrap();
    assert!(text.contains("OTIO_SCHEMA"), "{text}");
    let edl = dir.join("cut.txt");
    let written = client.ok(
        "command_run",
        json!({"id": "export_timeline", "params": {"output": edl, "format": "edl"}}),
    );
    // The extension follows the format.
    let path = written["path"].as_str().unwrap();
    assert!(path.ends_with("cut.edl"), "{written}");
    assert!(std::fs::read_to_string(path).unwrap().contains("TITLE:"));

    let csv = dir.join("markers.csv");
    let written = client.ok(
        "command_run",
        json!({"id": "export_markers", "params": {"output": csv}}),
    );
    assert_eq!(written["markers"], 2, "{written}");
    assert!(std::fs::read_to_string(&csv).unwrap().contains("Middle"));

    let relink = client.ok("command_run", json!({"id": "relink"}));
    assert_eq!(relink["not_on_disk"], json!([]), "{relink}");

    // Sync is reached: a reference that is no clip is the command's own
    // refusal, not "there is no operation".
    let refused = client.call(
        "command_run",
        json!({"id": "sync", "params": {"reference": "9:9", "clips": ["1:0"]}}),
    );
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(!text_of_err(&refused).contains("no operation"), "{refused}");
}
