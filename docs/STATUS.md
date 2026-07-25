# Where this project stands

Written to be read first by anyone — human or agent — picking this up cold.
Sessions are long and are not reopened, so nothing important is allowed to live
only in a conversation. If you learn something that would change how the next
person works, it belongs in this repository, not in a chat log.

Last updated: 2026-07-25.

## What this is

`chukcut` — a CapCut-style video editor. Rust engine, Tauri 2 shell, React
frontend. Everything lives in this repository at `~/git/chukcut`. There is no
other source tree; earlier attempts (`~/git/chukcut-rust`, `~/git/x`) are
reference material only and are described under "Prior work" below.

Run the built app:

```bash
cd ~/git/chukcut
pnpm tauri build --no-bundle          # ~1.5 min incremental
./src-tauri/target/release/chukcut
```

Do **not** judge performance from `pnpm tauri dev`. See "Traps" below.

## What works, verified

Each of these was measured or checked against an independent tool, not assumed.

| Capability | Evidence |
|---|---|
| Import of arbitrary formats | FFmpeg probe; canvas and frame rate adopted from the first clip |
| Frame-accurate decode | `examples/render_smoke.rs` — output matches ffmpeg's own frame at the same timestamp to a max channel delta of 2, no pixel differing by more than 8 |
| GPU compositing | wgpu on Vulkan, Intel Raptor Lake iGPU; transform, crop, opacity, keyframes |
| Preview playback | ~13.7 ms per frame at p50 against a 33.3 ms budget, decode p50 7.2 ms |
| Audio playback | cpal, 48 kHz stereo; the device's played-sample count is the clock master |
| Timeline editing | Magnetic docking, razor at the pointer, undo/redo via invertible commands |
| Export | `examples/export_smoke.rs` — 240 declared **and** 240 decodable frames, exact 4.000 s duration, AAC track at −18.2 dB mean, −1.7 dB peak |

Two of those deserve emphasis because they are the failure modes that usually
go unnoticed: the export is **not** truncated (the classic un-flushed-encoder
bug), and its audio is **not** silent.

## What is known to be rough

- **Seeking backwards during playback stalls briefly.** The ring buffer is
  discarded on every seek, so a backward seek into a region just played costs a
  full decoder seek (130–227 ms measured). Task #9: key the cache to document
  identity rather than to the session id.
- **Speed changes shift pitch.** Both in preview and export. A phase vocoder is
  its own piece of work.
- **The first `play` after launch can lead the sound** by up to one audio
  buffer, because the queue origin is marked before the stream exists. Every
  subsequent play and seek is exact.
- **Export runs on the CPU** at ~22 fps for 1080×1920. Hardware encoding is the
  next major piece; see `docs/decisions/0002-ffmpeg-and-hardware-encoding.md`.
- **`media/waveform.rs` fails on files with a channel count but no channel
  layout** (plain PCM WAV, typically). FFmpeg 6 replaced the layout bitmask with
  `AVChannelLayout` and `ffmpeg-next` still configures swresample through the
  legacy API. The fix exists in `audio/decode.rs::name_the_layout` and needs
  transplanting.

## Not built yet

Text and titles, transitions, keyframe editing in the UI, audio waveforms on
the timeline, proxy media for 4K, and the CapCut effect runtime (Phase 3).

## Traps that have already cost time

- **`pnpm tauri dev` builds Rust unoptimized.** `jpeg-encoder` needs ~130 ms per
  preview frame at `opt-level = 0` and ~7 ms in release. This made playback look
  fundamentally broken when it was merely unoptimized. `Cargo.toml` now sets
  `opt-level = 1` for our crate and `3` for dependencies, but the release build
  is still the one to judge by.
- **`cargo check`/`build` need `-j 4`.** Full parallelism gets rustc OOM-killed
  on this machine while compiling wgpu and the Tauri macro crates.
- **A frame request must never fail.** WebKitGTK tears down its web process
  under a stream of failed resource loads — an observed crash, not a theory. The
  protocol handler answers with a neighbouring frame (within two frames) or a
  204, never a 404.
- **Never treat a webview frame request as a seek.** Doing so made the decoder
  jump ahead, which forced the next playback frame to seek backwards, which made
  the renderer fall further behind. Single frames took 13.5 seconds. See the
  comment on `PreviewServer::request_frame`.
- **`ffmpeg-next` holds non-atomic `Rc` in both the format and codec contexts.**
  `VideoDecoder` is `Send` only because all handles move together and access is
  serialised by the provider's mutex. That mutex is load-bearing; read the
  safety comment before touching it.

## Prior work, and what it is good for

- `~/git/x/capcut-renderer` — 18k lines of Rust reverse-engineering CapCut's
  effect format (wgpu + mlua + a GLSL-ES compiler). The source for Phase 3.
- `~/git/x/python_renderer_specs/` — effect format, Lua API, shader and render
  graph specifications from that work.
- `~/git/chukcut-rust` — the abandoned egui attempt. Useful only for the CapCut
  screenshots in `screenhsots orignal cpacut/`.
- `~/capcut-libs/` — CapCut Web JavaScript bundles.
- `ssh 10.11.12.79` — Windows machine with CapCut 9.0.0.3858 installed, for
  further reverse engineering. Read-only; drive it with
  `ssh 10.11.12.79 'powershell -NoProfile -Command "..."'`.

## The research

Four documents under `docs/research/` were produced by dedicated agents and are
worth reading before making an architectural decision:

- `media-stack-options.md` — GStreamer/GES vs MLT vs building our own. Verdict:
  keep the custom stack; the frameworks give away the timeline we already have
  and withhold the hardware decode and proxy machinery we lack.
- `rust-crate-survey.md` — per-subsystem crate recommendations, with maintenance
  signals. Notably: hardware encode is *not* blocked by our FFmpeg version, and
  naga cannot consume the GLSL ES dialect CapCut ships.
- `draft-format.md` — a full specification of CapCut's project format, with
  units and coordinate conventions established by evidence.
- `effect-package-format.md`, `engine-symbols.md`, `ui-inventory.md` — how
  CapCut is built.
