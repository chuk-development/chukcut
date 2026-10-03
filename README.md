# chukcut

**A CapCut-style video editor for Linux, with a native GPU interface.**
Open a phone video, cut it, grade it, caption it and export it. The interface
and the compositor both run on the GPU, in one Rust process.

[![CI](https://github.com/chuk-development/chukcut/actions/workflows/ci.yml/badge.svg)](https://github.com/chuk-development/chukcut/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org)

![The chukcut editor: media panel, player with a title over the clip, the animation presets in the inspector, and the timeline with a text lane and a transition](docs/images/editor.png)

> **Status: early.** The features below exist and have tests. The app is not
> yet a daily driver: some panels have rough edges, and a few controls are
> placeholders. [`docs/STATUS.md`](docs/STATUS.md) says what works, what does
> not, and what each number cost to measure.

## Why

Linux has editors with a professional learning curve, and it has editors that
feel old. It does not have the quick, obvious editor that most people want for
short-form video.

There is also a concrete gap. DaVinci Resolve's free build on Linux cannot
import or export H.264 or H.265, and Resolve Studio has no AAC on Linux. So the
ordinary case, a phone MP4 in and an MP4 with sound out, means transcoding
around the editor. chukcut makes that case the default: FFmpeg import of any
format, hardware H.264/HEVC export on the GPU, and AAC audio.

## Features

**Editing**
- Magnetic timeline: razor, ripple trim and delete, snapping, markers, in and out marks
- Multi-select, rubber-band selection, cut, copy, paste and duplicate
- Linked audio and video; detach audio; fades with handles on the clip
- Keyframes on clips, speed changes, crop, transform and opacity
- Undo for every edit, including multi-clip edits as one step
- Start screen with recent projects, autosave, crash recovery and an unsaved-changes guard

**Colour**
- Basic adjustments: exposure, temperature, tint, saturation, vibrance, shadows, highlights and more
- HSL, curves and colour wheels (shadows, midtones, highlights, offset)
- `.cube` LUTs (1D and 3D) with a library, and one-click filters
- The preview and the export use the same shader, so they show the same pixels

**Effects and transitions**
- 17 GPU effects: glow, shake, light sweep, RGB split, glitch, blur, vignette, film grain, halation, bloom and more
- Effects on a clip, or as an effect clip that applies to everything below it
- Picture in picture and split-screen layouts with rounded corners
- 120 transitions from the gl-transitions library, plus seamless transitions

**Motion and tracking**
- In, Out and Combo animation presets with an easing library
- A text animator by letter, word or line
- Punch-in zoom, and automatic zoom on jump cuts
- Motion tracking: draw a box on the player, and a title or overlay follows the object

**Captions and speech**
- Automatic captions, offline with whisper.cpp or with any OpenAI-compatible server
- Word or sentence captions, karaoke highlight, styles, emoji
- SRT and VTT import and export
- Caption translation (DeepL or an OpenAI-compatible model)
- Text to speech, sound effects and music with your own ElevenLabs key

**Audio cleanup**
- Silence and filler-word cutting with a review list
- Voice cleanup (RNNoise) and a loudness target (EBU R128) on export

**Export**
- Hardware encode: NVENC on NVIDIA, VAAPI on Intel and AMD; software x264/x265 as the fallback
- H.264 and HEVC with AAC, range export, presets, frame snapshots
- Each hardware encoder is test-encoded before the dialog offers it

**Command line and MCP**
- `chukcut-cli`: every edit from a shell or a script: import, cut, grade, effects, titles, captions, silence cutting, export
- `chukcut-cli mcp`: the same operations as an MCP server, so an AI agent such as Claude Code can edit a project
- Batch files with undo, JSON output and exit codes for scripts. Free, not a paid tier: [`docs/cli.md`](docs/cli.md)

**Your own accounts, optional:** Pexels, Pixabay and Freesound stock search,
fal.ai background removal and upscaling with a price shown before you run it.
Keys stay on your machine (`secrets.toml`, mode 0600).

| Colour grading | Transition library |
|---|---|
| ![Colour wheels in the Adjust tab, with the filter tiles in the asset panel](docs/images/colour.png) | ![The transition library in the asset panel, with a cross dissolve between two clips on the timeline](docs/images/transitions.png) |

## Hardware

NVIDIA and Intel come first. AMD should work through VAAPI and Mesa, but
nobody tests it regularly.

| GPU | Decode | Encode | Notes |
|---|---|---|---|
| NVIDIA (proprietary driver) | NVDEC: H.264, HEVC, VP9, AV1 | NVENC | Frames go to the compositor as NV12 textures |
| Intel (iHD driver) | VAAPI: H.264, HEVC, VP9, AV1 | VAAPI | Zero-copy: decoded frames go to the GPU as DMA-BUF |
| AMD (Mesa) | VAAPI | VAAPI | Best effort |
| No GPU | software | x264 / x265 | Needs a Vulkan driver; Mesa's lavapipe works, slowly |

You need a working Vulkan driver (`vulkaninfo` lists your GPU). chukcut picks
the hardware path when the driver supports it, and falls back to software when
it does not. `CHUKCUT_DECODE=software|auto|vaapi|cuda` forces a decode path.

## Install

From a clone of this repository:

```bash
scripts/install.sh
```

The script checks the build dependencies first. When something is missing, it
prints the exact `apt` (Debian, Ubuntu, Mint) or `dnf` (Fedora) line for you to
run, and stops. It never installs packages itself. Then it builds a release
binary and installs it for your user only, with no sudo:

- `~/.local/bin/chukcut`
- a menu entry, icons, the `.chukcut` file type and AppStream metadata under `~/.local/share`

Other options: `--check` (only check dependencies), `--cuda` (whisper.cpp with
CUDA, needs `nvcc`), `--no-build` (install the binary you already built) and
`--uninstall`. Uninstalling keeps your projects and settings.

A release tarball (`packaging/tarball.sh`) contains the binary and the same
installer. It links the FFmpeg of the system it was built on, so it runs only
on a distribution with the same FFmpeg version. [`packaging/README.md`](packaging/README.md)
says what is bundled and what is not.

## Build

Linux only. You need Rust (stable, from [rustup.rs](https://rustup.rs)) and
these system packages:

```bash
# Debian / Ubuntu / Mint
sudo apt install \
  libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev \
  libavdevice-dev libswscale-dev libswresample-dev \
  libva-dev libasound2-dev libshaderc-dev libclang-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev \
  build-essential pkg-config cmake nasm
```

On Fedora, `scripts/install.sh --check` prints the `dnf` line. FFmpeg with
H.264 and HEVC comes from RPM Fusion there.

```bash
cargo build --release -p chukcut
./target/release/chukcut                      # start screen
./target/release/chukcut clip1.mp4 clip2.mov  # new project with these clips
./target/release/chukcut my.chukcut           # open a project
```

The command-line tool and MCP server build the same way:

```bash
cargo build --release -p chukcut-cli
./target/release/chukcut-cli --help
claude mcp add chukcut -- "$PWD/target/release/chukcut-cli" mcp   # for Claude Code
```

The first build compiles wgpu, GPUI and whisper.cpp and takes a while. Use
`-j 4` on a 32 GB machine; full parallelism can run out of memory.

For development, `scripts/run-dev.sh` runs a debug build with its own config,
data and cache directories, so a test session never touches your real
settings or recent projects:

```bash
scripts/run-dev.sh --fresh -- _scratch/media/vertical.mp4
```

## Architecture in one minute

One process. **The engine owns the machine; the app owns the window.**

```
crates/engine/   chukcut-engine: media, timeline, compositor, audio, export.
                 No UI dependency. modules/<name>/commands.rs is its API.
crates/app/      chukcut: the native app on GPUI (Zed's UI toolkit).
crates/cli/      chukcut-cli: the same commands from a shell, and an MCP server.
```

- **Every capability is a command** in the engine. The app calls it. The CLI
  and the MCP server (`crates/cli`) call the same functions.
- **Every change to the project is an `EditCommand`.** Each one has an exact
  inverse, which is how undo, autosave and validation work.
- **Time is exact:** `i64` microseconds, never floats or frame numbers.
- **One GPU device.** The engine opens one wgpu device on Vulkan and one VAAPI
  device, and shares them. The compositor draws every frame, for the preview
  and the export, with the same shaders.
- **Media:** FFmpeg through `ffmpeg-next`, with hardware decode and encode.
  Audio goes out through cpal (ALSA), and the audio device is the playback clock.
- **A project is one JSON file**, `<name>.chukcut`
  ([format](docs/architecture/project-format.md)).

[`docs/architecture/overview.md`](docs/architecture/overview.md) has the full
picture.

## Documentation

- **[`docs/STATUS.md`](docs/STATUS.md)**: what works, what it cost, and the traps. Read this first.
- [`docs/cli.md`](docs/cli.md): `chukcut-cli` and its MCP server, every command with examples
- [`CHANGELOG.md`](CHANGELOG.md): what each release contains
- [`docs/ROADMAP.md`](docs/ROADMAP.md): phases, in order
- [`docs/decisions/`](docs/decisions/): one file per decision that would be expensive to revisit
- [`docs/architecture/`](docs/architecture/): overview, project format, timeline editing, preview pipeline, transitions
- [`docs/design/language.md`](docs/design/language.md): colours, type, spacing and icons of the UI
- [`docs/research/`](docs/research/): investigations, including the ones that concluded "no"
- [`packaging/README.md`](packaging/README.md): install routes and what each one bundles
- [`CLAUDE.md`](CLAUDE.md): the working agreement, for humans and agents

## Contributing

Yes, please. [`CONTRIBUTING.md`](CONTRIBUTING.md) has the workflow. You do not
need a GPU: tests that need one skip with a printed reason, so you can work on
the timeline without a Vulkan device.

## Licence

GPL-3.0-or-later. See [`LICENSE`](LICENSE), and
[`docs/decisions/0010-open-source-under-gpl.md`](docs/decisions/0010-open-source-under-gpl.md)
for why copyleft.

Codec patents (H.264, H.265) are licensed separately from software, by the
patent pools. They are the responsibility of whoever distributes or uses a
binary. This project distributes source code and pays no royalties, in the
same position as VLC, Kdenlive and HandBrake. See [`NOTICE.md`](NOTICE.md).

No ByteDance-authored assets (effects, fonts, templates, icons) are in this
repository or in any build. The screenshots show chukcut's own interface and
media generated with FFmpeg's test sources.
