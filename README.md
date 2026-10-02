# chukcut

**A fast, modern video editor for Linux.** Rust engine, GPU compositor, native GPU UI.

[![CI](https://github.com/chuk-development/chukcut/actions/workflows/ci.yml/badge.svg)](https://github.com/chuk-development/chukcut/actions/workflows/ci.yml)
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

Linux only. NVIDIA and Intel are the targets; AMD should work. Needs Rust
(stable) and system libraries.

```bash
# Debian / Ubuntu
sudo apt install \
  libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev \
  libavdevice-dev libswscale-dev libswresample-dev \
  libva-dev libasound2-dev libshaderc-dev nasm cmake \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev

cargo build --release -p chukcut
./target/release/chukcut                      # new project
./target/release/chukcut clip1.mp4 clip2.mov  # new project with these clips
./target/release/chukcut my.chukcut           # open a project
```

Hardware acceleration is automatic where the driver supports it — VAAPI on
Intel today, NVDEC/NVENC next. `CHUKCUT_DECODE=software|auto|vaapi` overrides
decode if you need to compare.

## Architecture

One process. **The engine owns the machine; the app owns the window.**

```
crates/engine/   chukcut-engine — media, timeline, compositor, audio, export.
                 No UI dependency. modules/<name>/commands.rs is the API.
crates/app/      chukcut — the native app on GPUI (Zed's UI toolkit, wgpu).
```

Every capability is a command in the engine. The app calls it; a CLI and an
MCP server will call the same functions. The project document lives in the
engine and changes only through invertible edit commands.

| Layer | Choice |
|---|---|
| UI | GPUI (native, GPU-rendered) |
| Engine | Rust |
| GPU | wgpu on Vulkan |
| Media | FFmpeg via `ffmpeg-next`; VAAPI today, NVDEC/NVENC next |
| Audio | cpal (ALSA) |
| Lint | rustfmt, clippy |

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
