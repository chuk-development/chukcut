# chukcut

**A fast, modern video editor for Linux.** Rust engine, GPU compositor, web UI.

[![CI](https://github.com/chukfinley/chukcut/actions/workflows/ci.yml/badge.svg)](https://github.com/chukfinley/chukcut/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org)

> **Status: early, and honest about it.** Import, cut, preview and export work
> end to end, each verified against an independent tool rather than assumed.
> It is not yet a daily driver. See [`docs/STATUS.md`](docs/STATUS.md) — the most
> useful file in this repository.

## Why this exists

Linux has professional editors with professional learning curves, and it has
editors that feel like 2012. What it does not have is the thing most people
actually want: something quick and obvious, like CapCut, that opens a phone
video and gets out of the way.

There is also a concrete gap. DaVinci Resolve's free build on Linux **cannot
import or export H.264/H.265 at all**, and even Resolve Studio has no AAC on
Linux — so the ordinary case, a phone MP4 in and an MP4 out with sound, means
transcoding around the editor. chukcut treats that case as the default one:
FFmpeg-based import of arbitrary formats, hardware H.264/HEVC export on the GPU,
AAC audio, verified frame counts and audio levels in the test suite.

## What works today

Every row here was measured or checked against an independent tool. The full
evidence, including the commands that reproduce it, is in
[`docs/STATUS.md`](docs/STATUS.md).

| | |
|---|---|
| **Import** | Any format FFmpeg reads; canvas and frame rate adopted from the first clip |
| **Timeline** | Magnetic docking, razor, ripple, multi-select, rubber band, clipboard, linked A/V, markers, in/out |
| **Undo** | Invertible edit commands — a whole multi-clip edit undoes as one step |
| **Preview** | Frame server over a custom protocol, audio as the clock master, hardware decode by default |
| **Effects** | GLSL ES → WGSL rewriter, effect package loader, transitions, keyframes |
| **Text** | Full title rendering — stroke, shadow, box — identical in preview and export |
| **Export** | Range export, presets, hardware H.264/HEVC, AAC, progress, frame snapshots |

### Performance, measured

`cargo run --release --bin chukcut-bench -- --all` generates its own fixtures,
refuses to report on a busy machine, and writes JSON baselines for regression
comparison. On an Intel Raptor Lake iGPU:

| | before | after |
|---|---|---|
| Preview frame, 1920×1080 (decode → composite → JPEG) | 11.93 ms | **4.36 ms** |
| Preview frame, 1080×1920 | 11.41 ms | **3.98 ms** |
| Decode to texture | 20–66 ms | **0.8–3.3 ms** |

The "after" column is zero-copy: the compositor draws NV12 straight into a VA
surface the media driver allocated, and decoded frames arrive as DMA-BUF
textures. Nothing is read back, converted or re-uploaded.

Correctness is pinned the same way. Hardware and software decoders are asserted
to agree to a mean channel difference under 2 across 34 tests; hardware export
matches software encode at 51–53 dB PSNR on luma; a decoded frame matches
FFmpeg's own frame at the same timestamp to a max channel delta of 2.

## Build and run

Needs Rust (stable), Node 20+, pnpm, and system libraries.

```bash
# Debian / Ubuntu
sudo apt install \
  libwebkit2gtk-4.1-dev libsoup-3.0-dev librsvg2-dev \
  libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev \
  libavdevice-dev libswscale-dev libswresample-dev \
  libva-dev libasound2-dev libshaderc-dev nasm cmake

pnpm install
pnpm tauri dev
```

For anything performance-related, build in release — `pnpm tauri dev` is not
representative, and [`docs/STATUS.md`](docs/STATUS.md) explains why:

```bash
pnpm tauri build --no-bundle
./src-tauri/target/release/chukcut
```

Hardware acceleration is automatic where the driver supports it.
`CHUKCUT_DECODE=software|auto|vaapi` overrides decode if you need to compare.

## Architecture

**Rust owns the machine, the webview owns the pixels.** No file access, no
process spawning, no decoding and no GPU work happens in TypeScript. Every
capability crosses the boundary as a registered Tauri command.

```
src-tauri/src/modules/<name>/     src/modules/<name>/
  mod.rs      what it owns          components/   React
  commands.rs the IPC surface       lib/          typed invoke() wrappers
  <impl>.rs   the work              store.ts      Zustand slice (UI state only)
```

Module names are mirrored on both sides: `timeline` in Rust and `timeline` in
TypeScript are one feature seen from two directions. The project document lives
in Rust and is replaced wholesale after each edit — the frontend never patches
it locally.

| Layer | Choice |
|---|---|
| Shell | Tauri 2 |
| UI | React 19, TypeScript, Vite, Tailwind v4, shadcn/ui, Zustand |
| Engine | Rust |
| GPU | wgpu (Vulkan / Metal / D3D12 / GL) |
| Media | FFmpeg via `ffmpeg-next`, VAAPI for hardware paths |
| Lint | Biome, rustfmt, clippy |

## Documentation

This project writes things down. If a session's transcript vanished, the next
person should still be able to continue.

- **[`docs/STATUS.md`](docs/STATUS.md)** — what works and what it cost to get
  there. Read this first.
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — phases, in order
- [`docs/decisions/`](docs/decisions/) — one file per decision that would be
  expensive to revisit, written when it was made
- [`docs/architecture/`](docs/architecture/) — overview, project format, IPC
  contract, timeline editing, preview pipeline
- [`docs/research/`](docs/research/) — investigations, including the ones that
  concluded "no"
- [`CLAUDE.md`](CLAUDE.md) — the working agreement, for humans and agents alike

## Contributing

Yes, please. [`CONTRIBUTING.md`](CONTRIBUTING.md) has the workflow; the short
version is that tests skip rather than fail when a machine has no GPU or no
FFmpeg, so you can work on the timeline without a Vulkan device.

Good first areas: keyframe editing UI, audio envelopes, speed curves, masks and
chroma key, colour tooling. The roadmap marks them.

## Licence

GPL-3.0-or-later. See [`LICENSE`](LICENSE), and
[`docs/decisions/0010-open-source-under-gpl.md`](docs/decisions/0010-open-source-under-gpl.md)
for why copyleft rather than something permissive.

Codec patents (H.264, H.265) are licensed separately from software, by the
patent pools, and are the responsibility of whoever distributes or uses a
binary. This project distributes source code and pays no royalties, in the same
position as VLC, Kdenlive and HandBrake. See [`NOTICE.md`](NOTICE.md).

No ByteDance-authored assets — effects, fonts, templates, icons — are in this
repository or in any build. The effect runtime loads packages from a URL the
user provides at runtime.
