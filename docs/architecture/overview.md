# Architecture overview

chukcut is a video editor: a Rust engine that owns media, the GPU and the file
system, and a native GPUI shell that owns the window. One process, no webview.
Why: `../decisions/0011-native-ui-on-gpui.md`.

## The crates

```
┌─────────────────────────────────────────────────────────┐
│  crates/app  (chukcut — GPUI)                           │
│                                                         │
│  editor.rs   media panel, preview, timeline, actions    │
│  player.rs   preview render thread (latest-wins)        │
│  edits.rs    gestures → EditCommand                     │
│                                                         │
│  Owns: layout, interaction, view state                  │
└───────────────────────┬─────────────────────────────────┘
                        │  plain function calls into
                        │  modules/*/commands.rs
┌───────────────────────▼─────────────────────────────────┐
│  crates/engine  (chukcut-engine)                        │
│                                                         │
│  src/modules/<domain>/                                  │
│    mod.rs        the capability                         │
│    commands.rs   the shell-facing API                   │
│  src/shell.rs    spawn_blocking + Channel               │
│  src/state.rs    AppState: document, path, history      │
│                                                         │
│  Owns: document, media, GPU, encoding, disk             │
│  Depends on no UI crate.                                │
└─────────────────────────────────────────────────────────┘
```

The command layer is the contract. The app calls it today; a CLI and an MCP
server will call the same functions, so a capability that exists only in the
UI is a bug in where it was written.

## Modules

| Module | Responsibility |
|---|---|
| `project` | Document model, load/save, import, validation, autosave |
| `timeline` | Edit commands, undo/redo, split, link |
| `media` | FFmpeg probe/decode, hardware decode, thumbnails, waveforms |
| `gpu` | The process's one wgpu device and one VAAPI display |
| `render` | wgpu compositor: project + time → frame |
| `preview` | Playback clock; the old JPEG frame server (webview era, unused by the app) |
| `audio` | Mixer and output device; the device's played samples are the clock |
| `export` | Full-resolution render + encode, progress |
| `effects` | Lua + shader effect runtime |
| `text`, `transitions`, `inspector`, `proxy` | As named |
| `workspace` | Paths, caches, settings, recents, logging, hardware report |

## The data flow

There is one source of truth: the `Project` in `AppState`
(`modules/project/document.rs`). Everything else is derived from it.

```
    user gesture (crates/app)
         │
         ▼
  EditCommand ──► commands::timeline_apply ──► History.apply() ──► Project
                                                                      │
                     ┌────────────────────────────┬───────────────────┤
                     ▼                            ▼                   ▼
            app snapshot (Arc<Project>)     render thread        export walk
            redrawn after every edit        project + t → frame  every t
```

The app never mutates the document itself. It builds a command, the engine
applies it, and the app takes a fresh snapshot. The snapshot is an
`Arc<Project>` handed to the render thread and the audio engine as is.

## The preview, today

`player.rs` renders on its own thread with the engine's `Compositor` and a
`MediaSourceProvider`, reads the RGBA frame back, and hands it to GPUI as a
`RenderImage`. The audio device's played-sample count drives the
`PlaybackClock`; the view polls clock and render thread every 8 ms. No JPEG, no
IPC. The end state shares one GPU device with GPUI so frames never leave the
GPU (`../research/GPUI_SPIKE.md`, path (a)).

## What is deliberately not here

- **No platform other than Linux.** NVIDIA and Intel first, AMD best effort.
- **No CapCut project import.** Their format is a design reference for our
  schema, nothing more.
- **No bundled effect assets.** The effect runtime loads packages the user
  points it at.
- **No plugin system yet.**

## Reading order for a new agent

1. `project-format.md` — the data model everything derives from
2. `timeline-editing.md` — how mutations work
3. `../decisions/0011-native-ui-on-gpui.md` — why the UI is native, what is next
4. `transitions.md` — the effect between two clips
5. `../ROADMAP.md` — what is built, what is next

`ipc-contract.md` and `preview-pipeline.md` describe the webview era. They are
kept for the reasoning (frame pacing, staleness, read-ahead), not as a
description of the current app.
