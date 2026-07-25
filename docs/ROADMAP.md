# Roadmap

Phases are ordered so that each one ends with something a person can actually
use. An editor that imports, cuts and exports is a product; a perfect effect
renderer with no timeline is a tech demo.

## Phase 0 — Skeleton ✅

- Tauri 2 + React 19 + Vite + Tailwind v4 + shadcn + Zustand + Biome
- Mirrored module layout, `src-tauri/src/modules/` ↔ `src/modules/`
- Project document model (`project/document.rs`)
- Edit commands and undo/redo (`timeline/ops.rs`, `timeline/history.rs`)
- IPC contract documented and enforced by `generate_handler!`

## Phase 1 — The spine

The point of this phase: import a file, cut it, watch it, export it.

- **media** — FFmpeg probe, frame decode by timestamp, thumbnail strips, audio
  waveform peaks
- **render** — wgpu compositor: transform, crop, opacity, blend, canvas
  background
- **preview** — frame server over the custom protocol, playback clock, scrub
  and play paths, audio output as clock master
- **export** — full-res walk through the compositor into an FFmpeg encoder,
  presets, progress reporting, hardware encoder detection
- **frontend** — four-panel layout, media library, timeline with drag/trim/
  split/snap, preview player with transport, inspector for the selected clip

Done when: drop in three clips, trim them, add a title, export a 1080×1920 MP4
that plays correctly in another player.

## Phase 2 — The editor people expect

- Text rendering with the full `TextMaterial` surface (stroke, shadow, box)
- Transitions between adjacent clips
- Keyframe editing UI with an easing picker
- Audio: per-clip volume envelopes, fades, waveform-accurate trimming
- Speed: constant and curved, with pitch preservation
- Masks and chroma key
- Adjustment: brightness/contrast/saturation/HSL/curves
- Markers, in/out points, ripple/roll/slip/slide edits
- Multi-select, grouping, copy/paste across tracks

## Phase 3 — Effects runtime

Port the reverse-engineered engine from `~/git/x/capcut-renderer`:

- Lua VM (`mlua`) exposing the `Amaz` API surface
- GLSL ES 1.0/3.0 → WGSL translation, multi-pass graphs, ping-pong render
  targets
- Effect package loader: manifest, shaders, resources, parameter schema
- Parameter UI generated from the manifest

Packages are fetched at runtime from a URL the user supplies. Nothing
ByteDance-authored ships in the bundle, ever.

## Phase 4 — Scale and polish

- Proxy media generation for 4K sources
- Background render cache so scrubbing over rendered regions is instant
- Project templates and presets
- GPU encoder paths (VAAPI / QSV / NVENC / VideoToolbox)
- Auto-save and crash recovery
- Windows and macOS builds

## Non-goals

- Importing CapCut projects. Their format was a design reference, not an
  interop target.
- Cloud sync, accounts, telemetry.
- Mobile. The layout assumes a mouse and a wide screen.

## Current status

See the task list. Phase 0 complete, Phase 1 in progress.
