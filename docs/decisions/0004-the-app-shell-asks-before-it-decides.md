# 0004 — The app shell asks before it decides

> **Webview era.** Written for the Tauri + React shell, which was removed on
> 2026-10-02 (decision 0011). Kept for its reasoning; the code paths it names
> under `src/` and `src-tauri/` no longer exist.

Status: decided 2026-07-26. Covers the start screen, project lifecycle,
settings, the hardware panel and the crash-recovery seam in `src/app/` and
`src/modules/workspace/`.

## The decision

**The editor no longer creates a project at launch.** With nothing open, the
window shows a start screen: three canvas presets at 1080p, a name field, an
Open button, and the recent list. `project_new` is not called until someone
presses something.

**Every destructive lifecycle action is guarded.** New, Open and opening a
recent project go through `workspace/lib/lifecycle.ts::guardUnsaved`, which
raises a three-way prompt when the document is dirty. Saving only clears the way
if the save actually landed — a cancelled file dialog does not then discard the
work, which is the specific bug the guard exists to stop.

**Settings write through on every change**, one whole `Settings` struct at a
time, optimistically, reverting on refusal. There is no OK button and nothing is
batched.

## Why

The previous behaviour was `projectNew({ name: "Untitled", 1080×1920, 30 })` in
an effect at the top of `App`. It meant the first thing anyone did in a session
was undo a decision the app had made for them, and it meant the recent list —
which Rust has maintained the whole time in `workspace::RecentProjects` — was
reachable from nowhere in the UI. Both of those are the launch screen's job.

The whole-struct write is not a style choice: `workspace_settings_set` takes a
`Settings`, Rust has no per-field setter, and a partial write would silently drop
every preference the sending build did not know about — including one a newer
build wrote. `store.test.ts` pins that.

## What takes effect without a restart, and what cannot

| Setting | How it bites |
|---|---|
| `snapping` | Pushed into `useTimelineStore` from `App`. The `S` key still toggles it for the session without persisting. |
| `default_canvas`, `default_fps` | Read by the start screen and the New Project dialog when they open. |
| `cache_limit` | Persisted only. Nothing trims the cache to it yet — see below. |
| `preview_max_edge`, `preview_full_quality`, `preview_quality` | **Not yet.** See below. |

**The preview settings do not currently reach the preview.** `preview_start`
takes a `PreviewOptions { longEdge, quality }`, and
`preview/lib/session.ts::open()` passes `null`. Nothing on the Rust side reads
`Settings` either, so these three values are persisted and inert. `App` restarts
the preview session when they change, which is the half of the wiring that is in
this module's scope; the other half is one line in `session.ts::open()` —
`previewStart(channel, time, { longEdge, quality })` from the workspace store —
and it belongs to whoever owns the preview module.

**Nothing trims the cache to `cache_limit`.** `workspace::paths` can measure the
cache and delete all of it; there is no partial eviction. The setting is honest
about being a limit the user has expressed, not one the engine enforces yet.

## The hardware panel, and the command it wants

Settings → Hardware answers "is it actually using my graphics card", and it is
built on the fact that the engine establishes hardware support **by running it**:
`export/hwaccel.rs` encodes a 320×240 frame per encoder, `media/hwdecode.rs`
decodes an embedded bitstream per codec. So the panel distinguishes three states,
not two — working, absent, and *present but refused, with the driver's reason* —
because the third is the one this hardware produces constantly (`av1_vaapi` is in
every FFmpeg build and this chip cannot encode AV1).

Only half of that is reachable over IPC. `export_presets` carries
`Vec<HwEncoder>` because the export dialog needs it. **The decode probe and the
identity of the adapter the compositor opened are exposed by no command.**

`workspace/lib/hardware.ts` therefore asks for `workspace_hardware` first and
falls back to `export_presets` when it is not registered, marking the result
`partial` so the panel can say that an empty decoder section means "not asked"
rather than "not supported". The command it wants is small:

```rust
#[derive(Serialize)]
pub struct GpuInfo { name: String, backend: String, driver: String, device_type: String }

#[derive(Serialize)]
pub struct HardwareReport {
    gpu: Option<GpuInfo>,                       // from the render context's wgpu AdapterInfo
    encoders: Vec<export::hwaccel::HwEncoder>,  // hwaccel::detect()
    decoders: Vec<media::hwdecode::HwDecodeSupport>, // hwdecode::capabilities()
    decode_default: String,                     // media::provider::DEFAULT_ACCELERATION
}

#[tauri::command]
pub fn workspace_hardware() -> HardwareReport
```

Both probes are already cached for the process, so it costs one round trip after
the first call. Delete the fallback in `hardware.ts` when it lands.

## Crash recovery: what was assumed, and what changed under us

The frontend seam is `workspace/lib/recovery.ts`, and it is inert — none of the
commands it names is registered. It is worth recording *why it is shaped the way
it is*, because the Rust side made a decision mid-flight that changes the
question the UI should ask.

`project/autosave.rs` and `project_get` (both uncommitted when this was written)
chose to **apply** the working copy rather than offer it: the first `project_get`
after launch restores `<config>/autosave.chukcut` into `AppState` and answers
with it, once, and the path the user last saved to is remembered in a sibling
`.path` file.

That is defensible, and it makes "restore the recovered project?" the wrong
question — the restore has already happened before anyone could be asked. The
right question becomes **"do you know that what you are looking at is not in your
file yet?"**, because otherwise the restore is completely silent: the header
reports a clean document, the user closes the window, and only the working copy
ever held their afternoon.

So there are two seams, and they are mutually exclusive:

1. `restoredFromWorkingCopy()` → `workspace_recovery_status`, which reports
   whether the open document came out of the working copy. **This is the one that
   matches the code as it stands.** The shell shows a banner and marks the
   document dirty so the next Ctrl+S puts it somewhere real. One command over
   state `restore_working_copy` already has.
2. `pendingRecovery()` / `restoreRecovery()` / `discardRecovery()` → the
   offer-first design, where `project_get` answers `null` and the start screen
   asks. Better in one respect only, but a real one: it is the only version the
   user can *refuse*, which matters when the working copy is from a project they
   deliberately abandoned.

Whichever lands, delete the other half of `recovery.ts`.

## What was deliberately not built

- **A guard on window close.** `onCloseRequested` can veto a close, but
  proceeding afterwards needs `core:window:allow-destroy`, which `core:default`
  does not grant. A guard that can trap the user in the app is worse than no
  guard, so this needs a capability change in `src-tauri/capabilities/` first.
- **Editing the recent list** (pin, remove, clear). Rust prunes missing files
  before answering and the frontend drops an entry whose open fails; that covers
  the case people actually hit.

## The native app (2026-10-03)

The GPUI shell implements this decision again, in `crates/app/src/editor/`:
`shell.rs` (the window root and every way in and out of a document),
`home.rs` (the start screen), `lifecycle.rs` (dirty state, the guard, saving,
the window title), `settings.rs`, `shortcuts.rs` and `playback.rs`.

**Crash recovery took seam 2, offer-first.** A clean way out of a document
(`project_close`: going home, quitting, closing the window after the guard)
deletes the working copy, so one that survives to the next launch means the
session crashed. At launch, before the command line can open anything,
`project_recovery_claim` moves it into a recovery slot
(`autosave.recovered.chukcut`) — otherwise opening `chukcut x.chukcut` would
schedule an autosave over the very work about to be offered. The app then
asks: Restore, Discard, or Not now (the start screen keeps a banner, and the
slot survives to the next launch). A working copy identical to the file it
came from is discarded silently; a session lock with the process id keeps a
second instance from taking a running one's working copy for a crash.
`project_get`'s apply-on-first-call path is not used by the native app.

**Dirty is a fingerprint, not a counter.** The editor hashes the serialised
document and compares it with the hash at the last save, open or creation.
The serialisation is deterministic, so undoing back to the saved state is
clean again. Restored work has no baseline and is always unsaved.

**The window-close guard exists now.** GPUI's `on_window_should_close` can
veto a close and the app quits itself once the guard passes, so the reason
it was left out of the webview (a guard that could trap the user) no longer
applies.

**Settings that reach the native app:** `default_canvas`, `default_fps`
(start screen and command-line imports), `preview_scale` and
`preview_max_edge` (the player's render size, live), `audio_scrubbing`
(live). `proxy_policy` and `cache_limit` are persisted and shown, but nothing
acts on them yet: the player does not switch to proxies, and nothing trims
the cache. The dialog says so.
