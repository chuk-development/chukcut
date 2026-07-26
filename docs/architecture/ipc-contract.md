# IPC contract

The webview has exactly three ways to reach Rust. Nothing else crosses the
boundary.

## 1. Commands — `invoke()`

Request/response. Used for everything that has an answer: open a project, apply
an edit, probe a file, start an export.

**Adding a command**

1. Write the function in `src-tauri/src/modules/<area>/commands.rs`:

   ```rust
   #[tauri::command]
   pub fn media_probe(path: String) -> Result<MediaInfo, String> { ... }
   ```

2. Register it in the `generate_handler!` block in `src-tauri/src/lib.rs`.
   Unregistered commands are unreachable — that is the access control.

3. Add a typed wrapper in `src/modules/<area>/lib/`:

   ```ts
   import { invoke } from "@tauri-apps/api/core";

   export async function probeMedia(path: string): Promise<MediaInfo> {
     return invoke("media_probe", { path });
   }
   ```

   Components call the wrapper, never `invoke` directly. One place to change
   when a signature moves, one place to look when a call misbehaves.

**Conventions**

- Command names are `<module>_<verb>`: `project_save`, `timeline_split`,
  `media_probe`. The prefix is what makes a 60-command handler list readable.
- Errors are `Result<T, String>`. The string is shown to the user, so it reads
  as a sentence: `"cannot read /path/x.mp4: no such file"`, not `"ENOENT"`.
- Arguments arrive camelCase from TS and are matched to snake_case Rust
  parameters by Tauri. Do not fight it.
- A command that mutates the document returns the new document. See
  `timeline::commands::EditResponse`.
- Commands must not block for more than a few milliseconds. Anything longer
  spawns a task and reports through a channel.

## 2. Channels — `Channel<T>`

One-way streams from Rust to the webview: export progress, thumbnail batches,
playback position, decode errors. Preferred over Tauri's global events because
a channel is scoped to the caller instead of broadcast to every listener.

```rust
#[tauri::command]
pub fn export_start(request: ExportRequest, on_progress: Channel<ExportProgress>)
    -> Result<String, String>
{
    std::thread::spawn(move || {
        // ...
        let _ = on_progress.send(ExportProgress { frame, total, fps });
    });
    Ok(job_id)
}
```

```ts
const channel = new Channel<ExportProgress>();
channel.onmessage = (p) => setProgress(p);
await invoke("export_start", { request, onProgress: channel });
```

**A streaming command returns a job id, not the work.** Both channel commands
follow the same shape, and it is worth copying rather than inventing a third:

```ts
const onBatch = new Channel<ThumbnailBatch>();
onBatch.onmessage = (batch) => place(batch.tiles);      // tiles carry their own index
const jobId = await invoke<string>("media_thumbnails", { path, count, height, onBatch });
// …and when the component unmounts:
await invoke("media_thumbnails_cancel", { jobId });
```

Two rules that fall out of this and are not optional:

- **Every job sends exactly one terminal message**, with `complete: true`, and
  it is the last one. Failures arrive *there*, in `error`, not as a rejected
  promise — the call resolved long before the work did. A frontend that only
  handles the promise will never see a decode failure.
- **A batch that cannot be delivered stops the job.** Dropping the channel is
  therefore a legitimate way to cancel, and the explicit `_cancel` command
  exists for the case where the caller wants the work to stop while the channel
  is still alive.

## 3. Custom protocol — `chukcut-frame://`

Bulk binary that would choke on JSON. Currently: preview frames. Registered in
`lib.rs` with `register_uri_scheme_protocol`, served straight from the frame
cache as JPEG bytes with the right content type.

```
chukcut-frame://preview/<session>/<frame-number>
chukcut-thumb://<material-id>/<index>
```

Rationale and frame pacing: `preview-pipeline.md`.

## Rules

- **The webview never gets a file path it did not already have.** Paths come
  from a dialog the user drove, or from the open project. Rust does not accept
  an arbitrary path from the frontend for anything destructive.
- **No command holds the project lock while doing IO.** Take the lock, clone
  what you need, drop it, then work. A decode that blocks the document lock
  freezes the entire UI.
- **Long work is cancellable.** Export and background thumbnailing check an
  abort flag; the user closing a dialog must actually stop the work.
