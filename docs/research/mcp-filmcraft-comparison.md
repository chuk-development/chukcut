# The MCP server: FilmCraft's design, ours, and the plan

Date: 2026-10-21. Source: storytold/filmcraft at the `main` commit cloned
that day (shallow clone in `/mnt/data/git/_ref-filmcraft`), Apache-2.0 or MIT
at the user's option.

The owner's brief: chukcut's MCP server should work as well as FilmCraft's
and be designed like it. Take everything FilmCraft does better, keep our UI
and keep FFmpeg. Credit FilmCraft openly. **The design of chukcut's MCP server
and of its control socket is deliberately taken from FilmCraft.** NOTICE.md
lists what was taken; the adapted files carry FilmCraft's copyright line.

## 1. What FilmCraft does

FilmCraft is a pure-Rust NLE (no FFmpeg) with an egui UI. Its MCP server is
the crate `crates/automation` (about 1,650 lines with tests), on the official
SDK `rmcp` 3.5 over tokio, stdio transport. `apps/filmcraft-cli` starts it as
`filmcraft-cli mcp`.

### Two modes, one tool set

| Mode | Command | Drives |
|---|---|---|
| Headless | `filmcraft-cli mcp --demo` / `--project p.fcproj` | an in-process engine `Session`, no window |
| Bridge | `filmcraft-cli mcp --bridge 127.0.0.1:9876` | the running app, started as `filmcraft --control 9876` |

The repository's `.mcp.json` registers both (`filmcraft` = bridge,
`filmcraft-headless` = demo), so Claude Code in the repository offers them.

### Tools: few, and a catalog behind them

FilmCraft does not make one tool per feature. It has 17 tools, and every
capability is an engine **command** with an id (`timeline.razor`,
`effects.apply`) reached through `command_run`:

| Tool | Modes | What |
|---|---|---|
| `command_list` | both | every command: id, label, menu path, shortcut, param doc, enabled now; `filter`, `enabled_only` |
| `command_run` | both | `{id, params}`; undoable edits land in History |
| `command_batch` | both | `{steps: [{id, params}], stop_on_error}` → `{completed, failed, results}`; `isError` when a step failed |
| `doc_inspect` | both | project tree + active sequence in one call |
| `render_preview` | both | same as `render_frame` |
| `project_inspect` | both | bins, items, ids, durations |
| `sequence_inspect` | both | tracks, clips (ticks and frames), effects, markers, playhead, selection |
| `media_import` | both | `paths` (absolute), `bin`, `image_sequence` |
| `render_frame` | both | PNG at `seconds`, `max_side` (default 960); read-only: the playhead is put back |
| `ui_inspect`, `ui_elements`, `ui_click`, `ui_drag`, `ui_key`, `ui_type`, `ui_screenshot`, `ui_control` | bridge | the live window: state, on-screen elements with automation ids, synthetic input, screenshots, any control method |

The point of the shape: an agent learns nine tools and one catalog instead of
reading hundreds of schemas before its first call, and the same command ids
work in the menus, the shortcuts, the CLI (`filmcraft-cli exec <id>`) and MCP.

### Conventions (`docs/agents.md` § Conventions)

- **Titles and all four annotation hints** (`readOnlyHint`,
  `destructiveHint`, `idempotentHint`, `openWorldHint`) on every tool; a test
  checks every listed tool. A client can let the read-only tools run without
  asking.
- **Strict arguments.** An unknown tool argument is JSON-RPC `-32602` naming
  it and the accepted ones (`unknown argument "filtr" for command_list;
  expected: enabled_only, filter`). `command_run.params` goes to the command
  unchanged.
- **Errors.** A failing command is an `isError` result with the engine's
  message. A line that is not JSON gets `-32700` (id null) and the server goes
  on. A panicking command is `internal error: …` and the session stays usable
  (the poisoned lock is recovered).
- **Resources.** `filmcraft://document` (as `doc_inspect`) and
  `filmcraft://commands` (as `command_list`), `application/json`.
- **MCP 2026-07-28.** Clients that negotiate per request (`_meta`
  `io.modelcontextprotocol/protocolVersion`, `server/discover`, current Claude
  Code) get `resultType`, `ttlMs` and `cacheScope` on list and read results;
  older sessions get the old shape.
- **Long exports.** `file.exportMedia` with `wait: true` runs as a background
  job; with a `progressToken` the server sends `notifications/progress`
  (frames done of total, at most every 100 ms); `ping` is answered meanwhile;
  `notifications/cancelled` stops the encode, deletes the partial output and
  sends no response.

### The bridge (`docs/control-protocol.md`)

`filmcraft --control 9876` listens on loopback TCP. One JSON request per line,
one reply per line (`{"id", "ok", "result" | "error"}`). Only requests are
read: anything else (an HTTP request line from a browser's cross-origin
`fetch`, junk) gets one error and the connection is closed. Lines are capped
at 4 MiB, connections at 16. Methods: `engine.execute` (any command),
`engine.commands`, `ui.inspect`, `ui.elements`, `ui.click` / `ui.drag` /
`ui.scroll` / `ui.key` / `ui.type`, `ui.screenshot`, `ui.set`,
`ui.timeline.hit`, `perf.stats`, and more. Element ids are stable
(`timeline.clip.<id>`, `tools.Razor`). Driving the app never steals keyboard
focus. The MCP bridge client (`bridge.rs`) keeps a pool of connections so a
slow command does not hold up others, and turns a blocking export into a
polled app job so the app stays responsive.

### Tests

In `server.rs` and `bridge.rs`: an in-process client over `serve_io` (raw
JSON-RPC lines), every tool titled and annotated, strict arguments and parse
errors, batch semantics, resources, the 2026-07-28 shape, export progress and
cancel, a poisoned lock that keeps serving, `doc_inspect` pinned to one
sequence, and fake control servers for the bridge.

## 2. What chukcut had (before this change)

`crates/cli/src/mcp.rs`, about 870 lines, hand-written JSON-RPC over stdio
(no rmcp, no tokio), protocol 2024-11-05 to 2025-11-25.

- **One tool per operation**: about 150 tools (`split`, `grade`,
  `title_add`, …), each with a JSON schema derived from the same clap/serde
  struct the CLI parses, plus `new_project`, `view_frame`, `batch`,
  `catalog`, `ml`, `template_*`.
- **Project files only.** Every tool took `project`, an absolute path. Each
  project stayed open for the connection (undo across calls); every edit was
  saved at once, a failed one rolled back, a file changed on disk reloaded.
- `readOnlyHint` and `openWorldHint` from two lists; no titles, no
  `destructiveHint` or `idempotentHint`.
- Unknown arguments of an operation were a tool error (`isError`), not a
  protocol error.
- Resources `chukcut://project?path=` and `chukcut://frame?path=&at=`.
- Progress notifications for long operations; **no cancellation**, and
  `ping` waited behind a running export.
- `view_frame` rendered the full canvas to a temporary PNG file and read it
  back.
- **No way to reach the running app.** An agent edited a file; the app only
  saw it after reopening.

## 3. The comparison

| | FilmCraft | chukcut before | chukcut now |
|---|---|---|---|
| Transport | stdio, rmcp + tokio | stdio, hand-written | stdio, hand-written (a reader thread for ping and cancel) |
| Tool shape | 9 core tools + catalog | ~150 tools | 10 core tools + catalog; the 150 behind `--op-tools`, and accepted by name always |
| Running app | `--bridge` over TCP control channel | none | `--bridge` over a Unix control socket (`chukcut --control`) |
| Live UI tools | inspect, elements, click, drag, key, type, screenshot, control | none | inspect, click, drag, key, type, screenshot, control; no `ui_elements` |
| Picture | `render_frame` PNG, `max_side`, read-only | `view_frame`, full size via a temp file | `render_frame` / `render_preview`, `max_side`, rendered small, in memory |
| Annotations | title + 4 hints, tested | 2 hints | title + 4 hints, tested |
| Unknown argument | `-32602` naming accepted ones | tool error | `-32602` naming accepted ones |
| Parse error | `-32700`, keeps serving | same | same |
| Panic | internal error, session usable | same | same |
| Resources | `document`, `commands` | project, frame | `document`, `commands`, project, frame |
| 2026-07-28 cache hints | yes | no | yes (`server/discover`, `resultType`, `ttlMs`, `cacheScope`) |
| Progress | export | export, transcription, analysis, … | the same, also through the bridge |
| Cancel | export: stops, deletes partial, no response | none | export: stops, partial deleted by the job, no response; other commands: no response, they run to their end |
| Batch | each step its own undo step | all or nothing | both (`command_batch`, `atomic: true`) |
| Undo | the app's History | per connection | per connection headless; the app's own history in bridge mode |
| `.mcp.json` | both servers | none | both servers |

## 4. Decisions

**The command catalog is the operations.** chukcut's capabilities are
operations in `crates/cli/src/ops/` over the engine's `modules/*/commands.rs`.
`command_list` lists them with id (the batch name: `split`), label, group (the
`ops` module), parameter schema, `readOnly`, `openWorld`, `needsProject` and
`enabled`. We did not invent dotted ids (`timeline.razor`): the batch files,
the CLI manual and the catalog keep one word for one thing.

**The bridge runs the operations in the app process.** chukcut's architecture
makes this cheap: the app and the CLI share the engine, and an operation only
needs an `AppState`. The CLI crate became a library as well as a binary; the
app links it and serves `chukcut_cli::control` on a socket. An operation run
over the socket goes through `EditCommand` into the app's own undo stack, and
the editor's 8 ms tick takes a fresh snapshot when the socket says the
document changed. The `ui.*` methods cross to the UI thread through a queue
the tick drains; keys and clicks are dispatched deferred, after the tick's
update, so they never re-enter a borrowed editor.

**A Unix socket, not TCP.** FilmCraft guards its loopback port against
browsers with the request-only rule and has no authentication. A Unix socket
in `$XDG_RUNTIME_DIR` (mode 0600 in a 0700 folder) cannot be reached by a web
page at all and only by the user. We kept the request-only rule, the 4 MiB
line cap and the 16-connection cap anyway.

**Progress lines instead of a job poll.** FilmCraft's bridge starts an export
as an app job and polls `jobs.list`, because its control replies time out
after 60 s on the UI thread. Our commands do not run on the UI thread, so the
socket sends `{"id", "progress", "label"}` lines before the reply, and the
MCP server forwards them as `notifications/progress`. Cancel is
`engine.cancel {request}` on a second connection.

**`render_frame` renders small.** A new engine command,
`export::commands::export_preview_png(state, time, max_side)`, renders through
the export compositor at the size the caller wants (longest side 960 by
default) and returns PNG bytes. Headless it renders the project; in bridge
mode the app renders its own document at its playhead. Nothing is written to
disk.

**The `ui_*` tools are listed in bridge mode only.** FilmCraft lists them in
both modes and fails them headless. Six tools that cannot work are context an
agent pays for in every turn; we list them where they work.

**No `ui_elements`.** FilmCraft's egui registers an automation id per widget.
GPUI has element ids and hitboxes but no public way to enumerate them, so
`ui_click` takes window coordinates and `ui_screenshot` shows where things
are. Open point below.

**Screenshots need X11.** GPUI's `render_to_image` exists only in its test
build. The window is captured with ImageMagick's `import -window <XID>` (the
XID from `raw-window-handle`). On Wayland `ui_screenshot` fails with a
sentence that points at `render_frame`, which works on any display.

**No rmcp.** The reasons in the old module comment still hold: the CLI has no
async runtime and handles one request at a time. The one thing that needed
concurrency — answering `ping` and taking a cancel while a long command runs —
is a reader thread.

## 5. What was not taken, and why

- **`--demo`.** FilmCraft opens a built-in demo project. chukcut has none and
  media never enters git; `new_project` plus `media_import` is the start.
- **Bridge connection pool.** FilmCraft keeps several connections so a slow
  command does not hold up a fast one. Our MCP server handles one request at
  a time, so one connection (plus one for a cancel) is enough.
- **Read-only render that moves the playhead and puts it back.** Our bridge
  renders the document through the engine at any time without touching the
  playhead, so there is nothing to restore.

## 6. Open points

- `ui_elements` with stable ids needs an element registry in the app: every
  interactive element would register an id and its bounds while painting.
  That is a UI-wide change, not part of this one.
- `ui_screenshot` on Wayland (a portal capture, or rendering the GPUI scene
  offscreen).
- A focus rule like FilmCraft's ("driving the app never steals the user's
  keyboard"): `ui_key` dispatches to the window without raising it, but
  nothing has been measured under a compositor that raises on input.
- Cancelling commands other than export (transcription, analysis) needs a
  cancel flag in those engine jobs; today the server drops their result.
