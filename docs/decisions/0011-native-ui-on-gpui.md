# 0011 — The UI is native, on GPUI; the webview is gone

Status: decided 2026-10-02. Implemented the same day: the Tauri shell and the
React frontend are removed, the engine is its own crate (`crates/engine`), and
the app is a GPUI window (`crates/app`). The last webview state is the commit
before this change; nothing of it is lost from history.

This reverses the split described in the old `architecture/overview.md`
("Rust owns the machine, the webview owns the pixels").

## The decision

1. **One native process, one window, drawn on the GPU by GPUI** — Zed's UI
   toolkit, which renders through wgpu on Linux.
2. **The engine has no UI dependency.** `crates/engine` knows nothing of GPUI,
   Tauri or any window system. The app is a shell around it.
3. **The command layer stays.** `modules/*/commands.rs` is the shell-facing API.
   The app calls it directly; a CLI and an MCP server will call the same
   functions. `engine::shell` replaces the two things those files took from
   Tauri: `spawn_blocking` and an event `Channel`.
4. **Linux only.** NVIDIA and Intel first, AMD as best effort. Other platforms
   are not a goal.

## Why

The preview never stopped costing us. Every frame was composited on the GPU,
read back, encoded as JPEG, sent over a custom URI scheme, decoded by the
webview and painted. Months of work went into making that path fast — VAAPI
JPEG, zero-copy into a VA surface — and it was still a path that should not
exist. `docs/KIRU_TEARDOWN.md` shows the alternative in production: a native
GPU toolkit, preview and timeline in one process, frames that never leave the
GPU. `docs/research/GPUI_SPIKE.md` proved GPUI can show an engine-rendered
frame on upstream code with no fork.

The old reason for the webview — native UI iterates slowly, and the egui
attempt (`~/git/chukcut-rust`) drowned in hand-built widgets — was real. Two
things changed: the engine now exists and is the hard part, and agents write
UI code fast enough that iteration speed in TypeScript no longer decides it.

## What it costs

- The React UI's features are not ported yet: inspector, transitions panel,
  text, export dialog, settings, proxies, menus. They are in history and serve
  as the specification.
- GPUI is git-only and its API moves. It is pinned to one commit in
  `crates/app/Cargo.toml`; bump it deliberately.
- The first preview path reads each frame back to the CPU and hands it to
  GPUI as an image (spike path (c)). That is already far cheaper than JPEG over
  IPC, but it is not the end state.

## What comes next, in order

1. **NVDEC/NVENC.** This machine is an RTX 3060 with no VAAPI driver, so decode
   and encode run in software today. FFmpeg's `cuda` hwaccel for decode and
   `h264_nvenc`/`hevc_nvenc` for encode, beside the existing VAAPI paths.
2. **Shared GPU device (spike path (a)).** The engine renders into a texture
   GPUI composites directly. Blocked on version alignment: GPUI is on wgpu 29,
   the engine on wgpu 30.
3. **CLI and MCP server** (`crates/cli`) over the same command layer.

## What would change our minds

A GPUI regression on Linux that blocks us for longer than a fork would take to
maintain.
