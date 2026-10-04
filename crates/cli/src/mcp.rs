//! `chukcut-cli mcp`: the operations as MCP tools, over stdio.
//!
//! MCP's stdio transport is newline-delimited JSON-RPC 2.0, and the part a
//! tool server needs — `initialize`, `tools/list`, `tools/call`, resources and
//! progress notifications — is a few hundred lines. That is less than the
//! dependency it would replace: the official Rust SDK (`rmcp`, Apache-2.0)
//! brings tokio and an async runtime into a binary that has none, for a
//! server that answers one request at a time. Written here, against the
//! specification, versions 2024-11-05 through 2025-11-25.
//!
//! ## Sessions
//!
//! A connection keeps each project it touches open, so `undo` and `redo` walk
//! everything the agent did to it in this connection. Every successful edit is
//! saved to the file at once — an agent that stops mid-task leaves a
//! consistent project behind — and a failed one is rolled back. A file
//! changed on disk by someone else is reloaded before the next call, which
//! starts its history afresh.
//!
//! Requests are handled one at a time, in order. A long export holds the
//! connection until it finishes; its progress comes as
//! `notifications/progress` when the client sent a progress token.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::error::{CliError, CliResult};
use crate::ops::ml::MlArgs;
use crate::ops::project::{CatalogArgs, NewArgs};
use crate::ops::render::render_png;
use crate::ops::template::{TemplateApplyArgs, TemplateDeleteArgs, TemplateListArgs};
use crate::ops::{self, summary, Ctx, Outcome};
use crate::session::{absolute, Session};
use crate::values::Time;

/// Newest first. The first is what a client that asks for something else
/// is offered.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "chukcut is a video editor. Every tool works on a project file \
(.chukcut) named by `project`, an absolute path. Start with `new_project` or `info`; `info` lists \
every clip with a `ref` (lane:index) and an `id`, and other tools accept either. Edits are saved \
to the file immediately and can be undone with `undo` for as long as this connection lasts. \
Times are seconds as numbers, or strings like \"2.5s\", \"250ms\", \"1:02.5\", \"45f\". \
`catalog` lists effects, transitions, animations, grade controls and export presets. \
`presets` shows how each export preset fits the project; `export_queue` runs several exports. \
`view_frame` shows the picture at a time.";

/// The tools whose operation only reads: MCP's `readOnlyHint`.
const READ_ONLY: &[&str] = &[
    "info",
    "template_list",
    "template_slots",
    "validate",
    "captions_list",
    "silence_detect",
    "loudness",
    "catalog",
    "view_frame",
    "marker_list",
    "analysis",
    "stock_kinds",
    "stock_search",
    "presets",
    "estimate",
    // Reads the clip's faces; writes only the landmark cache.
    "face_landmarks",
    // Reads the clip's people; writes only the landmark cache.
    "body_landmarks",
];

/// The tools that talk to a service outside this machine: MCP's
/// `openWorldHint`.
const OPEN_WORLD: &[&str] = &[
    "captions_transcribe",
    "translate_captions",
    "tts",
    "stock_kinds",
    "stock_search",
    "stock_download",
    "ml",
    // Downloads the model and ONNX Runtime on first use.
    "remove_background",
    "select_object",
    "apply_to",
    // Download RIFE on first use (`--mode flow`).
    "frame_blend",
    "smooth_slow_mo",
    // Download HTDemucs or the face mesh on first use.
    "isolate_voice",
    "face_landmarks",
    "retouch",
    "follow_face",
    // Download RTMPose and the person detector on first use.
    "body_landmarks",
    "follow_body",
    // Download LaMa or Real-ESRGAN on first use.
    "remove_object",
    "enhance_quality",
    "sound",
    "fal",
    "sticker",
    "music",
    "sfx",
    "title_font",
    "catalog",
];

type Out = Arc<Mutex<std::io::Stdout>>;

fn send(out: &Out, message: &Value) {
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    let _ = writeln!(out, "{message}");
    let _ = out.flush();
}

struct Server {
    out: Out,
    sessions: HashMap<PathBuf, Session>,
    protocol: &'static str,
}

/// A JSON-RPC error.
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }
}

pub fn serve() -> std::io::Result<()> {
    let mut server = Server {
        out: Arc::new(Mutex::new(std::io::stdout())),
        sessions: HashMap::new(),
        protocol: PROTOCOL_VERSIONS[0],
    };
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(
                    &server.out,
                    &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}}),
                );
                continue;
            }
        };
        // Batches were in the 2025-03-26 revision; answer them as a batch.
        if let Value::Array(items) = message {
            let replies: Vec<Value> = items.into_iter().filter_map(|m| server.handle(m)).collect();
            if !replies.is_empty() {
                send(&server.out, &Value::Array(replies));
            }
            continue;
        }
        if let Some(reply) = server.handle(message) {
            send(&server.out, &reply);
        }
    }
    Ok(())
}

impl Server {
    /// Handle one message; `None` for a notification or a response.
    fn handle(&mut self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response to something we never ask, or garbage.
            return id.map(|id| {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32600, "message": "invalid request"}})
            });
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => self.call(&params),
            "resources/list" => Ok(self.resources()),
            "resources/templates/list" => Ok(templates()),
            "resources/read" => self.read(&params),
            "prompts/list" => Ok(json!({"prompts": []})),
            m if m.starts_with("notifications/") => return None,
            other => Err(RpcError {
                code: -32601,
                message: format!("method not found: {other}"),
            }),
        };
        let id = id?;
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(e) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.code, "message": e.message}})
            }
        })
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let asked = params.get("protocolVersion").and_then(Value::as_str);
        self.protocol = PROTOCOL_VERSIONS
            .iter()
            .copied()
            .find(|v| Some(*v) == asked)
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": self.protocol,
            "capabilities": {
                "tools": {"listChanged": false},
                "resources": {"subscribe": false, "listChanged": false},
            },
            "serverInfo": {
                "name": "chukcut",
                "title": "chukcut video editor",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": INSTRUCTIONS,
        })
    }

    /// The open session for `path`, opened or reloaded as needed.
    fn session(&mut self, path: &Path) -> CliResult<&mut Session> {
        let path = absolute(path);
        let stale = self
            .sessions
            .get(&path)
            .is_some_and(|s| s.changed_on_disk() || !s.path.exists());
        if stale {
            self.sessions.remove(&path);
        }
        if !self.sessions.contains_key(&path) {
            let session = Session::open(&path)?;
            self.sessions.insert(path.clone(), session);
        }
        Ok(self.sessions.get_mut(&path).expect("inserted"))
    }

    fn call(&mut self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("tools/call needs a name"))?
            .to_string();
        if !tool_names().contains(&name.as_str()) {
            return Err(RpcError::invalid_params(format!("unknown tool: {name}")));
        }
        let mut args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let token = params
            .get("_meta")
            .and_then(|m| m.get("progressToken"))
            .cloned();
        let ctx = self.progress_ctx(token);

        let outcome = self.run_tool(&name, &mut args, &ctx);
        Ok(self.tool_result(&name, outcome))
    }

    fn progress_ctx(&self, token: Option<Value>) -> Ctx {
        match token {
            None => Ctx::quiet(),
            Some(token) => {
                let out = Arc::clone(&self.out);
                Ctx {
                    progress: Box::new(move |label: &str, fraction: Option<f32>| {
                        let mut params = json!({
                            "progressToken": token,
                            "progress": fraction.unwrap_or(0.0),
                            "message": label,
                        });
                        if fraction.is_some() {
                            params["total"] = json!(1.0);
                        }
                        send(
                            &out,
                            &json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": params}),
                        );
                    }),
                }
            }
        }
    }

    fn run_tool(&mut self, name: &str, args: &mut Value, ctx: &Ctx) -> CliResult<ToolOutput> {
        if name == "catalog" {
            let args: CatalogArgs =
                serde_json::from_value(args.clone()).map_err(|e| CliError::usage(e.to_string()))?;
            return args.run().map(ToolOutput::text);
        }
        if name == "ml" {
            let args: MlArgs =
                serde_json::from_value(args.clone()).map_err(|e| CliError::usage(e.to_string()))?;
            return args.run().map(ToolOutput::text);
        }
        // Templates that are not about one project file.
        match name {
            "template_list" => return TemplateListArgs::default().run().map(ToolOutput::text),
            "template_delete" => {
                let args: TemplateDeleteArgs = serde_json::from_value(args.clone())
                    .map_err(|e| CliError::usage(e.to_string()))?;
                return args.run().map(ToolOutput::text);
            }
            _ => {}
        }
        let project = take_project(args)?;
        // `template_apply` with "into" is the operation on an existing
        // project; everything else about it writes a new file.
        let into =
            name == "template_apply" && args.get("into").and_then(Value::as_bool) == Some(true);
        let name = if into { "template_apply_into" } else { name };
        match name {
            "template_apply" => {
                let args: TemplateApplyArgs = serde_json::from_value(args.clone())
                    .map_err(|e| CliError::usage(e.to_string()))?;
                let (mut session, outcome) = args.create(&project)?;
                session.save()?;
                self.sessions.insert(session.path.clone(), session);
                Ok(ToolOutput::text(outcome))
            }
            "new_project" => {
                let args: NewArgs = serde_json::from_value(args.clone())
                    .map_err(|e| CliError::usage(e.to_string()))?;
                let (mut session, outcome) = args.create(&project)?;
                session.save()?;
                self.sessions.insert(session.path.clone(), session);
                Ok(ToolOutput::text(outcome))
            }
            "view_frame" => {
                let at: Time = serde_json::from_value(args.get("at").cloned().unwrap_or(json!(0)))
                    .map_err(|e| CliError::usage(format!("at: {e}")))?;
                let session = self.session(&project)?;
                let png = frame_png(session, at.resolve(session.fps()))?;
                Ok(ToolOutput {
                    outcome: Outcome::read(
                        format!("the frame at {at}"),
                        json!({"at": crate::values::seconds(at.resolve(session.fps()))}),
                    ),
                    image: Some(png),
                })
            }
            "batch" => {
                let list = args
                    .get("ops")
                    .cloned()
                    .ok_or_else(|| CliError::usage("batch needs an \"ops\" list"))?;
                let ops = crate::batch_ops(&list.to_string())?;
                // The batch saves once at the end; a session held here would
                // not see that history, so it is reopened afterwards.
                self.sessions.remove(&absolute(&project));
                let (outcome, _) = crate::run_batch(&project, ops, false, ctx)?;
                Ok(ToolOutput::text(outcome))
            }
            _ => {
                let session = self.session(&project)?;
                let before = serde_json::to_string(&session.project()).unwrap_or_default();
                let result = ops::run_named(name, args.clone(), session, ctx).and_then(|outcome| {
                    if outcome.mutated {
                        session.save()?;
                    }
                    Ok(outcome)
                });
                if result.is_err() {
                    let after = serde_json::to_string(&session.project()).unwrap_or_default();
                    if after != before {
                        // A refused edit must not stay half applied in memory
                        // where the next successful one would save it.
                        let path = session.path.clone();
                        self.sessions.remove(&path);
                    }
                }
                result.map(ToolOutput::text)
            }
        }
    }

    fn tool_result(&self, name: &str, output: CliResult<ToolOutput>) -> Value {
        match output {
            Ok(output) => {
                let mut content = vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&json!({
                        "message": output.outcome.message,
                        "data": output.outcome.data,
                    }))
                    .unwrap_or_default(),
                })];
                if let Some(png) = output.image {
                    content.push(json!({
                        "type": "image",
                        "data": base64::engine::general_purpose::STANDARD.encode(png),
                        "mimeType": "image/png",
                    }));
                }
                let mut result = json!({"content": content, "isError": false});
                if self.protocol >= "2025-06-18" {
                    result["structuredContent"] = json!({
                        "message": output.outcome.message,
                        "data": output.outcome.data,
                    });
                }
                result
            }
            Err(error) => json!({
                "content": [{"type": "text", "text": format!("{name} failed ({:?}): {}", error.kind, error.message)}],
                "isError": true,
            }),
        }
    }

    fn resources(&self) -> Value {
        let mut list = Vec::new();
        for path in self.sessions.keys() {
            let p = path.to_string_lossy();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            list.push(json!({
                "uri": format!("chukcut://project?path={}", encode(&p)),
                "name": format!("{name} (summary)"),
                "mimeType": "application/json",
            }));
            list.push(json!({
                "uri": format!("chukcut://frame?path={}&at=0", encode(&p)),
                "name": format!("{name} (first frame)"),
                "mimeType": "image/png",
            }));
        }
        json!({"resources": list})
    }

    fn read(&mut self, params: &Value) -> Result<Value, RpcError> {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("resources/read needs a uri"))?
            .to_string();
        let (kind, query) = parse_uri(&uri)
            .ok_or_else(|| RpcError::invalid_params(format!("not a chukcut resource: {uri}")))?;
        let path = query
            .get("path")
            .ok_or_else(|| RpcError::invalid_params("the resource needs ?path="))?;
        let not_found = |e: CliError| RpcError {
            code: -32002,
            message: e.message,
        };
        let session = self.session(Path::new(path)).map_err(not_found)?;
        match kind.as_str() {
            "project" => {
                let data = summary::project(&session.project(), &session.state, &session.path);
                Ok(json!({"contents": [{
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": serde_json::to_string_pretty(&data).unwrap_or_default(),
                }]}))
            }
            "frame" => {
                let at: Time = query
                    .get("at")
                    .map(|a| a.parse::<Time>())
                    .transpose()
                    .map_err(RpcError::invalid_params)?
                    .unwrap_or(Time::Micros(0));
                let png = frame_png(session, at.resolve(session.fps())).map_err(not_found)?;
                Ok(json!({"contents": [{
                    "uri": uri,
                    "mimeType": "image/png",
                    "blob": base64::engine::general_purpose::STANDARD.encode(png),
                }]}))
            }
            other => Err(RpcError::invalid_params(format!(
                "unknown resource kind {other}"
            ))),
        }
    }
}

struct ToolOutput {
    outcome: Outcome,
    image: Option<Vec<u8>>,
}

impl ToolOutput {
    fn text(outcome: Outcome) -> Self {
        Self {
            outcome,
            image: None,
        }
    }
}

fn take_project(args: &mut Value) -> CliResult<PathBuf> {
    let map = args
        .as_object_mut()
        .ok_or_else(|| CliError::usage("the arguments are an object"))?;
    match map.remove("project") {
        Some(Value::String(p)) if !p.is_empty() => Ok(PathBuf::from(p)),
        _ => Err(CliError::usage(
            "every tool but catalog needs \"project\", the path of a .chukcut file",
        )),
    }
}

/// Render a frame to a temporary PNG and read it back.
fn frame_png(session: &Session, at: i64) -> CliResult<Vec<u8>> {
    let dir = chukcut_engine::modules::workspace::paths::cache_root().join("cli-frames");
    let file = dir.join(format!("{}-{at}.png", std::process::id()));
    let written = render_png(session, at, &file)?;
    let bytes = std::fs::read(&written)
        .map_err(|e| CliError::render(format!("cannot read the rendered frame: {e}")))?;
    let _ = std::fs::remove_file(&written);
    Ok(bytes)
}

fn project_property() -> Value {
    json!({
        "type": "string",
        "description": "Absolute path of the project file (.chukcut).",
    })
}

/// Add the `project` argument every project tool takes.
fn with_project(mut schema: Value) -> Value {
    if let Some(map) = schema.as_object_mut() {
        let props = map
            .entry("properties")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("properties is an object");
        let mut ordered = Map::new();
        ordered.insert("project".into(), project_property());
        ordered.extend(std::mem::take(props));
        *props = ordered;
        let required = map.entry("required").or_insert_with(|| json!([]));
        if let Some(list) = required.as_array_mut() {
            list.insert(0, json!("project"));
        }
    }
    schema
}

fn tool_json(name: &str, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
        "annotations": {
            "readOnlyHint": READ_ONLY.contains(&name),
            "destructiveHint": false,
            "openWorldHint": OPEN_WORLD.contains(&name),
        },
    })
}

fn tools() -> Vec<Value> {
    let mut out = Vec::new();
    let new = ops::tool_spec::<NewArgs>("new_project");
    out.push(tool_json(
        new.name,
        &new.description,
        with_project(new.schema),
    ));
    for spec in ops::tools() {
        out.push(tool_json(
            spec.name,
            &spec.description,
            with_project(spec.schema),
        ));
    }
    out.push(tool_json(
        "view_frame",
        "Render the picture at a time and return it as a PNG image, without writing a file.",
        with_project(json!({
            "type": "object",
            "properties": {"at": {"type": ["number", "string"], "description": "The timeline time; seconds or \"2.5s\". Default 0."}},
        })),
    ));
    out.push(tool_json(
        "batch",
        "Run a list of operations against the project in one go, all or nothing, saved once at the end. Each operation is an object with \"op\" (a tool name without the project) and that tool's arguments. A first op \"new\" creates the project.",
        with_project(json!({
            "type": "object",
            "properties": {"ops": {"type": "array", "items": {"type": "object"}}},
            "required": ["ops"],
        })),
    ));
    let apply = ops::tool_spec::<TemplateApplyArgs>("template_apply");
    out.push(tool_json(
        apply.name,
        &format!(
            "{} \"project\" is the new project file to write, or with \"into\" the existing project.",
            apply.description
        ),
        with_project(apply.schema),
    ));
    for spec in [
        ops::tool_spec::<TemplateListArgs>("template_list"),
        ops::tool_spec::<TemplateDeleteArgs>("template_delete"),
    ] {
        out.push(tool_json(spec.name, &spec.description, spec.schema));
    }
    let catalog = ops::tool_spec::<CatalogArgs>("catalog");
    out.push(tool_json(
        catalog.name,
        &catalog.description,
        catalog.schema,
    ));
    let ml = ops::tool_spec::<MlArgs>("ml");
    out.push(tool_json(ml.name, &ml.description, ml.schema));
    out
}

fn tool_names() -> Vec<&'static str> {
    let mut names = ops::names();
    names.extend([
        "new_project",
        "view_frame",
        "batch",
        "catalog",
        "ml",
        "template_apply",
        "template_list",
        "template_delete",
    ]);
    names
}

fn templates() -> Value {
    json!({"resourceTemplates": [
        {
            "uriTemplate": "chukcut://project{?path}",
            "name": "Project summary",
            "description": "Lanes, clips, media, issues and undo state of the project at path.",
            "mimeType": "application/json",
        },
        {
            "uriTemplate": "chukcut://frame{?path,at}",
            "name": "Frame",
            "description": "The rendered picture of the project at path, at time `at` (seconds or \"2.5s\").",
            "mimeType": "image/png",
        },
    ]})
}

/// Percent-encode a query value.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                Err(_) => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `chukcut://<kind>?a=b&c=d` into the kind and the query.
fn parse_uri(uri: &str) -> Option<(String, HashMap<String, String>)> {
    let rest = uri.strip_prefix("chukcut://")?;
    let (kind, query) = rest.split_once('?').unwrap_or((rest, ""));
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect();
    Some((kind.trim_end_matches('/').to_string(), query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_paths_with_spaces() {
        let path = "/home/a b/my project%.chukcut";
        let uri = format!("chukcut://frame?path={}&at=2.5s", encode(path));
        let (kind, query) = parse_uri(&uri).unwrap();
        assert_eq!(kind, "frame");
        assert_eq!(query["path"], path);
        assert_eq!(query["at"], "2.5s");
    }

    #[test]
    fn every_tool_has_an_object_schema_and_a_description() {
        let tools = tools();
        assert!(tools.len() > 30);
        for tool in &tools {
            let name = tool["name"].as_str().unwrap();
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert!(
                !tool["description"].as_str().unwrap_or("").is_empty(),
                "{name} has no description"
            );
            if !matches!(name, "catalog" | "ml" | "template_list" | "template_delete") {
                assert_eq!(tool["inputSchema"]["required"][0], "project", "{name}");
            }
        }
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for added in [
            "marker_add",
            "marker_set",
            "marker_remove",
            "marker_list",
            "crop",
            "curve",
            "freeze",
            "speed_curve",
            "layout_pip",
            "layout_split",
            "title_style",
            "title_template",
            "title_position",
            "title_duplicate",
            "template_apply",
            "template_save",
            "template_replace",
            "template_slots",
            "template_list",
            "template_delete",
            "scenes_detect",
            "scenes_split",
            "scenes_clear",
            "stabilise",
            "stabilise_set",
            "stabilise_remove",
            "beats_detect",
            "beats_clear",
            "beats_cut",
            "beats_snap",
            "reframe",
            "analysis",
            "translate_captions",
            "tts",
            "stock_kinds",
            "stock_search",
            "stock_download",
            "presets",
            "preset_save",
            "preset_remove",
            "estimate",
            "export_queue",
        ] {
            assert!(names.contains(&added), "{added} is a tool");
        }
        for name in READ_ONLY.iter().chain(OPEN_WORLD) {
            assert!(names.contains(name), "{name} in a hint list is a tool");
        }
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "tool names are unique");
    }
}
