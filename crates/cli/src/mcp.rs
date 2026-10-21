// The design of this server — its core tools (`command_list`,
// `command_run`, `command_batch`, `doc_inspect`, `render_preview`,
// `project_inspect`, `sequence_inspect`, `media_import`, `render_frame`),
// the live-app `ui_*` tools of bridge mode, titles and all four annotation
// hints on every tool, strict arguments as JSON-RPC -32602, parse errors as
// -32700 that keep the server running, the `document` and `commands`
// resources, the 2026-07-28 cache hints, and progress plus cancellation of
// long commands — is taken from FilmCraft's MCP server,
// `crates/automation/src/server.rs` and `long_job.rs`, and
// `docs/agents.md` § MCP server:
//
//   Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
//   Licensed under the Apache License, Version 2.0 (or the MIT License, at
//   your option). https://github.com/storytold/filmcraft
//
// Changes for chukcut: written against the specification without rmcp or
// tokio (the CLI has no async runtime), chukcut's operations as the
// commands, project files as the headless backend, a Unix socket to the
// app as the bridge. NOTICE.md lists what was taken.

//! `chukcut-cli mcp`: chukcut as an MCP server over stdio.
//!
//! Two backends, as in FilmCraft:
//!
//! - **Headless** (the default): projects on disk. Every tool takes an
//!   optional `project` (an absolute `.chukcut` path); the last one named,
//!   or `--project`, is used when it is left out. Every successful edit is
//!   saved at once, a failed one is rolled back, and `undo`/`redo` walk the
//!   whole connection's history.
//! - **Bridge** (`--bridge [SOCKET]`): the running app, through its control
//!   socket (`chukcut --control`, [`crate::control`]). Commands edit the
//!   document on screen and land in the app's undo history; the `ui_*`
//!   tools inspect and work the window.
//!
//! The tools are few and the commands many: `command_list` is the catalog
//! of every operation, `command_run` runs one. `--op-tools` also lists each
//! operation as a tool of its own (the server's first design); those names
//! are accepted in `tools/call` either way.
//!
//! MCP's stdio transport is newline-delimited JSON-RPC 2.0 and the part a
//! tool server needs is a few hundred lines — less than the `rmcp` SDK with
//! the async runtime it brings into a CLI that has none. Requests are
//! handled one at a time; a reader thread answers `ping` and takes
//! `notifications/cancelled` while a long command runs.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::bridge::Bridge;
use crate::commands;
use crate::error::{CliError, CliResult};
use crate::inspect;
use crate::ops::project::NewArgs;
use crate::ops::template::TemplateApplyArgs;
use crate::ops::{Ctx, Outcome};
use crate::session::{absolute, Session};
use crate::values::Time;

/// Newest first. The first is what a client that asks for something else
/// is offered.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Revisions a client names per request in `_meta` instead of through
/// `initialize`. Their list and read results carry `resultType`, `ttlMs`
/// and `cacheScope` (current Claude Code requires them).
const PER_REQUEST_VERSIONS: &[&str] = &["2026-07-28"];

/// The longest side of a rendered frame unless the call says otherwise.
const DEFAULT_MAX_SIDE: u32 = 960;

const INSTRUCTIONS: &str = "chukcut is a video editor (CapCut-style, FFmpeg underneath). Every \
edit is a command: `command_list` finds ids and their parameters, `command_run` runs one \
({\"id\": \"split\", \"params\": {\"at\": 2.5}}), `command_batch` runs several. \
`doc_inspect` (or `project_inspect` / `sequence_inspect`) returns the clips with a `ref` \
(lane:index) and an `id` that commands take; `render_frame` shows the picture at a time. \
Undo with `command_run` {\"id\": \"undo\"}. Times are seconds as numbers, or strings like \
\"2.5s\", \"250ms\", \"1:02.5\", \"45f\"; the engine counts microseconds. Headless, tools \
work on a project file named by `project` (an absolute path; `new_project` makes one) and \
remember the last one; edits are saved at once. In bridge mode they work on the project open \
in the running app, and the `ui_*` tools inspect and drive its window.";

/// How the server was started.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Headless: the project used when a call names none.
    pub project: Option<PathBuf>,
    /// Bridge mode, with the app's socket or `None` to find it.
    pub bridge: Option<Option<PathBuf>>,
    /// Also list every operation as a tool of its own.
    pub op_tools: bool,
}

type Out = Arc<Mutex<Box<dyn Write + Send>>>;

fn send(out: &Out, message: &Value) {
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    let _ = writeln!(out, "{message}");
    let _ = out.flush();
}

/// The request being handled now, so the reader thread can cancel it.
type InFlight = Arc<Mutex<Option<(Value, Arc<AtomicBool>)>>>;

pub fn serve(options: Options) -> std::io::Result<()> {
    serve_io(options, std::io::stdin(), std::io::stdout())
}

/// Serve MCP over any pair of streams; returns at the end of `input`.
pub fn serve_io(
    options: Options,
    input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
) -> std::io::Result<()> {
    let out: Out = Arc::new(Mutex::new(Box::new(output)));
    let in_flight: InFlight = Arc::new(Mutex::new(None));
    let (tx, rx) = mpsc::channel::<Value>();
    let reader_out = Arc::clone(&out);
    let reader_flight = Arc::clone(&in_flight);
    let reader = std::thread::Builder::new()
        .name("chukcut-mcp-read".into())
        .spawn(move || read_loop(input, &reader_out, &reader_flight, &tx))?;
    let mut server = Server::new(options, Arc::clone(&out), in_flight);
    for message in rx {
        if let Value::Array(items) = message {
            // Batches were in the 2025-03-26 revision; answer them as one.
            let replies: Vec<Value> = items
                .into_iter()
                .filter_map(|m| server.handle_contained(m))
                .collect();
            if !replies.is_empty() {
                send(&out, &Value::Array(replies));
            }
            continue;
        }
        if let Some(reply) = server.handle_contained(message) {
            send(&out, &reply);
        }
    }
    reader
        .join()
        .unwrap_or_else(|_| Err(std::io::Error::other("the reader thread panicked")))
}

/// Read requests. A line that is not JSON gets a parse error (id null) and
/// the server goes on. `ping` is answered here, so it is answered while a
/// long command runs; `notifications/cancelled` stops the running request.
fn read_loop(
    input: impl Read,
    out: &Out,
    in_flight: &InFlight,
    tx: &mpsc::Sender<Value>,
) -> std::io::Result<()> {
    for line in BufReader::new(input).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(
                    out,
                    &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}}),
                );
                continue;
            }
        };
        match message.get("method").and_then(Value::as_str) {
            Some("ping") if message.get("id").is_some() => {
                let mut result = json!({});
                if per_request_version(message.get("params")).is_some() {
                    result["resultType"] = json!("complete");
                }
                send(
                    out,
                    &json!({"jsonrpc": "2.0", "id": message["id"], "result": result}),
                );
            }
            Some("notifications/cancelled") => {
                let asked = message.get("params").and_then(|p| p.get("requestId"));
                let flight = in_flight.lock().unwrap_or_else(|e| e.into_inner());
                if let Some((id, cancel)) = flight.as_ref() {
                    if Some(id) == asked {
                        cancel.store(true, Ordering::Release);
                    }
                }
            }
            _ => {
                if tx.send(message).is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// The revision a request names in its `_meta`, when it is one this server
/// speaks per request.
fn per_request_version(params: Option<&Value>) -> Option<&'static str> {
    let asked = params?
        .get("_meta")?
        .get("io.modelcontextprotocol/protocolVersion")?
        .as_str()?;
    PER_REQUEST_VERSIONS.iter().copied().find(|v| *v == asked)
}

enum Backend {
    Files {
        sessions: HashMap<PathBuf, Session>,
        /// The project a call that names none works on: `--project`, then
        /// the last one named.
        current: Option<PathBuf>,
    },
    App(Bridge),
}

struct Server {
    out: Out,
    backend: Backend,
    op_tools: bool,
    protocol: &'static str,
    in_flight: InFlight,
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

/// What a tool answers: the outcome, and a picture for the tools that show one.
struct ToolOutput {
    outcome: Outcome,
    image: Option<Vec<u8>>,
    /// Answered as an error result although the call itself worked: a batch
    /// with a failed step.
    is_error: bool,
}

impl ToolOutput {
    fn text(outcome: Outcome) -> Self {
        Self {
            outcome,
            image: None,
            is_error: false,
        }
    }
}

fn read_outcome(message: impl Into<String>, data: Value) -> Outcome {
    Outcome::read(message, data)
}

impl Server {
    fn new(options: Options, out: Out, in_flight: InFlight) -> Self {
        let backend = match options.bridge {
            Some(socket) => Backend::App(Bridge::new(socket)),
            None => Backend::Files {
                sessions: HashMap::new(),
                current: options.project.map(|p| absolute(&p)),
            },
        };
        Self {
            out,
            backend,
            op_tools: options.op_tools,
            protocol: PROTOCOL_VERSIONS[0],
            in_flight,
        }
    }

    fn bridge(&self) -> bool {
        matches!(self.backend, Backend::App(_))
    }

    /// [`Self::handle`] with a panic contained: one request that hits a bug
    /// answers an internal error and the session goes on.
    ///
    /// The open projects are dropped, because the request may have stopped
    /// half way through an edit; every tool call saves, so the next call
    /// reopens them from disk as they were after the last good one.
    fn handle_contained(&mut self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        match chukcut_engine::lifecycle::contain("That request", || self.handle(message)) {
            Ok(reply) => reply,
            Err(error) => {
                if let Backend::Files { sessions, .. } = &mut self.backend {
                    sessions.clear();
                }
                id.map(|id| {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": error}})
                })
            }
        }
    }

    /// Handle one message; `None` for a notification, a response, or a
    /// request the client cancelled.
    fn handle(&mut self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response to something we never ask, or garbage.
            return id.map(|id| {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32600, "message": "invalid request"}})
            });
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let modern = per_request_version(Some(&params));
        chukcut_engine::faults::hit_keyed("mcp.request", method);
        let mut cancel = None;
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "server/discover" => Ok(self.discover()),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": self.tools()})),
            "tools/call" => {
                let flag = Arc::new(AtomicBool::new(false));
                if let Some(id) = &id {
                    *self.in_flight.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some((id.clone(), Arc::clone(&flag)));
                }
                cancel = Some(Arc::clone(&flag));
                let result = self.call(&params, flag);
                *self.in_flight.lock().unwrap_or_else(|e| e.into_inner()) = None;
                result
            }
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
        // A cancelled request gets no response, as the MCP cancellation
        // utility asks.
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            return None;
        }
        Some(match result {
            Ok(mut result) => {
                if modern.is_some() {
                    if let Some(map) = result.as_object_mut() {
                        map.insert("resultType".into(), json!("complete"));
                        let ttl = match method {
                            "tools/list" | "resources/list" | "resources/templates/list" => {
                                Some(600_000)
                            }
                            // The document changes with every edit.
                            "resources/read" => Some(0),
                            _ => None,
                        };
                        if let Some(ttl) = ttl {
                            map.insert("ttlMs".into(), json!(ttl));
                            map.insert("cacheScope".into(), json!("private"));
                        }
                    }
                }
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            }
            Err(e) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.code, "message": e.message}})
            }
        })
    }

    fn capabilities(&self) -> Value {
        json!({
            "tools": {"listChanged": false},
            "resources": {"subscribe": false, "listChanged": false},
        })
    }

    fn server_info(&self) -> Value {
        json!({
            "name": "chukcut",
            "title": "chukcut video editor",
            "version": env!("CARGO_PKG_VERSION"),
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
            "capabilities": self.capabilities(),
            "serverInfo": self.server_info(),
            "instructions": INSTRUCTIONS,
        })
    }

    fn discover(&self) -> Value {
        let versions: Vec<&str> = PER_REQUEST_VERSIONS
            .iter()
            .chain(PROTOCOL_VERSIONS)
            .copied()
            .collect();
        json!({
            "supportedVersions": versions,
            "capabilities": self.capabilities(),
            "serverInfo": self.server_info(),
            "instructions": INSTRUCTIONS,
        })
    }

    // --- tools -----------------------------------------------------------------

    fn tools(&self) -> Vec<Value> {
        let mut out: Vec<Value> = core_tools()
            .into_iter()
            .filter(|t| self.bridge() || !t.bridge_only)
            .map(|t| t.json(self.bridge()))
            .collect();
        if self.op_tools {
            let taken: Vec<String> = out
                .iter()
                .filter_map(|t| t["name"].as_str().map(str::to_string))
                .collect();
            for spec in commands::specs() {
                if taken.iter().any(|n| n == spec.name) {
                    continue;
                }
                let ro = commands::READ_ONLY.contains(&spec.name);
                let schema = if self.bridge() || commands::NO_PROJECT.contains(&spec.name) {
                    spec.schema.clone()
                } else {
                    with_project(spec.schema.clone(), true)
                };
                out.push(tool_json(
                    spec.name,
                    &title_of(&spec.description, spec.name),
                    &spec.description,
                    schema,
                    Hints {
                        read_only: ro,
                        destructive: !ro,
                        idempotent: ro,
                        open_world: commands::OPEN_WORLD.contains(&spec.name),
                    },
                ));
            }
        }
        out
    }

    /// The first argument `tool` does not take, as a message naming the ones
    /// it does. `command_run.params` and a batch step's `params` go to their
    /// command unchanged and are checked there.
    fn unknown_arg(&self, tool: &str, args: &Value) -> Option<String> {
        let map = args.as_object()?;
        if let Some(core) = core_tools().into_iter().find(|t| t.name == tool) {
            let props = core.schema(self.bridge());
            let props = props.get("properties")?.as_object()?;
            let bad = map.keys().find(|k| !props.contains_key(*k))?;
            let mut accepted: Vec<&str> = props.keys().map(String::as_str).collect();
            accepted.sort_unstable();
            let accepted = if accepted.is_empty() {
                "no arguments".to_string()
            } else {
                accepted.join(", ")
            };
            return Some(format!(
                "unknown argument \"{bad}\" for {tool}; expected: {accepted}"
            ));
        }
        match tool {
            "view_frame" | "batch" => {
                let allowed: &[&str] = if tool == "batch" {
                    &["project", "ops"]
                } else {
                    &["project", "at"]
                };
                let bad = map.keys().find(|k| !allowed.contains(&k.as_str()))?;
                Some(format!(
                    "unknown argument \"{bad}\" for {tool}; expected: {}",
                    allowed.join(", ")
                ))
            }
            _ => commands::unknown_argument(tool, args),
        }
    }

    fn call(&mut self, params: &Value, cancel: Arc<AtomicBool>) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("tools/call needs a name"))?
            .to_string();
        let known = core_tools().iter().any(|t| t.name == name)
            || commands::is_command(&name)
            || matches!(name.as_str(), "view_frame" | "batch");
        if !known {
            return Err(RpcError::invalid_params(format!("unknown tool: {name}")));
        }
        let mut args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if args.is_null() {
            args = json!({});
        }
        if !args.is_object() {
            return Err(RpcError::invalid_params(format!(
                "the arguments of {name} are an object"
            )));
        }
        if let Some(message) = self.unknown_arg(&name, &args) {
            return Err(RpcError::invalid_params(message));
        }
        let token = params
            .get("_meta")
            .and_then(|m| m.get("progressToken"))
            .cloned();
        let ctx = self.progress_ctx(token, cancel);
        let output = self.run_tool(&name, args, &ctx);
        Ok(self.tool_result(&name, output))
    }

    fn progress_ctx(&self, token: Option<Value>, cancel: Arc<AtomicBool>) -> Ctx {
        let mut ctx = match token {
            None => Ctx::quiet(),
            Some(token) => {
                let out = Arc::clone(&self.out);
                // Progress only ever moves forward within one request.
                let last = Mutex::new(-1.0f64);
                Ctx::with_progress(Box::new(move |label: &str, fraction: Option<f32>| {
                    let progress = fraction.map(f64::from).unwrap_or(0.0);
                    {
                        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                        if fraction.is_some() && progress < *last {
                            return;
                        }
                        *last = progress;
                    }
                    let mut params = json!({
                        "progressToken": token,
                        "progress": progress,
                        "message": label,
                    });
                    if fraction.is_some() {
                        params["total"] = json!(1.0);
                    }
                    send(
                        &out,
                        &json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": params}),
                    );
                }))
            }
        };
        ctx.cancel = cancel;
        ctx
    }

    fn run_tool(&mut self, name: &str, mut args: Value, ctx: &Ctx) -> CliResult<ToolOutput> {
        let project = take_string(&mut args, "project");
        match name {
            "command_list" => {
                let filter = take_string(&mut args, "filter");
                let enabled_only = args.get("enabled_only").and_then(Value::as_bool) == Some(true);
                let list = match &self.backend {
                    Backend::App(bridge) => bridge
                        .call(
                            "engine.commands",
                            json!({"filter": filter, "enabled_only": enabled_only}),
                        )
                        .map_err(CliError::refused)?,
                    Backend::Files { .. } => {
                        let has = self.target(project).is_ok_and(|p| p.is_file());
                        commands::list(filter.as_deref(), enabled_only, has)
                    }
                };
                let n = list.as_array().map_or(0, Vec::len);
                Ok(ToolOutput::text(read_outcome(
                    format!("{n} command(s)"),
                    list,
                )))
            }
            "command_run" => {
                let id = take_string(&mut args, "id")
                    .ok_or_else(|| CliError::usage("command_run needs \"id\", a command id"))?;
                let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
                self.command(&id, params, project, ctx)
                    .map(ToolOutput::text)
            }
            "command_batch" => self.command_batch(args, project, ctx),
            "batch" => {
                // The first design's all-or-nothing batch, by its old name.
                let ops = args
                    .get("ops")
                    .cloned()
                    .ok_or_else(|| CliError::usage("batch needs an \"ops\" list"))?;
                self.atomic_batch(project, ops, ctx).map(ToolOutput::text)
            }
            "doc_inspect" | "project_inspect" | "sequence_inspect" => {
                let data = match &self.backend {
                    Backend::App(bridge) => {
                        let method = match name {
                            "doc_inspect" => "doc.inspect",
                            "project_inspect" => "project.inspect",
                            _ => "sequence.inspect",
                        };
                        bridge.call(method, json!({})).map_err(CliError::refused)?
                    }
                    Backend::Files { .. } => {
                        let session = self.session_for(project)?;
                        match name {
                            "doc_inspect" => inspect::document(session),
                            "project_inspect" => inspect::project(session),
                            _ => inspect::sequence(session),
                        }
                    }
                };
                Ok(ToolOutput::text(read_outcome(name.replace('_', " "), data)))
            }
            "media_import" => {
                let paths = args.get("paths").cloned().unwrap_or(Value::Null);
                if !paths.is_array() {
                    return Err(CliError::usage(
                        "media_import needs \"paths\", a list of absolute file paths",
                    ));
                }
                let append = args.get("append").and_then(Value::as_bool).unwrap_or(false);
                self.command(
                    "import",
                    json!({"files": paths, "append": append}),
                    project,
                    ctx,
                )
                .map(ToolOutput::text)
            }
            "render_frame" | "render_preview" | "view_frame" => {
                // `render_frame` with an output file is the operation of the
                // first design, which writes a PNG.
                if args.get("output").is_some() {
                    let mut params = Map::new();
                    params.insert("output".into(), args["output"].clone());
                    let at = args
                        .get("at")
                        .cloned()
                        .or_else(|| args.get("seconds").cloned())
                        .unwrap_or(json!(0));
                    params.insert("at".into(), at);
                    return self
                        .command("render_frame", Value::Object(params), project, ctx)
                        .map(ToolOutput::text);
                }
                self.render(project, &args)
            }
            "new_project" => self.new_project(project, args).map(ToolOutput::text),
            "ui_screenshot" => {
                let bridge = self.app("ui_screenshot")?;
                let max_side = args.get("max_side").cloned().unwrap_or(json!(1600));
                let shot = bridge
                    .call("ui.screenshot", json!({"max_side": max_side}))
                    .map_err(CliError::render)?;
                let png = decode_png(&shot)?;
                let mut data = shot.clone();
                if let Some(map) = data.as_object_mut() {
                    map.remove("png");
                }
                Ok(ToolOutput {
                    outcome: read_outcome("a screenshot of the chukcut window", data),
                    image: Some(png),
                    is_error: false,
                })
            }
            n if n.starts_with("ui_") => {
                let bridge = self.app(n)?;
                let (method, params) = match n {
                    "ui_control" => {
                        let method = take_string(&mut args, "method").ok_or_else(|| {
                            CliError::usage("ui_control needs \"method\", a control method")
                        })?;
                        (method, args.get("params").cloned().unwrap_or(json!({})))
                    }
                    other => (format!("ui.{}", &other[3..]), args),
                };
                let data = bridge.call(&method, params).map_err(CliError::refused)?;
                Ok(ToolOutput::text(read_outcome(method, data)))
            }
            // An operation called by its own name (`split`, `grade`): the
            // tools of `--op-tools`, accepted whether listed or not.
            op => self.command(op, args, project, ctx).map(ToolOutput::text),
        }
    }

    fn app(&self, tool: &str) -> CliResult<&Bridge> {
        match &self.backend {
            Backend::App(bridge) => Ok(bridge),
            Backend::Files { .. } => Err(CliError::usage(format!(
                "{tool} drives the running app: start it with `chukcut --control` and this \
                 server with `chukcut-cli mcp --bridge`"
            ))),
        }
    }

    /// The project a headless call works on: the one it names (which then
    /// becomes the current one), or the current one.
    fn target(&mut self, project: Option<String>) -> CliResult<PathBuf> {
        let Backend::Files { current, .. } = &mut self.backend else {
            return Err(CliError::usage("no project files in bridge mode"));
        };
        match project.filter(|p| !p.is_empty()) {
            Some(p) => {
                let path = absolute(Path::new(&p));
                *current = Some(path.clone());
                Ok(path)
            }
            None => current.clone().ok_or_else(|| {
                CliError::usage(
                    "name the project: \"project\": \"/abs/path/to/file.chukcut\" (new_project \
                     makes one), or start the server with --project",
                )
            }),
        }
    }

    /// The open session for the project a call names, opened or reloaded as
    /// needed. A file changed on disk by someone else is reloaded, which
    /// starts its history afresh.
    fn session_for(&mut self, project: Option<String>) -> CliResult<&mut Session> {
        let path = self.target(project)?;
        let Backend::Files { sessions, .. } = &mut self.backend else {
            return Err(CliError::usage("no project files in bridge mode"));
        };
        let stale = sessions
            .get(&path)
            .is_some_and(|s| s.changed_on_disk() || !s.path.exists());
        if stale {
            sessions.remove(&path);
        }
        if !sessions.contains_key(&path) {
            let session = Session::open(&path)?;
            sessions.insert(path.clone(), session);
        }
        Ok(sessions.get_mut(&path).expect("inserted"))
    }

    fn adopt(&mut self, session: Session) {
        if let Backend::Files { sessions, current } = &mut self.backend {
            *current = Some(session.path.clone());
            sessions.insert(session.path.clone(), session);
        }
    }

    /// Run command `id` with `params` on whichever backend.
    fn command(
        &mut self,
        id: &str,
        mut params: Value,
        project: Option<String>,
        ctx: &Ctx,
    ) -> CliResult<Outcome> {
        if params.is_null() {
            params = json!({});
        }
        if !commands::is_command(id) {
            return Err(CliError::usage(format!(
                "there is no command called {id:?}; command_list lists them"
            )));
        }
        // The first design put `project` inside the operation's arguments.
        let project = project.or_else(|| take_string(&mut params, "project"));
        if let Backend::App(bridge) = &self.backend {
            let progress = |label: &str, fraction: Option<f32>| ctx.progress(label, fraction);
            let reply = bridge
                .call_with(
                    "engine.execute",
                    json!({"command": id, "params": params}),
                    &progress,
                    &ctx.cancel,
                )
                .map_err(CliError::refused)?;
            return Ok(Outcome {
                message: reply["message"].as_str().unwrap_or_default().to_string(),
                data: reply.get("data").cloned().unwrap_or(Value::Null),
                mutated: reply["changed"] == true,
            });
        }
        if let Some(result) = commands::run_without_project(id, params.clone()) {
            return result;
        }
        let into = params.get("into").and_then(Value::as_bool) == Some(true);
        if id == "new_project" || (id == "template_apply" && !into) {
            let path = self.target(project)?;
            let (mut session, outcome) = if id == "new_project" {
                let args: NewArgs = serde_json::from_value(params)
                    .map_err(|e| CliError::usage(format!("new_project: {e}")))?;
                args.create(&path)?
            } else {
                let args: TemplateApplyArgs = serde_json::from_value(params)
                    .map_err(|e| CliError::usage(format!("template_apply: {e}")))?;
                args.create(&path)?
            };
            session.save()?;
            self.adopt(session);
            return Ok(outcome);
        }
        let session = self.session_for(project)?;
        let before = serde_json::to_string(&session.project()).unwrap_or_default();
        let result = commands::run_on(id, params, session, ctx).and_then(|outcome| {
            if outcome.mutated {
                session.save()?;
            }
            Ok(outcome)
        });
        if result.is_err() {
            let after = serde_json::to_string(&session.project()).unwrap_or_default();
            if after != before {
                // A refused edit must not stay half applied in memory where
                // the next successful one would save it.
                let path = session.path.clone();
                if let Backend::Files { sessions, .. } = &mut self.backend {
                    sessions.remove(&path);
                }
            }
        }
        result
    }

    fn new_project(&mut self, project: Option<String>, args: Value) -> CliResult<Outcome> {
        if self.bridge() {
            return Err(CliError::usage(
                "new_project writes a project file; in bridge mode the app's open project is \
                 the target (use the app's New project, or run this server headless)",
            ));
        }
        let project = project.ok_or_else(|| {
            CliError::usage("new_project needs \"project\", the path of the file to create")
        })?;
        self.command("new_project", args, Some(project), &Ctx::quiet())
    }

    /// FilmCraft's batch: each step its own command and undo step, results
    /// for every step. `atomic` is chukcut's all-or-nothing batch, saved once.
    fn command_batch(
        &mut self,
        args: Value,
        project: Option<String>,
        ctx: &Ctx,
    ) -> CliResult<ToolOutput> {
        let steps = args
            .get("steps")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| CliError::usage("command_batch needs \"steps\": [{id, params}]"))?;
        if args.get("atomic").and_then(Value::as_bool) == Some(true) {
            let ops: Vec<Value> = steps
                .into_iter()
                .map(|step| {
                    let mut op = step
                        .get("params")
                        .cloned()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({}));
                    let id = if step["id"] == "new_project" {
                        json!("new")
                    } else {
                        step["id"].clone()
                    };
                    op["op"] = id;
                    op
                })
                .collect();
            return self
                .atomic_batch(project, Value::Array(ops), ctx)
                .map(ToolOutput::text);
        }
        let stop = args
            .get("stop_on_error")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let (mut completed, mut failed, mut results) = (0u32, 0u32, Vec::new());
        for step in steps {
            if ctx.cancelled() {
                break;
            }
            let id = step.get("id").and_then(Value::as_str).unwrap_or_default();
            let params = step.get("params").cloned().unwrap_or_else(|| json!({}));
            match self.command(id, params, project.clone(), ctx) {
                Ok(outcome) => {
                    completed += 1;
                    results.push(json!({"ok": true, "result": commands::outcome_json(&outcome)}));
                }
                Err(error) => {
                    failed += 1;
                    results.push(json!({"ok": false, "error": error.message}));
                    if stop {
                        break;
                    }
                }
            }
        }
        Ok(ToolOutput {
            outcome: Outcome {
                message: format!("{completed} step(s) done, {failed} failed"),
                data: json!({"completed": completed, "failed": failed, "results": results}),
                mutated: completed > 0,
            },
            image: None,
            is_error: failed > 0,
        })
    }

    fn atomic_batch(
        &mut self,
        project: Option<String>,
        ops: Value,
        ctx: &Ctx,
    ) -> CliResult<Outcome> {
        if self.bridge() {
            return Err(CliError::usage(
                "an all-or-nothing batch works on a project file; in the app each step is its \
                 own undo step (command_batch without \"atomic\")",
            ));
        }
        let path = self.target(project)?;
        let ops = crate::batch_ops(&ops.to_string())?;
        // The batch saves once at the end; a session held here would not
        // see that history, so it is reopened afterwards.
        if let Backend::Files { sessions, .. } = &mut self.backend {
            sessions.remove(&path);
        }
        let (outcome, _) = crate::run_batch(&path, ops, false, ctx)?;
        Ok(outcome)
    }

    fn render(&mut self, project: Option<String>, args: &Value) -> CliResult<ToolOutput> {
        let max_side = args
            .get("max_side")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_SIDE, |v| v.min(u32::MAX as u64) as u32);
        let at = args.get("at").or_else(|| args.get("seconds")).cloned();
        let (png, at_us) = match &self.backend {
            Backend::App(bridge) => {
                let mut params = json!({"max_side": max_side});
                if let Some(at) = at {
                    let at: Time = serde_json::from_value(at)
                        .map_err(|e| CliError::usage(format!("seconds: {e}")))?;
                    // The app's frame rate resolves "45f"; seconds go as they are.
                    let fps = bridge
                        .call("project.inspect", json!({}))
                        .ok()
                        .and_then(|p| p["fps"].as_f64())
                        .unwrap_or(30.0);
                    params["seconds"] = json!(at.resolve(fps) as f64 / 1_000_000.0);
                }
                let frame = bridge
                    .call("frame.render", params)
                    .map_err(CliError::render)?;
                (decode_png(&frame)?, frame["at_us"].as_i64().unwrap_or(0))
            }
            Backend::Files { .. } => {
                let session = self.session_for(project)?;
                let at: Time = serde_json::from_value(at.unwrap_or(json!(0)))
                    .map_err(|e| CliError::usage(format!("seconds: {e}")))?;
                let at = at.resolve(session.fps());
                let png = pollster::block_on(
                    chukcut_engine::modules::export::commands::export_preview_png(
                        &session.state,
                        at,
                        max_side,
                    ),
                )
                .map_err(CliError::render)?;
                (png, at)
            }
        };
        let seconds = crate::values::seconds(at_us);
        Ok(ToolOutput {
            outcome: read_outcome(
                format!("the frame at {seconds:.3} s"),
                json!({"at": seconds}),
            ),
            image: Some(png),
            is_error: false,
        })
    }

    fn tool_result(&self, name: &str, output: CliResult<ToolOutput>) -> Value {
        match output {
            Ok(output) => {
                let body = json!({
                    "message": output.outcome.message,
                    "data": output.outcome.data,
                });
                let mut content = vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&body).unwrap_or_default(),
                })];
                if let Some(png) = output.image {
                    content.push(json!({
                        "type": "image",
                        "data": base64::engine::general_purpose::STANDARD.encode(png),
                        "mimeType": "image/png",
                    }));
                }
                let mut result = json!({"content": content, "isError": output.is_error});
                if self.protocol >= "2025-06-18" {
                    result["structuredContent"] = body;
                }
                result
            }
            Err(error) => json!({
                "content": [{"type": "text", "text": format!("{name} failed ({:?}): {}", error.kind, error.message)}],
                "isError": true,
            }),
        }
    }

    // --- resources -----------------------------------------------------------

    fn resources(&self) -> Value {
        let mut list = vec![
            json!({
                "uri": DOCUMENT_URI,
                "name": "document",
                "title": "Project",
                "description": "The project and its open timeline (as doc_inspect).",
                "mimeType": "application/json",
            }),
            json!({
                "uri": COMMANDS_URI,
                "name": "commands",
                "title": "Command catalog",
                "description": "Every command with id, label, group, parameters and hints (as command_list).",
                "mimeType": "application/json",
            }),
        ];
        if let Backend::Files { sessions, .. } = &self.backend {
            for path in sessions.keys() {
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
        let not_found = |e: CliError| RpcError {
            code: -32002,
            message: e.message,
        };
        let json_contents = |value: &Value| {
            json!({"contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(value).unwrap_or_default(),
            }]})
        };
        let path = query.get("path").cloned();
        match kind.as_str() {
            "commands" => {
                let ctx = Ctx::quiet();
                let output = self
                    .run_tool("command_list", json!({}), &ctx)
                    .map_err(not_found)?;
                Ok(json_contents(&output.outcome.data))
            }
            "document" | "project" => {
                let ctx = Ctx::quiet();
                let mut args = json!({});
                if let Some(p) = &path {
                    args["project"] = json!(p);
                }
                let output = if kind == "document" {
                    self.run_tool("doc_inspect", args, &ctx)
                } else if self.bridge() {
                    self.run_tool("project_inspect", args, &ctx)
                } else {
                    // The `info` summary, as the first design served it.
                    self.command("info", json!({}), path.clone(), &ctx)
                        .map(ToolOutput::text)
                }
                .map_err(not_found)?;
                Ok(json_contents(&output.outcome.data))
            }
            "frame" => {
                let mut args = json!({});
                if let Some(p) = &path {
                    args["project"] = json!(p);
                }
                if let Some(at) = query.get("at") {
                    // Checked here so a bad time is a protocol error.
                    at.parse::<Time>().map_err(RpcError::invalid_params)?;
                    args["at"] = json!(at);
                }
                let ctx = Ctx::quiet();
                let output = self
                    .run_tool("render_frame", args, &ctx)
                    .map_err(not_found)?;
                let png = output.image.unwrap_or_default();
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

const DOCUMENT_URI: &str = "chukcut://document";
const COMMANDS_URI: &str = "chukcut://commands";

fn decode_png(reply: &Value) -> CliResult<Vec<u8>> {
    let data = reply
        .get("png")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::render("the app answered no picture"))?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| CliError::render(format!("the app's picture is not base64: {e}")))
}

fn take_string(args: &mut Value, key: &str) -> Option<String> {
    let map = args.as_object_mut()?;
    match map.remove(key) {
        Some(Value::String(s)) => Some(s),
        Some(other) => {
            map.insert(key.into(), other);
            None
        }
        None => None,
    }
}

// --- tool definitions -----------------------------------------------------------

/// MCP's four tool annotation hints.
#[derive(Clone, Copy)]
struct Hints {
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    open_world: bool,
}

const READ: Hints = Hints {
    read_only: true,
    destructive: false,
    idempotent: true,
    open_world: false,
};
const EDIT: Hints = Hints {
    read_only: false,
    destructive: true,
    idempotent: false,
    open_world: false,
};

/// One of the server's own tools.
struct CoreTool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// The properties, without `project` (added in headless mode).
    properties: Value,
    required: &'static [&'static str],
    hints: Hints,
    /// Listed only in bridge mode: it works the app's window.
    bridge_only: bool,
}

impl CoreTool {
    fn schema(&self, bridge: bool) -> Value {
        let mut schema = json!({
            "type": "object",
            "properties": self.properties,
        });
        if !self.required.is_empty() {
            schema["required"] = json!(self.required);
        }
        if !bridge && !self.bridge_only {
            schema = with_project(schema, self.name == "new_project");
        }
        schema
    }

    fn json(&self, bridge: bool) -> Value {
        tool_json(
            self.name,
            self.title,
            self.description,
            self.schema(bridge),
            self.hints,
        )
    }
}

fn core_tools() -> Vec<CoreTool> {
    let seconds = json!({"type": ["number", "string"], "description": "The timeline time: seconds, or \"2.5s\", \"250ms\", \"1:02.5\", \"45f\". Default: 0 headless, the playhead in the app."});
    let max_side = json!({"type": "integer", "minimum": 0, "description": "The longest side of the PNG in pixels (default 960; 0 = the full canvas)."});
    let point = json!({"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}}, "required": ["x", "y"]});
    let modifiers = json!({"type": "object", "description": "{\"shift\": bool, \"ctrl\": bool, \"alt\": bool, \"super\": bool}"});
    vec![
        CoreTool {
            name: "command_list",
            title: "List commands",
            description: "List every command: id, label, group, parameter schema, readOnly/openWorld, and whether it can run now. Every capability of chukcut is one of these ids.",
            properties: json!({
                "filter": {"type": "string", "description": "Only commands whose id or label contains this text (any case)."},
                "enabled_only": {"type": "boolean", "description": "Only commands that can run right now."},
            }),
            required: &[],
            hints: READ,
            bridge_only: false,
        },
        CoreTool {
            name: "command_run",
            title: "Run a command",
            description: "Run a command by id with its parameters, e.g. {\"id\": \"split\", \"params\": {\"at\": 2.5}} or {\"id\": \"grade\", \"params\": {\"clip\": \"0:1\", \"set\": {\"exposure\": 0.3}}}. Edits are undoable ({\"id\": \"undo\"}).",
            properties: json!({
                "id": {"type": "string", "description": "A command id from command_list."},
                "params": {"type": "object", "description": "The command's parameters (its `params` schema in command_list)."},
            }),
            required: &["id"],
            hints: EDIT,
            bridge_only: false,
        },
        CoreTool {
            name: "command_batch",
            title: "Run several commands",
            description: "Run commands in order: {steps: [{id, params?}], stop_on_error?: true}. Returns {completed, failed, results: [{ok, result | error}]}; each edit is its own undo step. With atomic: true (headless only) the steps run all or nothing and save once; a first step new_project creates the file.",
            properties: json!({
                "steps": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "params": {"type": "object"}}, "required": ["id"]}},
                "stop_on_error": {"type": "boolean", "description": "Stop at the first failing step (default true)."},
                "atomic": {"type": "boolean", "description": "All or nothing, saved once at the end (project files only)."},
            }),
            required: &["steps"],
            hints: EDIT,
            bridge_only: false,
        },
        CoreTool {
            name: "doc_inspect",
            title: "Inspect the document",
            description: "The project (as project_inspect) and its open timeline (as sequence_inspect) in one answer.",
            properties: json!({}),
            required: &[],
            hints: READ,
            bridge_only: false,
        },
        CoreTool {
            name: "render_preview",
            title: "Render a preview",
            description: "The picture at `seconds` as a PNG image; the same as render_frame.",
            properties: json!({"seconds": seconds, "max_side": max_side}),
            required: &[],
            hints: READ,
            bridge_only: false,
        },
        CoreTool {
            name: "project_inspect",
            title: "Inspect the project",
            description: "The project: name, path, canvas, frame rate, imported media with ids, timelines and compound clips, validation issues, undo state.",
            properties: json!({}),
            required: &[],
            hints: READ,
            bridge_only: false,
        },
        CoreTool {
            name: "sequence_inspect",
            title: "Inspect the open timeline",
            description: "The open timeline: lanes, clips (ref lane:index, id, kind, start/end in seconds and microseconds, effects, grade), markers, duration, undo state; in the app also the playhead and the selection.",
            properties: json!({}),
            required: &[],
            hints: READ,
            bridge_only: false,
        },
        CoreTool {
            name: "media_import",
            title: "Import media",
            description: "Import media files by absolute path (video, audio, images); with append, also put each at the end of its lane.",
            properties: json!({
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Absolute paths of the files."},
                "append": {"type": "boolean", "description": "Also append each file to the timeline."},
            }),
            required: &["paths"],
            hints: Hints { destructive: false, ..EDIT },
            bridge_only: false,
        },
        CoreTool {
            name: "render_frame",
            title: "Render a frame",
            description: "Render the timeline's picture at `seconds` (default: 0 headless, the playhead in the app) and return it as a PNG image. Read-only: nothing moves. With `output`, write the full-size PNG to that file instead.",
            properties: json!({"seconds": seconds, "max_side": max_side, "output": {"type": "string", "description": "Write the full-size PNG here instead of returning it."}}),
            required: &[],
            hints: READ,
            bridge_only: false,
        },
        CoreTool {
            name: "new_project",
            title: "Create a project",
            description: "Create a project file at `project` (headless only) and make it the current project. Options: name, width, height, fps, force (overwrite).",
            properties: crate::ops::tool_spec::<NewArgs>("new_project").schema["properties"].clone(),
            required: &[],
            hints: Hints { destructive: false, ..EDIT },
            bridge_only: false,
        },
        CoreTool {
            name: "ui_inspect",
            title: "Live app: UI state",
            description: "Live app: the playhead, playing or not, the selected clip, the project file, unsaved changes, the window's size and the status line.",
            properties: json!({}),
            required: &[],
            hints: READ,
            bridge_only: true,
        },
        CoreTool {
            name: "ui_screenshot",
            title: "Live app: screenshot",
            description: "Live app: a screenshot of the chukcut window as PNG (X11 with ImageMagick). For the picture alone, render_frame works everywhere.",
            properties: json!({"max_side": {"type": "integer", "minimum": 0, "description": "The longest side of the PNG (default 1600; 0 = as captured)."}}),
            required: &[],
            hints: READ,
            bridge_only: true,
        },
        CoreTool {
            name: "ui_key",
            title: "Live app: press a key",
            description: "Live app: press a key or shortcut as the user would: `space`, `s`, `ctrl-z`, `shift-delete`, `Ctrl+K` (both spellings).",
            properties: json!({"key": {"type": "string"}}),
            required: &["key"],
            hints: EDIT,
            bridge_only: true,
        },
        CoreTool {
            name: "ui_type",
            title: "Live app: type text",
            description: "Live app: type text into the focused field.",
            properties: json!({"text": {"type": "string"}}),
            required: &["text"],
            hints: EDIT,
            bridge_only: true,
        },
        CoreTool {
            name: "ui_click",
            title: "Live app: click",
            description: "Live app: click at x, y in window points (ui_inspect gives the size). button left|right|middle, count 2 for a double click, modifiers.",
            properties: json!({
                "x": {"type": "number"},
                "y": {"type": "number"},
                "button": {"type": "string", "enum": ["left", "right", "middle"]},
                "count": {"type": "integer", "minimum": 1},
                "modifiers": modifiers,
            }),
            required: &["x", "y"],
            hints: EDIT,
            bridge_only: true,
        },
        CoreTool {
            name: "ui_drag",
            title: "Live app: drag",
            description: "Live app: press, move and release from one point to another in window points (move a clip, trim an edge, scrub).",
            properties: json!({"from": point, "to": point, "steps": {"type": "integer", "minimum": 1}, "modifiers": modifiers}),
            required: &["from", "to"],
            hints: EDIT,
            bridge_only: true,
        },
        CoreTool {
            name: "ui_control",
            title: "Live app: control call",
            description: "Live app: call any control-socket method directly (ui.seek, ui.select, ui.playback, app.info, engine.commands, …; docs/cli.md lists them).",
            properties: json!({"method": {"type": "string"}, "params": {"type": "object"}}),
            required: &["method"],
            hints: EDIT,
            bridge_only: true,
        },
    ]
}

/// A tool's title from its description: the first sentence, or the name.
fn title_of(description: &str, name: &str) -> String {
    let first = description
        .split(['.', '\n'])
        .next()
        .unwrap_or_default()
        .trim();
    if first.is_empty() || first.len() > 60 {
        name.replace('_', " ")
    } else {
        first.to_string()
    }
}

fn project_property() -> Value {
    json!({
        "type": "string",
        "description": "Absolute path of the project file (.chukcut). Default: the last project named, or --project.",
    })
}

/// Add the `project` argument; `required` for the tools that create it.
fn with_project(mut schema: Value, required: bool) -> Value {
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
        if required {
            let list = map.entry("required").or_insert_with(|| json!([]));
            if let Some(list) = list.as_array_mut() {
                list.insert(0, json!("project"));
            }
        }
    }
    schema
}

fn tool_json(name: &str, title: &str, description: &str, schema: Value, hints: Hints) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": schema,
        "annotations": {
            "title": title,
            "readOnlyHint": hints.read_only,
            "destructiveHint": hints.destructive,
            "idempotentHint": hints.idempotent,
            "openWorldHint": hints.open_world,
        },
    })
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
            // The two digits are read from the bytes, not sliced out of `s`:
            // a `%` before a multi-byte character ("%aé") would put the end
            // of a `&str` slice inside that character, which panics and
            // ended the whole MCP session.
            b'%' if i + 2 < bytes.len() => match std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                Some(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                None => out.push(b'%'),
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

    fn server(bridge: bool) -> Server {
        let out: Out = Arc::new(Mutex::new(Box::new(std::io::sink())));
        let options = Options {
            bridge: bridge.then_some(None),
            ..Options::default()
        };
        Server::new(options, out, Arc::new(Mutex::new(None)))
    }

    /// One request that panics answers an internal error; the next one is
    /// served as if nothing happened.
    #[test]
    fn a_panicking_request_answers_an_error_and_the_session_goes_on() {
        let mut server = server(false);
        chukcut_engine::faults::arm("mcp.request:resources/templates/list");
        let reply = server
            .handle_contained(
                json!({"jsonrpc": "2.0", "id": 7, "method": "resources/templates/list"}),
            )
            .expect("a reply");
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["error"]["code"], -32603);
        let message = reply["error"]["message"].as_str().unwrap();
        assert!(message.contains("internal error"), "{message}");
        let reply = server
            .handle_contained(
                json!({"jsonrpc": "2.0", "id": 8, "method": "resources/templates/list"}),
            )
            .expect("a reply");
        assert!(reply.get("result").is_some(), "{reply}");
        // A notification that panics answers nothing, and nothing breaks.
        chukcut_engine::faults::arm("mcp.request:notifications/initialized");
        assert!(server
            .handle_contained(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .is_none());
    }

    #[test]
    fn a_percent_before_a_multibyte_character_is_kept_as_text() {
        assert_eq!(decode("%aé"), "%aé");
        assert_eq!(decode("a%20b%2"), "a b%2");
        assert_eq!(decode("%C3%A9"), "é");
    }

    #[test]
    fn uris_round_trip_paths_with_spaces() {
        let path = "/home/a b/my project%.chukcut";
        let uri = format!("chukcut://frame?path={}&at=2.5s", encode(path));
        let (kind, query) = parse_uri(&uri).unwrap();
        assert_eq!(kind, "frame");
        assert_eq!(query["path"], path);
        assert_eq!(query["at"], "2.5s");
    }

    /// FilmCraft's rule: every listed tool has a title and the four hints,
    /// so a client can run the read-only ones without asking.
    #[test]
    fn every_tool_is_titled_and_annotated_in_both_modes() {
        for bridge in [false, true] {
            for op_tools in [false, true] {
                let mut server = server(bridge);
                server.op_tools = op_tools;
                let tools = server.tools();
                let mut names: Vec<&str> =
                    tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
                for tool in &tools {
                    let name = tool["name"].as_str().unwrap();
                    assert!(tool["title"].is_string(), "{name} has no title");
                    assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
                    assert!(
                        !tool["description"].as_str().unwrap_or("").is_empty(),
                        "{name} has no description"
                    );
                    for hint in [
                        "readOnlyHint",
                        "destructiveHint",
                        "idempotentHint",
                        "openWorldHint",
                    ] {
                        assert!(
                            tool["annotations"][hint].is_boolean(),
                            "{name} lacks {hint}"
                        );
                    }
                }
                for core in [
                    "command_list",
                    "command_run",
                    "command_batch",
                    "doc_inspect",
                    "render_preview",
                    "project_inspect",
                    "sequence_inspect",
                    "media_import",
                    "render_frame",
                ] {
                    assert!(names.contains(&core), "{core} is listed");
                }
                assert_eq!(
                    names.contains(&"ui_click"),
                    bridge,
                    "ui tools only in bridge mode"
                );
                assert_eq!(
                    names.contains(&"split"),
                    op_tools,
                    "op tools only on request"
                );
                let n = names.len();
                names.sort_unstable();
                names.dedup();
                assert_eq!(names.len(), n, "tool names are unique");
            }
        }
    }

    #[test]
    fn an_unknown_argument_names_itself_and_the_accepted_ones() {
        let server = server(false);
        let m = server
            .unknown_arg("command_list", &json!({"filtr": "x"}))
            .unwrap();
        assert!(
            m.contains("\"filtr\"") && m.contains("filter") && m.contains("enabled_only"),
            "{m}"
        );
        assert!(server
            .unknown_arg(
                "command_run",
                &json!({"id": "split", "params": {"whatever": 1}})
            )
            .is_none());
        assert!(server.unknown_arg("split", &json!({"att": 1})).is_some());
        assert!(server
            .unknown_arg("split", &json!({"project": "/a.chukcut", "at": 1}))
            .is_none());
        let m = server
            .unknown_arg("doc_inspect", &json!({"verbose": true}))
            .unwrap();
        assert!(m.contains("project"), "{m}");
        let bridge = self::server(true);
        let m = bridge
            .unknown_arg("doc_inspect", &json!({"project": "/a"}))
            .unwrap();
        assert!(
            m.contains("no arguments"),
            "in the app there is no project argument: {m}"
        );
    }

    #[test]
    fn a_title_is_the_first_sentence_or_the_name() {
        assert_eq!(
            title_of("Split clips at a time. Then more.", "split"),
            "Split clips at a time"
        );
        assert_eq!(title_of("", "speed_curve"), "speed curve");
    }
}
