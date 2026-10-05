# 0035 — Panics are logged and contained, documents are checked before they render, jobs commit only into their own project

Date: 2026-10-05. Status: accepted (robust agent, `agent/robust`).

Findings 1, 2, 3, 8 and 10 of `docs/research/quality-audit-2026-10.md`.

## What was decided

### A panic is a bug: log it, contain it, tell the user, keep the process

- **Every panic goes to the log file**, with the thread, the place and a
  backtrace (`lifecycle::install_panic_hook`). The app installs it in
  `chukcut_engine::init`. The CLI installs it in `init_engine`. The CLI's
  `tracing` writes to stderr only, so the hook writes the report directly to
  the same daily file the app uses (`logging::write_raw`). The ML worker
  writes the report to stderr as lines that start with `panic:`. The editor
  forwards these lines into its log at error level. The previous hook still
  runs after ours, so stderr also keeps its usual message.
- **Every place that must not die with a bug catches the panic** and reports
  it as the failure it already knows how to show.
  `lifecycle::contain` / `contained` turn a panic into the sentence "<what>
  stopped on an internal error: <message>. The log has the details." The
  places are:

  | Where | What the user sees |
  |---|---|
  | Player render thread | "The preview stopped: …" and a **Restart preview** button (`FramePlayer::restart`) |
  | Every export (`run_export`) | The dialog's failure message. The queue marks the item failed and runs the next one. |
  | Export queue worker, outside the export | The running item is failed and the `worker` flag is cleared (`Unstick`) |
  | MCP request | A JSON-RPC `-32603`. The session goes on, and the open projects are reopened from disk. |
  | Bakes (matte, flow, enhance), tracking, analyses, project preparation (per clip and per run) | The job's `Err`, like any other failure |
  | Audio render, compound mix-down, proxy, autosave worker loops | That item fails, and the loop serves the next one |
  | ML worker request | An `inference` error reply. The worker serves the next request. |

- **Faults are injectable**: `faults::arm("point[:key]")` makes the next
  `faults::hit` / `hit_keyed` there panic. Each containment point has a
  test that uses it, or one that injects a panicking runner or listener.
  The key scopes a fault to one job, so parallel tests cannot set off each
  other's faults. When nothing is armed, a hit is one atomic load.

### A document with errors does not render

`Project::validate` now also refuses:

- a canvas outside 2 to 8192 pixels per edge (`MAX_CANVAS_EDGE`),
- a frame rate outside 1 to 240 (`MIN_FPS`, `MAX_FPS`),
- any segment that ends after `MAX_TIME` (100 hours) on the timeline or in
  its source. The check is done with checked arithmetic, so a time near
  `i64::MAX` gives an error and not an overflow.

It warns about an odd canvas edge and about a clip that reads past the end
of its media. `Project::render_check` turns the errors into one sentence.
`export::job::resolve_settings` and `export_snapshot` run it, so the app,
the queue, the CLI and MCP all refuse in the same way.

A project with errors still **opens**. Its errors and the repairs made by
the migration are logged and kept in `AppState::open_notes`. The app shows
them once (`project_open_notes`). Refusing to open the file would lock the
user out of their own work. Refusing to render it puts the reason where it
is.

The first clip's canvas and frame rate are now an edit:
`import_material_planned` returns a `ConfigureCommand`, and
`project_import_media` applies it through the history, so Undo restores
them. Only common rates are adopted (`adopted_fps`): 23.976 to 60, taken
exactly within 0.1 %, or snapped to the nearest common rate within 1.5 %.
For any other rate the project keeps its own, and the import returns a
`notice` that the app shows.

### A background job commits only into the project it was started for

`AppState` has a **generation**. It is raised *before* a project is
installed by open, new, template, restore or recovery, and by close. A job
captures the generation when it starts and commits through a check made
under the project's write lock (`AppState::with_project_of`, or the same
check inline in tracking, analysis and reframe). A commit for another
generation fails with `STALE_JOB` and changes nothing: no edit, no pool
entry, no undo step. `project_close` also cancels tracking, the analyses
and the preparation (`modules::jobs::cancel_all`). Bakes are not cancelled:
they fill caches keyed by the file, not the document. The app's caption
transcript carries the generation too, and is cancelled when its editor
closes. Each editor removes its export-queue listener when it is dropped.

### An export is over when its thread is gone

`run_export` runs the work on a thread of its own. It closes the
provider's decoders on that thread (`SourceProvider::release`) and joins
the thread with `pthread_join`, which waits until the thread-local
destructors have run. A caller that returns from `main` right after the
export, such as a test binary or an embedder, therefore no longer races
the driver's library destructors with a thread that still holds CUDA or
Vulkan state. Shells still exit through `lifecycle::exit`.

## What it costs

- One extra thread per export.
- A cleared decoder cache after every export, including each part of a
  sampled size estimate.
- A `validate` pass per render request, which takes milliseconds on a
  500-clip document.
- A loudness-target export still buffers its range (not the timeline): it
  measures, limits and measures again.
- A clip with audio effects or a pitch-preserving speed change is still
  rendered whole when the range reaches it, because a reverb tail depends
  on all of the clip before it.

## What would change our minds

- A crash report that contained panics hide. Then the policy for that place
  becomes "abort with a report".
- A loudness target that must stream (multi-hour podcasts with a target).
  That needs a streaming true-peak limiter and two mix passes.
