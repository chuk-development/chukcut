# Where this project stands

Written to be read first by anyone — human or agent — picking this up cold.
Sessions are long and are not reopened, so nothing important is allowed to live
only in a conversation. If you learn something that would change how the next
person works, it belongs in this repository, not in a chat log.

Last updated: 2026-10-02 (**the UI is native now** — decision 0011. The Tauri
shell and the React frontend are gone; the engine is `crates/engine`
(`chukcut-engine`, no UI dependency) and the app is a GPUI window in
`crates/app`. What the native app does today: import (dialog or command line),
a media list, click-to-append onto the timeline, preview with playback driven
by the audio clock, scrubbing, clip select/move/delete, split, undo/redo,
open/save, timeline zoom and scroll. Verified on an RTX 3060 under X11: Vulkan
render device, software decode (no VAAPI driver on NVIDIA), playback advancing
in step with the clock. Everything the React UI had beyond that — inspector,
transitions, text, export dialog, settings, proxies — is not ported yet; the
engine side of all of it is intact and compiles, tests included.)
Previously 2026-08-05 (**the project is public, under GPL-3.0** — decision
0010. Two things follow that are not paperwork: the licence question 0002 was
built around is settled, so linking the distribution's ordinary `--enable-gpl`
FFmpeg and using `libx264`/`libx265` in-process is now allowed and the
LGPL-clean build is no longer required; and the repository now runs `cargo fmt`,
clippy and both test suites in CI, which meant formatting the whole tree once —
rustfmt had never been enforced, 483 sites — and taking clippy's
machine-applicable fixes. **Clippy is reported but not fatal**: about 28 style
lints remain, none of them correctness, and `-D warnings` belongs in CI only
once that list is empty). Previously 2026-07-28: (**the media
library is now a view of the project's
pool, and missing media is a state** — removing an import is an undoable edit
that leaves the clips offline instead of deleting them; decision 0009. The bug
behind "saving does not save my imports" was never the save: the pool was
always persisted, but `MediaLibrary` rendered a session list a restart
forgets). Previously 2026-07-27: the preview stopped copying its frames — the
JPEG encoder now reads a surface the compositor drew into, 2.7–2.9× on a whole
frame; and earlier the same day, the attempt that went the other way round and
the `vkDeviceWaitIdle` crash it found.

## What this is

`chukcut` — a CapCut-style video editor for Linux. Rust engine, native GPUI
shell, one process. Everything lives in this repository at `~/git/chukcut`
(GitHub: `chuk-development/chukcut`). Earlier attempts (`~/git/chukcut-rust`,
`~/git/x`) are reference material only and are described under "Prior work"
below. The webview UI that most of this file describes lives in history before
decision 0011.

Run the app:

```bash
cd ~/git/chukcut
cargo build --release -p chukcut
./target/release/chukcut [project.chukcut | media files…]
```

Judge performance from a release build only.

## Open work, in order

1. **NVDEC and NVENC — done 2026-10-03, on an RTX 3060.**
   - Decode: `media::hwdecode` has a second backend, CUDA (`HwBackend::Cuda`,
     `gpu::cuda_device()`), probed per codec like VAAPI — H.264, HEVC, VP9 and
     AV1 all decode. `Acceleration::Auto` tries VAAPI, then NVDEC, then
     software; `CHUKCUT_DECODE=cuda` forces it. NVDEC frames have no DMA-BUF
     export, so they are downloaded as NV12 and uploaded as the two textures
     the compositor's YUV path already samples — no swscale.
     `decode_bench` (`CHUKCUT_BENCH_HW=cuda`): 1080p H.264 **1.6 ms** against
     12.9 ms in software, 4K HEVC **3.9 ms** against 19.2 ms. Downloaded and
     converted to RGBA on the CPU instead, NVDEC was *slower* than software
     (14.6 / 28.5 ms) — the same lesson the VAAPI path taught.
   - **Trap: every seek on NVDEC costs ~25 ms, fixed.** FFmpeg 6.1 re-runs
     `get_format` after `avcodec_flush_buffers` and rebuilds the NVDEC decoder.
     On real footage that still beats decoding a GOP in software; on a tiny
     fixture it does not, which is why the seek-timing test skips NVDEC.
   - Encode: the export already knew NVENC. Proxies did not open on it
     (all-intra with B-frames); fixed with `bf=0`, plus a software fallback.
   - Acceptance: `tests/every_card.rs` plays, scrubs and exports a timeline
     with jump cuts on every decode path and every usable encoder here, and
     reads the frame index back out of every frame.
   - Found on the way: export sampled frames on their first microsecond, so a
     cut whose source offset was a rounded frame time showed the frame before —
     a duplicated frame at the cut. `project::SAMPLE_SLACK` (10 µs) fixes the
     export and the app's preview.
2. **Shared GPU texture with GPUI** so preview frames never leave the GPU.
   Investigated 2026-10-03: needs a patched GPUI either way; the plan (DMA-BUF
   import, no wgpu alignment) is `docs/research/gpui-shared-texture.md`. The
   readback path is now asynchronous and BGRA (see "Perf" below).
3. **Port the UI the webview had:** inspector, export dialog, text,
   transitions, trim handles, thumbnails and waveforms on clips, settings.
4. **CLI and MCP server** over the command layer (`crates/cli`).

## What works, verified

Each of these was measured or checked against an independent tool, not assumed.

| Capability | Evidence |
|---|---|
| Import of arbitrary formats | FFmpeg probe; canvas and frame rate adopted from the first clip |
| Performance, reproducibly | `cargo run --release --bin chukcut-bench -- --all` — six groups, 75 rows, generates its own fixtures, refuses to report on a busy machine, `--json`/`--compare` for regressions. Baseline for this commit in `src-tauri/benches/baseline-4da5cff.json` |
| Frame-accurate decode | `examples/render_smoke.rs` — output matches ffmpeg's own frame at the same timestamp to a max channel delta of 2, no pixel differing by more than 8 |
| GPU compositing | wgpu on Vulkan, Intel Raptor Lake iGPU; transform, crop, opacity, keyframes |
| Preview playback | `chukcut-bench --filter preview-frame` — **10.7 ms** for a whole 1920×1080 frame (decode, composite, readback, JPEG), i.e. 32% of the 30 fps budget and 64% of the 60 fps one, measured serially at native resolution on a quiet machine |
| Audio playback | cpal, 48 kHz stereo; the device's played-sample count is the clock master |
| Timeline editing | Magnetic docking, razor at the pointer, undo/redo via invertible commands |
| Multi-selection and the clipboard | Ctrl/Cmd+click, Shift+click along a lane, a rubber band over the lanes, Ctrl+A; cut, copy, paste and duplicate. Moving, trimming and deleting a selection is **one** undo step — `timeline_apply_many` → `ops::compose_edits`, which also expands link partners exactly once and orders the parts so no intermediate state overlaps. Paste lands at the playhead on the clip's own lane, or the next free one, and never overlaps. Tests: `ops.rs` (batch composition, links, ordering), `timeline/lib/clipboard.test.ts`, `selection.test.ts`, `batch.test.ts` |
| Linked audio and video | An imported file with both streams lands as two clips on two lanes that move, trim, split and delete as one, and can be unlinked. `modules/timeline/ops.rs` — the mirrored move, trim, split and delete each undo in one step, and `tests/round_trip.rs` proves the linkage survives a save. Decision `docs/decisions/0005-linked-audio-and-video.md` |
| Export | `examples/export_smoke.rs` — 240 declared **and** 240 decodable frames, exact 4.000 s duration, AAC track at −18.2 dB mean, −1.7 dB peak |
| Hardware preview JPEG (VAAPI) | `mjpeg_vaapi` on the Intel iGPU. A 1080x1920 preview frame encodes in 6.2 ms against 31 ms for the old pure-Rust encoder, and matches the software encoder's picture at 37 dB PSNR. Falls back to libjpeg-turbo on any machine or frame size the device refuses. **On by default**, after the deadlock that had it switched off — see "The hang that was not the device" |
| Zero-copy preview JPEG | The compositor draws NV12 straight into a VA surface the media driver allocated, so nothing is read back, converted or uploaded. `chukcut-bench --filter preview-frame`, both arms in one process: **11.93 → 4.36 ms** at 1920×1080 and **11.41 → 3.98 ms** at 1080×1920. Right per pixel at 1440, 1360, 700 and 394 — 41.8–47.5 dB against libjpeg-turbo, against the copying path's own 37 dB. Gated on a row-ramp probe through the real encoder, never on a capability list. `docs/research/preview-zerocopy-jpeg.md` |
| Hardware export (VAAPI) | `h264_vaapi` and `hevc_vaapi` on the Intel iGPU. 240 declared and 240 decodable frames, exact 8.000 s, audio identical to the software export at −17.7 dB mean. Frames match the software encode at 51–53 dB PSNR on luma and 60–62 dB on chroma |
| Hardware decode (VAAPI) | H.264, HEVC, VP9 **and AV1**, each probed by decoding a real embedded frame. `tests/decode.rs` — 34 tests, 22 of them run against both the software and hardware decoders and pass identically, including every seek, VFR and rotation case. The two decoders produce the same picture to a mean channel difference under 2 |
| Zero-copy decode into wgpu | `examples/dmabuf_import.rs` — a decoded VA surface exported as DMA-BUF and imported as two wgpu textures reconstructs the software decode's picture to a mean channel difference of **0.32**, with a deliberately chroma-swapped control at 41.9 |
| Hardware decode through the compositor | `examples/hwdecode_pipeline.rs` — the imported surface is composited by the quad shader and matches a software-decoded composite of the same instant to a mean channel difference of **1.1–1.4**, against 9.4 for a deliberately wrong colour matrix. Decode to texture falls from 20–66 ms to 0.8–3.3 ms; a whole preview frame from 39–72 ms to 7.5–16 ms. **On by default** |
| Media survives the session, and losing it is survivable | The library renders `project.materials` (decision 0009), so a reopened project shows its imports; "Remove from project" is `EditCommand::RemoveMaterial` — exact undo, clips kept, file untouched. An offline clip (material removed, or file gone from disk) draws red with an offline icon on the timeline and the library card, composites as a flat dark-red field (`media::MISSING_MEDIA_RGBA`, served by the provider), is a validate *warning*, and the export refuses it by name ("2 clips reference media that is missing: …"). `tests/missing_media.rs` is the whole scenario, pixel assertion included |
| Export range and frame snapshots | `tests/export.rs` — a 1 s..3 s range of a 4 s counter timeline yields exactly 60 decodable frames whose first/middle/last are source frames 30/59/89 (rebased to zero), with 2.0 s ± 0.1 of AAC; marks past the end clamp rather than fail; `export_snapshot` writes a decodable canvas-size PNG of the requested frame through the export compositor, never the preview's panel-sized picture. Dialog wiring (range select, long-edge resolutions, estimated size, remember-settings, snapshot button) in `src/modules/export/**.test.*`, 106 vitest |
| Motion tracking (T1) | `modules/tracking`: pyramidal KLT + RANSAC similarity fit, a colour model (object against its ring) that re-finds the object, and a colour template fallback; pure Rust, 640 px analysis. `tests/tracking.rs` on generated clips (release, loaded machine): a red disc over testsrc2, 1280×720, **90–130 frames/s**, centre error **mean 1.35 px, worst 5.1 px**, no frame lost, rotation drift under 2°; a thrown, motion-blurred ball at ~30 px/frame, **mean 1.25 px, worst 6.2 px**. In the debug app with the UI running: 31–42 frames/s. The follower is drawn where the track says in both the preview and the NV12 export path (`a_follower_is_drawn_on_the_object_in_preview_and_export`); trim, slip, speed, move, scale and split of the tracked clip keep it on the object (`tracking::follow` tests). Decision 0012 |

Two of those deserve emphasis because they are the failure modes that usually
go unnoticed: the export is **not** truncated (the classic un-flushed-encoder
bug), and its audio is **not** silent.

## What is known to be rough

- **RESOLVED 2026-07-27, owner-confirmed ("hat perfekt funktioniert"): the
  stutter is gone. Kept here as a diagnosis record because each of its causes
  can come back.** It was never one bug; it was four, and the sum was what the
  owner felt. Anyone re-investigating a "timeline/playback laggy" report should
  check all four before assuming a new cause:
  1. **The pipeline rendered ~8× the pixels the screen showed** — canvas-sized
     frames into a panel-sized widget. Fixed: `preview_viewport`, panel-size
     rendering (11.68 → 3.75 ms/frame). Regression sign: `render_width` in the
     playback log far above the panel's device-pixel width.
  2. **Delivering pixels to the JPEG encoder was 60% of the frame** — readback,
     CPU convert, upload. Fixed: the compositor draws into VAAPI's own surfaces
     (encode thread 5.32 → 1.18 ms). Regression sign: the startup log line
     "composite straight into the JPEG encoder's own surfaces" missing — the
     probe refused, and the readback fallback is carrying every frame.
  3. **The playhead re-rendered the world.** The fastest-changing value in the
     app was a subscription of the whole Timeline, the Inspector and the
     Preview: three big React trees per position event and per scrub move, on
     the same thread that decodes the preview's JPEGs. Fixed: every consumer
     subscribes itself; `bodyPaintCount` and its paint test pin it. Regression
     sign: that test failing, or any new `useTimelineStore((s) => s.playhead)`
     in a component bigger than a line of text.
  4. **Silent frame loss in the webview**: a 204 answered as `response.ok`,
     threw inside `createImageBitmap`, was swallowed, and nobody retried — up
     to a quarter of frames on a loaded run, plus duplicate renders of the
     frame being awaited. Both fixed alongside the viewport work.

- **The follow-up it caused, also resolved 2026-07-27: playback looked *soft*
  on the very build that fixed the stutter** — "die Abspielqualität ist nicht
  1080p". The log said it in one line: `render_width=960 render_height=540
  downscaled=true rung=0`, so not the quality ladder — a hard cap. History:
  `preview_max_edge` defaulted to 960 and was **inert since birth**
  (`session.ts` passed `null`; the dialog wrote a value nothing read). Wiring
  the settings turned that never-chosen default into a silent cap on every
  preview. Fix: default and stored inert-era 960 are now 0 = automatic
  (`Settings::migrated`, versioned so a *deliberate* 960 chosen after the wiring
  survives). This is the third resolution cap the user never asked for — canvas
  adoption at 1080 and the preview's 720 were the first two. The pattern to
  refuse in review: **a fixed resolution number as a default in a path the user
  watches.** Auto-size from the surface, cap only on request.

- **Playback stutters, and it is now diagnosed.** Full working, with the
  reproduction commands, in **`docs/research/preview-performance.md`**. The
  short version, measured 2026-07-27 at `3440e48` with a real `PreviewServer`
  playing real 1080p footage:
  - **The stages are not serialised.** The render and encode threads overlap
    exactly as `server.rs` claims, while there is headroom: with headroom, 0 of
    155 frames were encoded on the render thread and it idled 88% of the time.
  - **It is raw throughput after all, and the margin is thin.** At 1080×1920 the
    pipeline renders 58 fps at load 7 and **34 fps at load 40** — so on a busy
    machine it drops under a 24 fps demand, the ring empties, and every frame
    request then blocks for up to `FRAME_WAIT` = 60 ms. Measured at load 33:
    21.6 fps rendered, the ring at or behind the playhead 47.8% of the time,
    and 64 of 239 frames answered 204.
  - **60–76% of the frame was delivering finished pixels to the JPEG encoder** —
    readback, CPU RGBA→NV12, upload. **Fixed on 2026-07-27, on the second
    attempt.** The first attempt allocated the NV12 buffer, described it as
    linear and let the driver import it; this chip's JPEG engine reads such a
    surface as though it were tiled while its video engine reads the identical
    file descriptor correctly. The second turns it round: **VAAPI allocates the
    surface, Vulkan imports its two planes as colour attachments, and the
    compositor draws NV12 straight into them**, so the layout is the driver's own
    and nothing on our side declares one. `chukcut-bench --filter preview-frame`,
    both arms in one process: 11.93 → **4.36 ms** at 1920×1080 and 11.41 →
    **3.98 ms** at 1080×1920, i.e. 72% → 26% of the 60 fps budget. Three runs
    over an hour give ratios of 2.6–2.9× at both aspects; quote the ratio, not
    the milliseconds. On a running server, A/B in one process, the
    encode thread falls from 5.32 to **1.18 ms** a frame.
    `docs/research/preview-zerocopy-jpeg.md`.
  - **Rendering at canvas resolution rather than panel resolution is the single
    biggest recoverable item**: capping the long edge to 960 took 34.1 fps to
    61.1 fps and every failure symptom to zero, back to back in one process.
    **This one is now fixed** — the preview renders at the measured size of the
    player panel, and a second binary arrived at the same conclusion from the
    other direction (3.1× the frame cost, "The preview renders the panel, not
    the canvas" below). Whether it is enough to close this entry is not known:
    nobody has re-measured the reported 11–20 fps with it in place.
  - The number quoted two rows above as "Preview playback | **10.7 ms**" is a
    *serial latency* on a generated fixture at load 2.4, not a playback rate.
    On real 1080p footage at load 10 the serial frame is 15.8 ms. The research
    document lists five more claims in this repository that it contradicts.

  Originally reported by the owner on 2026-07-26 against the build that fixed
  the export stride, in his words "total... leckt immer noch geisteskrank rum",
  and again on 2026-07-27 as "aber noch lahm".

  What was known before that investigation, kept because the reasoning is why
  it looked like a tail and was not:
  - The per-frame numbers do not obviously explain it. The last DEBUG session
    logged composite 5–10 ms and encode 6–19 ms against a 33.3 ms budget, with
    `over_budget=false` throughout. So the *mean* looked fine — but
    `stats::record_frame` is given `max(composite, encode)` rather than the
    frame's real cost, and neither term includes the inline encode that
    back-pressure forces onto the render thread once the encoder falls behind.
  - Three known contributors, each measured and each insufficient alone:
    backward seeks discard the ring (130–227 ms, the entry below), the frame
    cache is keyed to the session rather than the document (Task #9), and the
    webview decodes a JPEG per frame on the UI thread.
  - **The file log now shows it. This was the first step and it is done.**
    `preview frame ready` is still DEBUG — promoting it would be one unbuffered
    `write` syscall per frame on the encode thread, i.e. measurement that
    changes what it measures — so `preview/stats.rs` aggregates instead and
    emits one INFO line a second while playback runs, plus one when it stops.
    Nothing has to be reproduced under `RUST_LOG` any more: **ask him for
    `~/.local/state/chukcut/logs/chukcut-YYYY-MM-DD.log` and grep
    `preview playback`.**

    ```text
    INFO …preview::stats: preview playback shown=30 dropped=2 discarded=0 rendered=30
      scrubs=0 over_budget=1 mean_ms=12.7 p99_ms=120.25 max_ms=120.0 budget_ms=33.333
      window_ms=1000.0 decode="vaapi" encode="vaapi" width=1080 height=1920
      downscaled=false rung=0 ladder="full" render_width=1080 render_height=1920
      render_quality=88
    ```

    How to read it, because the whole point is that the mean already looked
    fine: `shown` far above `rendered` is a **stalled renderer** — the clock
    moved and the picture did not. `mean_ms` inside `budget_ms` with `max_ms`
    far outside it is a **tail**, which is where this bug was always going to
    be. `window_ms` much above 1000 means the **pacer itself** was blocked.
    `dropped` above zero is the renderer losing the race outright, and
    `discarded` above zero is it finishing frames that were already too late to
    show. `rung` above 0 means the quality ladder gave up resolution to keep up,
    and `render_*` is what a playback frame was actually made of — `width` and
    `height` stay the size a *paused* frame gets, which the ladder never
    touches. Four more lines, each once per occurrence: `preview decode path`
    (with the reason, once per run), `preview seek was slow` (over 50 ms, with
    the direction — backwards is the expensive one), `preview JPEG encoder
    changed backend`, and `preview quality ladder moved`.

    Two things about it that will otherwise cost time. **The decode path is
    inferred, not asked**: `media::provider::acceleration` is private and the
    provider reaches the preview as an opaque `dyn SourceProvider`, so
    `stats::decode_path` reproduces that function's rule from the same two
    public inputs (`CHUKCUT_DECODE`, `RenderContext::can_import_dmabuf`). It is
    exact today and is the one thing here that can silently go stale. And
    **`downscaled` is not `modules::proxy`**: it means the preview is rendering
    below the canvas. Proxy *media* is not wired into the preview at all yet;
    when it is, it belongs in `DecodePath` as a third value.
- **Seeking backwards during playback stalls briefly.** The ring buffer is
  discarded on every seek, so a backward seek into a region just played costs a
  full decoder seek (130–227 ms measured). Task #9: key the cache to document
  identity rather than to the session id.
- **Speed changes shift pitch.** Both in preview and export. A phase vocoder is
  its own piece of work. Clips on a speed curve are muted for the same reason.
- **An export doubles the audio of a linked clip.** An imported file with both
  streams is now two segments naming the same material — the picture on a video
  lane, the sound on an audio lane — and the mixer's rule is "a video material
  whose container carries audio makes sound, wherever it sits", so both are
  planned unless something says otherwise. `Project::sound_is_on_a_linked_lane`
  is that something and `audio::mixer::plan` asks it, so **the preview is
  right**. `export/audio.rs` holds its own copy of the same resolution and does
  not, because that file was being edited by other work when this landed. One
  call in `export/audio.rs`, next to its `audio_path`, against the same document
  method. Until then an export of such a project sums that clip's audio twice:
  6 dB louder, and phasing with any microsecond the two are out by.
- **The first `play` after launch can lead the sound** by up to one audio
  buffer, because the queue origin is marked before the stream exists. Every
  subsequent play and seek is exact.
- **The export encoder can run on the GPU; most of the export still does not.**
  VAAPI H.264/H.265 works and is measured below. The gain is real but it is
  **1.5×, not the order of magnitude the decision document projected**, because
  the encoder stopped being the bottleneck and nothing else moved. Numbers and
  the reason are under "Hardware export, measured" below.
- **Waveforms and thumbnails are done and stream; nothing here is known rough.**
  Recorded because the next person will otherwise re-derive the two traps:
  a file with a channel *count* and no channel *layout* (plain PCM WAV, and
  therefore most of what gets dropped on an audio lane) decodes to frames whose
  `AVChannelLayout` is `AV_CHANNEL_ORDER_UNSPEC`, and `ffmpeg-next` 6.1 still
  configures swresample through the legacy mask API, so **every such frame is
  rejected with "Input changed"**. `waveform.rs::name_the_layout` relabels the
  frame; the same eight lines are in `audio/decode.rs`, which met it first.
  `tests/media_data.rs` builds the WAV by hand rather than with `ffmpeg`,
  because an `ffmpeg`-written file may carry a mask and then proves nothing.
  Second: a peak-only waveform draws a solid block for anything compressed, so
  the envelope carries signed min/max **and** RMS per bucket.
  Third, and the one that will bite anything else that caches derived data:
  **size plus mtime is not a file identity.** The kernel stamps mtime at clock
  tick granularity, so a file rewritten in place without changing length keeps
  its key and serves stale thumbnails forever — a unit test writes `one` then
  `two` and both writes land in the same tick. `thumbnails.rs::fingerprint` now
  folds in 8 KB from each end of the file as well, which costs nothing against a
  decode and catches every container whose header or trailer moves. Both the
  thumbnail and waveform caches share that one function on purpose, so a single
  edit invalidates everything derived from a file at once.

## Perf: the native preview and the export, measured (2026-10-03)

RTX 3060 (NVIDIA 5xx, Vulkan, NVDEC/NVENC), 16 threads. **No Intel GPU in this
machine**, so every VAAPI path below is unchanged and unmeasured here — run
`tests/every_card.rs` and the harness on an Intel box before trusting it there.

Reproduce (headless, never opens a window, generates its media under
`crates/engine/target/player-bench/`):

```bash
cargo run --release -p chukcut-engine --example player_bench            # stages + playback
cargo run --release -p chukcut-engine --example player_bench -- --quick
cargo run --release -p chukcut-engine --example player_bench -- --only export   # 60 s export
```

Projects: 1 layer = one clip; 3 = the clip graded (contrast, saturation,
vignette, grain) + a graded picture-in-picture + a title; 5 = those + a second
picture-in-picture + a title animated letter by letter over its whole span.
"4K" is a 4K HEVC main clip rendered at 3840×2160 (a 4K fullscreen viewer);
"4K in 960x540" is the same project in an ordinary viewer.

### What a preview frame cost, and where (ms per frame, serial)

Before (commit `8b7ff30`, the old `player.rs`: blocking readback, CPU swizzle,
CPU text animator over the whole frame). **Measured at load 38** — other agents
were compiling — so the absolute numbers are inflated; the shape is the point.

| | sources | composite | readback | swizzle | UI upload | render ceiling |
|---|---:|---:|---:|---:|---:|---:|
| 1080p, 1 layer | 6.65 | 1.06 | 14.02 | 3.36 | 18.28 | 40 fps |
| 1080p, 5 layers | 31.34 | 0.64 | 4.53 | 2.94 | 4.98 | 25 fps |
| 4K, 1 layer | 29.58 | 0.35 | 50.11 | 12.97 | 59.60 | 11 fps |
| 4K, 5 layers | **175.44** | 0.56 | 48.10 | 12.02 | 59.13 | **4 fps** |
| 4K in 960×540, 5 layers | 40.44 | 1.37 | 2.10 | 0.33 | 0.37 | 23 fps |

After, at load 8 (both arms in one process, so the old arm also has the new
text animator — that is why its 4K/5-layer sources are 13 ms, not 175):

| | old: sources / readback+swizzle / total | new: sources / readback / total |
|---|---:|---:|
| 1080p, 1 layer | 1.53 / 2.17 / 4.41 ms | 1.48 / 1.38 / 3.54 ms |
| 1080p, 5 layers | 4.63 / 2.62 / 8.00 ms | 4.35 / 2.04 / 7.16 ms |
| 4K, 1 layer | 5.02 / 8.38 / 16.65 ms | 5.52 / 6.76 / 16.73 ms |
| 4K, 5 layers | 13.18 / 10.95 / 28.23 ms | 16.64 / 10.33 / 34.56 ms |

Read it as: on a quiet machine the serial cost of one frame barely moved,
because nothing was slow except the text animator; **what changed is overlap
and robustness**. Playback, 4 s real time at 30 fps, frames that reached the
screen of 120:

| | load 36, old | load 36, new | load 8, old | load 8, new |
|---|---:|---:|---:|---:|
| 1080p, 5 layers | 110 (lag 1.2 fr) | 113 (lag 0.0) | 120 (lag 0.0) | 120 (lag 0.0) |
| 4K, 1 layer | 43 (lag 3.0) | **68** (lag 0.8) | 119 (lag 0.8) | 120 (lag 0.0) |
| 4K, 3 layers | 62 | 73 | 119 (lag 0.8) | 120 (lag 0.0) |
| 4K, 5 layers | 27 | 27 | 119 (lag 1.0) | 120 (lag 0.0) |
| 4K in 960×540, 5 layers | 102 | **117** | 101 | **112** |

"Lag" is how many frames behind the audio clock the picture on screen was. The
old player showed the frame it was asked for one render-latency late — always
behind the sound by up to a frame at 4K; the new one renders ahead and shows
each frame on its instant.

### What changed in the preview

- **`render::readback::BgraReadback`**: a compute pass packs the target into
  tight BGRA rows (no 256-byte padding, no CPU swizzle), a ring of three
  mapped buffers reused per size, `map_async` collected later. Byte-exact
  against `render_frame` (test).
- **`preview::player::FramePlayer`** (engine, headless, benchmarked) replaces
  the app's render loop: render-ahead of 4 frames on the audio clock, late
  frames skipped by measured latency instead of queued, latest-wins scrubbing
  with in-flight frames thrown away, an edit invalidates the ring.
- **Decode-ahead**: `SourceProvider::prefetch` decodes every visible clip of the
  next frame in parallel (one thread per clip; decoders now have one lock each)
  on a helper thread while the current frame is read back. Waits for the GPU
  first when the provider hands out VAAPI surfaces, which go back to the
  decoder's pool on drop.
- **Copying a 4K frame out of the mapped buffer costs ~5 ms** (31.6 MB; memory
  bandwidth, on a quiet machine). Under load 36 the same copy measured 46 ms.
  A `mallopt` that kept frame buffers off `mmap` was tried and A/B'd four times
  at load 10: no difference, so it was not kept. At 4K the copy and GPUI's own
  re-upload are now the largest items; only the shared texture removes them.
- **Text animator**: `text::animate::compose_region` composes only the box the
  glyphs and backdrop reach and uploads only that into a GPU-cleared texture.
  It had zeroed and walked a frame-sized `f32` buffer per frame: **127–175 ms
  of a 4K frame, now ~3**.
- **Proxies**: `proxy_consider(path, policy)` on import, project open and a
  settings change; `Auto` builds for footage the decision rule says will not
  decode in time (4K HEVC; not 1080p H.264). The player previews from the
  proxy (`with_preview_proxies`) and picks up a finished one on its next
  request; the export never sees one (`proxy::switch`).
- **Not done**: GPUI still re-uploads every frame (a new frame-sized atlas
  texture each time). `docs/research/gpui-shared-texture.md` has the measured
  path and the plan (DMA-BUF into a patched GPUI renderer).

### Export, 60 s of 1080p, the 5-layer project (1800 frames)

| | before | after |
|---|---:|---:|
| `h264_nvenc` | 51.9 fps (load 11) | **135.1 fps** (load 9) |
| `libx264` | 39.2 fps (load 11) | 47.4 fps (load 9) |

Three changes: NVENC now asks for NV12 (`HwAccel::upload_format`), so the
export converts on the GPU and reads back 1.5 bytes a pixel instead of RGBA
plus a CPU swscale (this is the bulk of it); the text animator above; and
`prefetch` decodes the frame's clips in parallel. NVENC zero-copy (CUDA frames
from a Vulkan export) is not attempted: it needs CUDA–Vulkan interop, and the
NV12 readback is ~1.2 ms a frame. Captions were not in the measured project.

## The benchmark suite

Everything measured below now comes from one binary, and it is the only thing
that should be quoted:

```bash
cd src-tauri
cargo run --release --bin chukcut-bench -- --all            # ~1 minute, six groups
cargo run --release --bin chukcut-bench -- --filter export  # one group, in isolation
cargo run --release --bin chukcut-bench -- --json runs/today.json
cargo run --release --bin chukcut-bench -- --compare runs/today.json
```

Source in `src-tauri/benches/`. It replaces the four separate harnesses that
produced the earlier figures (`examples/{decode_bench,export_smoke,
preview_jpeg_bench,hwdecode_pipeline}.rs`). Those examples are still worth
keeping — they print things a table cannot, like a DMA-BUF plane layout or a
PSNR against a reference encode — but the *numbers* belong to `chukcut-bench`,
because there they are all taken the same way.

One example is a deliberate exception, because it measures something a table of
absolutes cannot: `examples/preview_waste.rs` runs both arms of a change in one
process — the preview at the canvas against the preview at the panel, each
quality-ladder rung, and the two savings behind
`preview::server::experiment` — and prints the ratio. A ratio taken in one
minute on one machine survives a busy machine; a "before" taken from a build
that no longer exists does not survive at all.

Four properties of it are the point:

- **It refuses to report above a load average of 4.** `--force` overrides and
  stamps the output. This is not fastidiousness: see "A busy machine is not a
  slow machine, it is a different machine" under Traps.
- **It generates its own media** with ffmpeg into `target/bench-media/`, so it
  runs on any machine and nobody's numbers depend on a directory of footage that
  has since moved. Read the bitrates in the decode table before comparing
  anything to a figure taken on real footage.
- **Median, best and worst/best spread on every row.** A row whose spread is
  above ~1.5 is marked and should not be quoted.
- **`--json` and `--compare <file>`.** A baseline for this commit is checked in
  at `src-tauri/benches/baseline-4da5cff.json`; `--compare` against it prints a
  regression table. A change is only called a regression if it clears **both**
  ±20% and an absolute floor (0.3 ms, 2 fps, 0.5 µs) — a percentage alone turned
  a 0.18 ms wobble on a sub-millisecond row into "SLOWER, +21%", six times in the
  first self-comparison. Self-compared on a quiet machine it now reports 72 of 76
  rows as noise.

Measured 2026-07-26 at commit `4da5cff`, Raptor Lake iGPU (Vulkan), 12 threads,
FFmpeg 6.1.1, load average 2.45 before and 2.34 after, 58 seconds of measuring.
Every table in this section is that one run unless it says otherwise.

### One preview frame, which is the number that decides playback

Decode, upload, composite, read back, JPEG-encode — everything between "the
playhead moved" and "there are bytes for the webview", measured serially at the
project's own resolution.

| | 1920×1080 | 1080×1920 |
|---|---:|---:|
| Whole frame | **10.66 ms** | **11.37 ms** |
| of which sources (decode + to texture) | 1.11 ms | 1.17 ms |
| of which compositing | 0.41 ms | 0.43 ms |
| of which RGBA readback | 3.27 ms | 3.33 ms |
| of which JPEG (VAAPI) | ~5.9 ms | ~6.4 ms |
| Ceiling if nothing overlapped | 93.8 fps | 87.9 fps |
| Share of the budget at 24 fps | 26% | 27% |
| Share of the budget at 30 fps | **32%** | **34%** |
| Share of the budget at 60 fps | **64%** | **68%** |

**60 fps playback now fits, serially, at native resolution.** It did not before
the decode path was wired to the compositor: the same measurement at `fc15c71`
gave 24.99 ms and 40 fps, with `sources` at 13.04 ms rather than 1.11. That is
the single largest change this suite has recorded, and it is the zero-copy decode
work paying off end to end rather than in a microbenchmark.

Note what the frame is now made of: **the readback and the JPEG encode are 86% of
it**, and decoding is 10%. The preview's remaining cost is entirely "get the
finished frame to the webview", which is `docs/research/zero-copy-encode.md`
pointed at the preview.

The real frame is *cheaper* than this: `preview/server.rs` runs the JPEG encode
on a thread that overlaps the next frame's compositing. This number is one
frame's latency, which is what a seek pays with nothing to overlap. It excludes
the ring buffer, the pacing clock and the webview's own decode and paint — those
need a running GUI, which this binary deliberately does not.

### The preview renders the panel, not the canvas

Measured 2026-07-27 by `cargo run --release --example preview_waste`, which
prints every number below as an A/B **in one process** — same machine, same
minute, same clip — because a "before" taken from a build that no longer exists
is not a measurement. Load average 5.9, which is higher than the bench suite
would accept; the ratios below are stable across runs at load 4–24, the absolute
milliseconds are not.

The waste was structural and needed no profile to justify. A 1920×1080 project
in the default 700 px-wide player panel was composited, read back and
JPEG-encoded at **7.5× the pixels the screen can display**, and then thrown away
by a CSS downscale in the webview.

| Rendered at | Pixels | Whole frame | Ceiling |
|---|---:|---:|---:|
| 1920×1080, the canvas — what shipped | 100% | 11.68 ms | 86 fps |
| 1400×788, the panel on a 2× display | 53% | 8.11 ms | 123 fps |
| **700×394, the panel on a 1× display** | **13%** | **3.75 ms** | **267 fps** |

**3.1× the throughput for pixels nobody could see.** Not 7.5×, and the gap is
worth understanding: the decode is a fixed cost — the source file is 1920×1080
whatever the preview is — so only the upload, composite, readback and JPEG scale
with the target. That is also why `settings.preview_max_edge` measures the same
as no cap at all here (3.68 ms): the panel had already asked for less than the
cap would have allowed.

The quality ladder (`preview/ladder.rs`) is worth much less at the panel size
than at the canvas, for the same reason:

| | rung 0 | rung 1 (¾) | rung 2 (½) |
|---|---:|---:|---:|
| From the panel size (700×394) | 4.34 ms | 2.93 ms (1.5×) | 2.64 ms (1.6×) |
| From the canvas (1920×1080) | 13.52 ms | 9.71 ms (1.4×) | 5.52 ms (2.5×) |

So **the ladder is the second lever and not the first**: once the frame is the
size of the panel it is mostly decode, and no rung reduces decode. It earns its
keep at full-quality preview, in fullscreen, and on a 4K panel — which is
exactly when playback is in trouble.

Two smaller findings from the same binary, one of them negative:

- **Skipping the JPEG of a frame the playhead has already passed does almost
  nothing on this machine.** Driven with an injected clock at 1.5× the frame
  budget it fires **zero** times; at 8× it fires between 1 and 13 times per
  twenty steps, and which end of that range depends entirely on how busy the
  machine is (13 at load 16, 5 at load 4, 1 at load 6). The renderer here is
  fast enough that a finished frame is rarely more than one interval late, and
  one interval late
  is deliberately still encoded — `serve_uri` will hand it to a request for its
  neighbour. It is kept because it costs nothing when it does not fire and
  because `discarded` in the summary line answers "is the renderer finishing
  work that is already useless?" on a machine that is not this one. **Do not
  quote it as a speedup.**
- **A frame request that arrives while its frame is being rendered used to start
  a second render of it.** Four concurrent requests for one uncached frame after
  a seek: **2 composites of the same frame, now 1.** The webview asks again
  whenever the handler answers a miss, and a scrub job seeks the decoder — so
  the duplicate was the most expensive frame there is, paid twice, exactly when
  the preview was already slow.

Traps this left behind, both of which took a run to find:

- **Discarding late frames can freeze the picture.** A renderer consistently
  more than one frame behind finds *every* frame it finishes late by the same
  margin, discards all of them, and shows nothing while staying busy — measured
  at 1 frame encoded in 14. `render_one` therefore never discards two in a row.
- **An odd render size costs five times the encode.** NV12 cannot represent one,
  so `vaapi::size_is_encodable` refuses it and every frame falls back to
  libjpeg-turbo. Every size the panel or the ladder computes is rounded **down**
  to even. `session::proxy_size` still returns an odd canvas verbatim, which is
  a real if rare hole: a 1235×695 project previews on the software encoder.

### Decode, per frame

`sw →RGBA` and `hw →RGBA` both end in tightly packed RGBA in system memory;
`hw →DMA-BUF` stops at the GPU surface, which is what the compositor now takes.
Sequential walk, 48 frames, median of 3.

| per frame | sw →RGBA | hw →RGBA | hw →DMA-BUF | bitrate |
|---|---:|---:|---:|---:|
| H.264 1920×1080 | 11.28 ms | 11.68 ms | **1.34 ms** | 9.8 Mbit/s |
| H.264 1080×1920 | 11.42 ms | 11.75 ms | **1.37 ms** | 10.1 Mbit/s |
| HEVC 1920×1080 | 11.33 ms | 11.50 ms | **0.96 ms** | 6.7 Mbit/s |
| HEVC 1080×1920 | 12.14 ms | 11.07 ms | **0.88 ms** | 7.0 Mbit/s |
| VP9 1920×1080 | 12.08 ms | 11.45 ms | **0.99 ms** | 10.4 Mbit/s |
| VP9 1080×1920 | 12.42 ms | 11.45 ms | **0.95 ms** | 11.2 Mbit/s |
| AV1 1920×1080 | 9.17 ms | 12.11 ms | **1.05 ms** | 4.4 Mbit/s |
| AV1 1080×1920 | 9.17 ms | 11.73 ms | **0.92 ms** | 4.6 Mbit/s |

Two things to read out of it:

- **`hw →RGBA` is a wash against software**, within a millisecond either way in
  seven of eight cases, and *worse* for AV1. This is the finding the earlier
  sections describe, reproduced: the detiling download plus the swscale pass cost
  about what they save. Hardware decode is only worth having if the frame never
  comes back to the CPU.
- **`hw →DMA-BUF` is 9–13×** and barely varies with codec or aspect, because at
  that point the fixed-function block is the whole cost.

Random access, which nothing had measured and which backs up the "seeking
backwards during playback stalls" note above. Milliseconds **per seek**, 6 seeks
to deterministic positions, median of 2 rounds:

| per seek | sw →RGBA | hw →DMA-BUF |
|---|---:|---:|
| H.264 1920×1080 | 63.9 ms | 16.1 ms |
| H.264 1080×1920 | 64.2 ms | 15.9 ms |
| HEVC 1920×1080 | 122.3 ms | 16.1 ms |
| HEVC 1080×1920 | 123.6 ms | 16.1 ms |
| VP9 1920×1080 | 88.5 ms | 9.8 ms |
| VP9 1080×1920 | 88.3 ms | 11.0 ms |
| AV1 1920×1080 | 44.3 ms | 10.8 ms |
| AV1 1080×1920 | 45.7 ms | 10.9 ms |

**A seek costs 5–11 sequential frames**, and on HEVC it costs 10 whole frame
budgets at 30 fps. These clips have half-second GOPs, which is favourable; long-
GOP footage will be worse. The spread on these rows is 1.4–2.4× even on a quiet
machine — a seek's cost depends on where in the GOP it lands — so read them as
an order of magnitude, not as figures.

### Compositing, per frame, with no decoder in the loop

Every layer samples one texture uploaded before the clock starts, so this is
purely uniform writes, draw calls, blending and the readback. 1920×1080 canvas.
`GPU only` waits on the device but does not map the result.

| per frame | 1 layer | 3 layers | 10 layers |
|---|---:|---:|---:|
| Plain, with readback | 3.65 ms | 4.78 ms | 8.03 ms |
| Plain, GPU only | 1.12 ms | 1.94 ms | 5.09 ms |
| Transform + crop + keyframes, with readback | 4.13 ms | 5.92 ms | 9.19 ms |
| Transform + crop + keyframes, GPU only | 0.91 ms | 2.36 ms | 5.52 ms |

- **A layer costs about 0.5 ms** and the relationship is linear, so a ten-clip
  timeline composites in under 10 ms and layer count is not what will break
  playback.
- **Transform, crop and keyframes are free**, within noise, at every layer count.
  Five keyframes on three properties per segment per frame do not register. The
  CPU-side work in `render::layout` is not worth optimising.
- **The readback is a flat ~2.5–3.5 ms** whatever is being composited, and at one
  layer it is three quarters of the frame. Same conclusion as everywhere else in
  this document.

The transformed rows are deliberately built to cover the same canvas area as the
plain ones. An earlier version scaled layers to 55% and measured *faster* than
plain, because a compositor at 1080p is fill-rate bound.

### Preview JPEG encode, per frame

Nine interleaved runs at quality 88, median/best in milliseconds. Interleaved
rather than one encoder at a time, for the reason in `preview_jpeg_bench`: run
sequentially, whichever encoder went second paid for the heat the first made.

| | jpeg-encoder (was) | libjpeg-turbo | VAAPI (now) |
|---|---:|---:|---:|
| 540×960 | 6.35 / 6.26 | 2.13 / 2.10 | **1.53 / 1.46** |
| 1080×1920 | 25.03 / 24.88 | 8.24 / 8.20 | **4.75 / 4.41** |
| 1920×1080 | 24.75 / 24.65 | 8.28 / 8.19 | **4.94 / 4.62** |

Through `encode_preview_jpeg`, which is what the server actually calls, at
1080×1920: **5.95 ms median, 4.94 best**, dispatched to VAAPI.

The VAAPI breakdown at 1080×1920 is 1.06 ms of RGBA→NV12, 1.59 ms of upload and
2.12 ms of encoding — **the fixed-function encoder is a third of its own path**,
and the rest is carrying pixels to a chip that already had them.

These agree with the July 25 figures from `preview_jpeg_bench` to within this
machine's spread, which is the useful thing about having measured them twice with
different harnesses.

### Export, end to end, with audio

3 seconds of a two-clip timeline at 1920×1080 through `export::job::run_export`,
median of 2. Frames per second; higher is better.

| | fps | what the frame path does |
|---|---:|---|
| Software (`libx264`) | 30.5 | swscale on the CPU |
| VAAPI tier 1 (`--no-gpu-nv12`) | 62.8 | swscale on the CPU, then upload |
| VAAPI tier 2 (`--no-zero-copy`) | 98.8 | compute shader, read back, upload |
| VAAPI tier 3 (default) | **147.6** | compute shader, nothing crosses the bus |

**Hardware export is now 4.8× software, not the 1.5× recorded above**, because
the thing that limited it — decoding the sources — moved to the GPU as well. The
per-frame breakdown says so directly: `sources` is 1.7 ms where it used to be 12
to 76, and at tier 3 `prepare` is 0.0 ms and `submit` 1.0.

Cross-checked with `--filter export` on the same quiet machine: 35.9 / 46.6 /
75.7 / **128.4** fps. The tier *ordering* and the ratios reproduce; the absolute
figures move by up to 15% between runs, so quote them as "about 130–150 fps at
tier 3" and not to three figures.

### Project operations

A 500-segment document across four video tracks, 402 KB of JSON.

| | |
|---|---:|
| Save (serialize only) | 0.58 ms |
| Load (deserialize only) | 0.50 ms |
| Save + load through the disk — what an autosave costs | **1.37 ms** |
| Apply 1000 `SetTransform` edits | **2.67 µs** each |
| Undo the whole history | **0.79 µs** each |

The document layer is nowhere near being a problem: an autosave of a large
project costs less than a twentieth of one frame budget, and a thousand edits
cost 2.7 ms in total.

**But "undo them all" is not possible.** `timeline::history::MAX_DEPTH` is 500,
so of 1000 applied edits only the last 500 can be undone; the rest were dropped
off the bottom of the stack as they went. See Known defects.

## Hardware export, measured

**The whole-export rows here are history, kept because the reasoning below them
is still the reasoning.** The current figures come from `cargo run --release
--bin chukcut-bench -- --filter export` and are in "The benchmark suite" above:
software 30.5 fps against tier 3's 147.6, i.e. **4.8×, not the 1.5× this section
concluded**. What changed is not the encoder; it is that decoding moved to the
GPU too, which is exactly what the last paragraph of this section predicted would
have to happen.

Measured 2026-07-25 on the Raptor Lake iGPU, median of three runs, 240 frames
of real phone footage from `~/git/editing/footage`, through
`examples/export_smoke.rs`. `--encode-only` feeds the encoder synthetic frames
instead of composited ones, so the difference between the two rows is
everything upstream of the encoder. The encoder-only rows are the one thing here
`chukcut-bench` does not reproduce — it measures whole exports — so they stay.

| | 1920×1080 | 1080×1920 |
|---|---|---|
| Whole export, software (`libx264` medium) — superseded | 16.1 fps | 14.2 fps |
| Whole export, VAAPI (`h264_vaapi`) — superseded | 24.2 fps | 21.3 fps |
| Encoder only, software | 16.5 fps | 20.1 fps |
| Encoder only, VAAPI H.264 | **80.6 fps** | **63.4 fps** |
| Encoder only, VAAPI H.265 | 60.8 fps | 51.9 fps |

Run-to-run spread on this machine is wide — software at 1080×1920 was measured
anywhere from 11.3 to 16.9 fps across a session, apparently with thermal state —
so single runs are not worth comparing and a change of less than about 20%
cannot be read off one. Use `--clip=8` or longer and take a median.

Read those two pairs together, because the story is in the gap:

- **The encoder itself got 3–5× faster.** That part of the decision document
  held up.
- **The whole export got 1.5× faster**, at both aspect ratios, and the number is
  suspiciously identical because the same thing limits both.
- What limits it is **everything the encoder is not doing**: decoding the
  sources, compositing, reading the frame back off the GPU, and converting RGBA
  to NV12 on the CPU. At 1080p the hardware export spends about 41 ms per frame
  and the encoder accounts for roughly 12 of them.
- The software comparison also flatters itself: **x264 encodes on eight threads
  in the background**, so a large part of its cost already overlaps with
  decoding and compositing. VAAPI has less to overlap.

So the next win is not a faster encoder. It is, in order: not reading the frame
back to the CPU (`docs/research/zero-copy-encode.md`), doing RGBA→NV12 on the
GPU, and hardware *decode*. The last of those was ranked third in the decision
document and the numbers above argue it should be higher.

**All three are now done**, and the last of them was indeed the biggest: see
"The export's frame path, measured stage by stage" and "Hardware decode through
the compositor" below. The export table above is the state before any of them
landed and is kept because the *gap* it documents is the argument.

## Hardware decode, measured

**Superseded by `cargo run --release --bin chukcut-bench -- --filter decode`**,
whose table in "The benchmark suite" above covers four codecs rather than two,
both aspects, and random access as well as sequential. Kept because the
explanation under it is the important part and is unchanged.

One difference matters when comparing the two tables: these figures are real
phone footage at ~1.5 Mbit/s and `chukcut-bench` generates clips at 4–11 Mbit/s.
That is not a detail — see "A decode number without a bitrate next to it is not a
number" under Traps.

Measured 2026-07-26, `examples/decode_bench.rs`, median of three runs of 90
sequential frames of real footage, first frame discarded. Full working in
`docs/research/hardware-decode.md`.

| per frame | 1920×1080 H.264 | 1080×1920 H.264 | 1920×1080 VP9 | 1080×1920 VP9 |
|---|---:|---:|---:|---:|
| Software decode → RGBA | 26.9 ms | 31.7 ms | 15.1 ms | 38.4 ms |
| Hardware decode → RGBA | 37.3 ms | 26.1 ms | 18.8 ms | 54.2 ms |
| Hardware decode → DMA-BUF | **1.70 ms** | **2.09 ms** | **1.71 ms** | **2.37 ms** |

The middle row is the surprise and it is the point of the whole exercise:
**hardware decode is slower than software when the frame has to end up in system
memory**, in three of these four cases. A VA surface is `Y_TILED`, so
`av_hwframe_transfer_data` is a detiling pass over the whole frame, and swscale
still has to turn NV12 into RGBA afterwards. Together they cost more than the
decode they were supposed to accelerate — the decode itself is under 2.5 ms.

The bottom row is what the frame costs when nothing copies it. That is 9–16×,
and it is the number the preview and the export were both waiting on. **It is
reachable now** — the next section is the whole pipeline measured the same way.

On an integrated GPU the lesson generalises and is worth carrying into the next
piece of work: **"move it to the GPU" is not the optimisation. "Stop copying it"
is.**

## Hardware decode through the compositor, measured

Measured 2026-07-26, `cargo run --release --example hwdecode_pipeline -- --dir
target/bench-media`. Both decoders are measured **in one process, back to back,
per file**, because run-to-run spread on this machine is wider than most of what
is being compared. Median of three runs of 90 sequential frames, first frame
discarded.

`to texture` is one `SourceProvider::frame` call — the decode plus whatever it
takes to reach something the compositor can bind, which is an 8 MB upload on the
software path and two `texture_from_dmabuf_fd` calls on the hardware one.
`whole frame` is that plus compositing and the RGBA readback: everything the
preview does short of the JPEG encode.

| per frame | sw, to texture | sw, whole frame | hw, to texture | hw, whole frame |
|---|---:|---:|---:|---:|
| 1920×1080 H.264 | 65.7 ms | 71.9 ms | **2.19 ms** | **9.39 ms** |
| 1080×1920 H.264 | 58.8 ms | 67.6 ms | **2.74 ms** | **10.78 ms** |
| 1920×1080 VP9 | 33.8 ms | 43.1 ms | **1.70 ms** | **10.20 ms** |
| 1080×1920 VP9 | 31.8 ms | 47.2 ms | **1.94 ms** | **10.16 ms** |
| 1920×1080 HEVC | 43.3 ms | 45.4 ms | **1.21 ms** | **7.49 ms** |
| 1080×1920 HEVC | 44.0 ms | 65.3 ms | **1.63 ms** | **10.95 ms** |
| 1920×1080 AV1 | 34.2 ms | 39.3 ms | **0.99 ms** | **10.22 ms** |
| 1080×1920 AV1 | 34.3 ms | 41.3 ms | **0.82 ms** | **9.67 ms** |

**Other agents were compiling throughout, so the absolute numbers are high** —
the software column is roughly twice the quiet-machine figures in the section
above. The two columns moved together, which is what interleaving them was for.

Read it as three claims:

- **Decode to texture is 15–40× cheaper.** That is the `hw → DMA-BUF` row of the
  previous table finally reaching the compositor, and it is slightly *better*
  than that row because the software path it replaces also included
  `upload_rgba`.
- **A whole preview frame is 4–8× cheaper**, and now fits the 33.3 ms budget with
  room to spare at native resolution — 7.5 to 16 ms against 39 to 72.
- **What is left in the frame is the readback**, 5.6–14.2 ms of it, against
  0.7–1.6 ms of compositing. Decoding is no longer the largest term in the
  preview; copying the finished frame back to the CPU for the JPEG encoder is.
  That is `docs/research/zero-copy-encode.md` pointing at the preview instead of
  the export.

What is still on the CPU on this path, in descending order: the readback above,
the JPEG encode that follows it (6.2 ms at 1080p, hardware), demuxing
(`av_read_frame` — headers and I/O, not pixels), and the `vaSyncSurface` inside
`av_hwframe_map`, which is a wait rather than a copy. Thumbnails and waveforms
are deliberately still software; the reasons are in
`docs/research/hardware-decode.md`.

**One behaviour changed and it is worth knowing about.** The software path asks
swscale to downscale during colour conversion, so a preview at 540×960 decodes a
1080p clip to 540×960. A VA surface comes back at full resolution whatever was
asked for, so the hardware path hands the compositor a full-resolution texture
and lets the sampler shrink it. That is free on the GPU and it is why the
hardware column barely varies with aspect ratio — but it means
`provider::fitted_height` now only governs the software path, and a timeline of
4K clips will hold 4K surfaces rather than proxy-sized ones. Proxy media is the
answer to that, not a VPP downscale.

`media::provider::DEFAULT_ACCELERATION` is now `Acceleration::Auto`, **gated on
`RenderContext::can_import_dmabuf()`**. Without that gate a machine on the GL
fallback or an old driver would get the middle row of the previous table, which
is worse than software; with it, such a machine stays on software.
`CHUKCUT_DECODE=software|auto|vaapi` still overrides both, and
`MediaSourceProvider::from_project_with` forces the choice per provider so the
two paths can be compared inside one process.

### Verified by pixels

`cargo run --release --example hwdecode_pipeline -- --dir target/bench-media
--verify`. A hardware-decoded frame composited through the real compositor,
against a software-decoded composite of the same instant, mean absolute
difference over R, G and B:

| | hardware vs software | control: wrong matrix | control: one frame apart |
|---|---:|---:|---:|
| H.264, HEVC, VP9, AV1, both aspects | **1.10 – 1.38** | 9.30 – 9.57 | 2.51 – 4.11 |

`tests/compositor.rs` is the other half of this and is the stronger check,
because it asserts on named colours rather than on agreement: seventeen tests
render real video files through `MediaSourceProvider` and the compositor and
check pixels at known coordinates — letterboxing, crop, opacity in linear light,
keyframe ramps, track order, rotation. All seventeen now run on the imported
hardware path and pass unchanged, so the two-plane case is not merely *similar*
to the copying one, it is right about colour in absolute terms.

The bar established by `tests/decode.rs` is a mean channel difference under 2,
and every codec and aspect passes. The two controls are what make that mean
something: forcing BT.601 onto the same surface scores 9.4, so the matrix really
is being read from the file rather than agreeing by luck, and two adjacent
software frames differ by 2.5–4.1, so the metric can see a difference this size.
The residual 1.2 is chroma upsampling — the shader interpolates the half-size
chroma texture with the sampler and swscale does its own thing — plus the 8-bit
round trip through the sRGB render target.

### The colour bug this found, which was in the *software* path

**`sws_getContext` does not read a frame's colour tags.** It initialises with
`SWS_CS_DEFAULT`, which is BT.601, whatever the file declares — so every BT.709
clip in this editor was being converted with BT.601 coefficients. It is a
consistent tint of up to ten code values on saturated reds and greens, not an
obvious fault, which is exactly why it survived: `render_smoke`'s comparison
against ffmpeg's own frame absorbed it as rounding.

It surfaced from the other side. The compositor's new path takes the matrix from
`MappedFrame::color_space`, i.e. from the file, so a correct hardware frame and
an incorrect software one composited 9.6 code values apart — and forcing the
*wrong* matrix on the hardware path brought them back together, which is a
diagnosis rather than a coincidence.

`decoder.rs::apply_colour` now calls `sws_setColorspaceDetails` with the file's
matrix and range. The numbers above are after that fix; before it, hardware
against software scored 9.6 and the wrong-matrix control scored 1.4 — the table
inverted.

## Proxy media, measured

`src-tauri/src/modules/proxy/`, decision 0003. Measured 2026-07-26 on the
Raptor Lake machine. **The machine was at load ~29 while these ran**, so every
absolute figure is roughly twice what a quiet machine gives — a quiet run of the
first row measured 40.9 ms. The ratios are what the numbers are for.

Decode of one frame to RGBA at preview size, single-threaded, which is what
`media::provider` pays (`ffmpeg -threads 1 -i F -vf scale=-2:1080,format=rgba -f
null -`, 240 frames):

| | per frame |
|---|---:|
| 4K H.264 source → 1080-tall RGBA | 84–98 ms |
| 4K HEVC source → 1080-tall RGBA | 116–136 ms |
| **720p all-intra proxy → RGBA** (`libx264 -tune fastdecode`) | **7.9–12.4 ms** |
| 720p all-intra proxy → RGBA (`h264_vaapi`) | 15.3–18.8 ms |

So a proxy is worth **8–11×** against 4K H.264 and **10–15×** against 4K HEVC,
which is the difference between a timeline that plays and one that does not.

Three things there that are not obvious and are worth not rediscovering:

- **How the proxy was encoded changes how expensive it is to decode, by 50%.**
  Same content, same 1280×720, four encoders: `libx264 -crf 23 -g 1` 18.1 ms,
  the same plus `-tune fastdecode` 12.4–15.8 ms, `h264_vaapi -qp 23 -g 1`
  23.0–26.5 ms, and the same plus `-coder cavlc` 18.1–19.5 ms. `fastdecode` and
  `cavlc` are the same trade — CABAC and the deblocking filter off — and both
  are now set. The hardware encoder's *default* output was the most expensive
  proxy of the four.
- **Hardware encoding buys almost nothing here.** The whole 4K→720p transcode
  took 11.33 s in software and 10.40 s on VAAPI: **8%**, because decoding the 4K
  source and scaling it dominate. Same conclusion as the export, one section up.
  The hardware path is still the default because it is what the user waits on,
  and `CHUKCUT_PROXY_ENCODER=software` takes the other side without a rebuild.
- **FFmpeg guesses the muxer from the output filename**, so an atomic write to
  `proxy.mp4.part` fails inside `format::output` with a bare `EINVAL` and no
  hint. The partial file is `.partial-proxy.mp4` — a prefix, not a suffix.
- **A synthetic fixture painted with per-pixel noise proves the opposite of
  what it looks like it proves.** `x ^ y` content does not compress at any
  resolution, so the 720p all-intra proxy came out *three times larger* than its
  4K source and only 2.9× cheaper to decode instead of 8–11×. Real footage is
  locally smooth with a few hard edges; `proxy/tests.rs::paint` now paints that,
  and the test that compares decode costs interleaves its two measurements
  because the load on this machine can double between them.

## The export's frame path, measured stage by stage

Measured 2026-07-25/26 on the Raptor Lake iGPU, 1920×1080, `h264_vaapi`, 480
frames of `dont_regret_60fps.mp4` through `examples/export_smoke --clip=8`.
Every run prints a `BREAKDOWN` and an `ENCODESIDE` line; these come from
`render::RenderStats` and `export::WriterStats`, which are always on.

The export now runs at whichever of three tiers the machine reaches, each a
strict improvement on the one below, each falling back automatically:

| | tier 1 `--no-gpu-nv12` | tier 2 `--no-zero-copy` | tier 3 (default) |
|---|---|---|---|
| Colour conversion | swscale, CPU | compute shader | compute shader |
| What crosses the bus | 8 MB RGBA back, 3 MB NV12 up | 3 MB NV12 back, 3 MB up | **nothing** |
| Readback (`readback`/`nv12`) | 5.1 ms | 3.0 ms | 1.5 ms |
| Frame prep (`prepare`) | 14.8 ms | 0.8 ms | **0.05 ms** |
| Encoder submit (`submit`) | 3.9 ms | 3.5 ms | **0.4 ms** |
| Sum of those three | 23.8 ms | 7.4 ms | **2.0 ms** |
| Whole export | 24.0 fps | 41.4 fps | **77.3 fps** |

Two cautions on that table. The whole-export row is the noisiest thing here —
the decode stage (`sources`) was measured anywhere from 12.8 to 76 ms per frame
across a session depending on what else the machine was doing, and it dominates
now. The three *stage* columns are ratios measured inside a single run and are
stable; the fps column is one quiet triple and should not be quoted to three
figures. Take medians, and only compare runs taken back to back.

Which tier a *user's* export took is no longer a guess: every export logs it,
with the reason the tier above was not reached and the strides in use. See
"The log file, and what an export writes into it".

### The isolated readback, which nobody had measured

`Compositor::render_frame`'s map-and-copy costs **4.4–5.7 ms per 1080p frame**
on a quiet machine, median ≈ 5.1 ms, split roughly evenly:

- **2.1–2.6 ms blocked in `device.poll`**, waiting for the GPU to finish
  compositing and copying. This is a pipeline stall, not bandwidth.
- **2.1–2.9 ms** copying the mapped rows into a `Vec` and stripping the
  256-byte row padding. Pure CPU memory bandwidth, 8 MB a frame.

Against the 39 ms frame the export used to cost, the readback was 13% — so
`zero-copy-encode.md`'s worry that it might be "4 ms of a 15 ms frame" was
close on the absolute number and wrong about the ceiling, because the thing
next to it was much larger than assumed. **The RGBA→NV12 swscale pass was 14.8
ms**, not the ~9 ms the research document estimated for steps ② to ④, and it
was the single largest item in the export after decoding.

### What each step bought

- **RGBA→NV12 on the GPU (tier 2)** is the big one: 1.7× on the whole export,
  and it deletes 14 of the 15 ms of frame preparation outright. It needs no
  DMA-BUF, no `ash`, and no driver cooperation — a compute pass and a storage
  buffer.
- **Zero-copy (tier 3)** is another 1.8×. Most of it is not the readback: it is
  `submit` falling from 3.5 ms to 0.4, because `av_hwframe_transfer_data` was
  most of what "submitting a frame to the encoder" cost.

### Verified by pixels

Against a `libx264` export of the same timeline, decoded at the same timestamps:

| | luma | Cb | Cr |
|---|---|---|---|
| Tier 1, the old path | 52.04 dB | 55.64 | 57.06 |
| Tier 2, GPU NV12 | 52.00 dB | 55.47 | 57.14 |
| Tier 3, zero-copy | 52.00 dB | 52.73 | 51.45 |

And against each other: tier 2 vs tier 1 is 55.1 dB on luma — the compute
shader and swscale disagree by a fraction of a code value. Tier 3 vs tier 2 is
**infinite on luma**: the encoder received bit-identical luma whether the
surface was uploaded or imported. Chroma differs by at most 4 code values on 6
to 10% of samples, which is the VAAPI encoder making marginally different
decisions on a linear imported surface; the DMA-BUF import itself is proven
byte-exact by `hwframes.rs`'s round-trip test.

The controls that say those numbers mean something: a deliberately
chroma-swapped tier-3 output scores 40.9/41.3 dB on U and V, and comparing
frames one apart scores 40.5 dB on luma. So the new paths are not green, not
plane-swapped and not offset by a frame.

**Do not read a quality verdict into these numbers.** x264's CRF and VAAPI's QP
are not the same scale, so "the same quality setting" produces different files:
on the same eight-second timeline the hardware output was 17% larger at
1080×1920 and 5% smaller at 1920×1080. Neither figure means much on its own —
comparing the two properly means a rate-distortion sweep, which nobody has done
here. This is the reason software stays the default: its result is predictable
across machines and the hardware's is not.

The pictures themselves match, though, which is the thing that had to be
checked: decoding both files at the same timestamps gives 51–53 dB PSNR on
luma and 60–62 dB on chroma. For calibration, a deliberately chroma-swapped
control scores 42.6 dB on U and V, and comparing frames one apart scores
34–40 dB on luma. So the hardware path is not green, not chroma-swapped and not
offset by a frame — which are the three ways it usually goes subtly wrong.

## Hardware preview JPEG, measured

Measured 2026-07-25 on the Raptor Lake iGPU. Every preview frame is served to
the webview as a JPEG, so this cost is a direct term in the 33.3 ms playback
budget — and at native resolution it used to be most of it.

**The numbers now come from `cargo run --release --bin chukcut-bench --
--filter preview-encode`** and are tabulated in "The benchmark suite" above.
Kept here for the analysis that follows, which is unchanged. Full detail and the
traps are in `docs/research/vaapi-jpeg-preview.md`.

The figures below are the original 2026-07-25 run of
`examples/preview_jpeg_bench` — median/best of 21 interleaved runs at load
average 3, quality 88, milliseconds per frame. They agree with the
`chukcut-bench` run to within this machine's spread, which is worth knowing:
two harnesses written months apart, measuring the same code, landed in the same
place.

| | jpeg-encoder (was) | libjpeg-turbo | VAAPI (now) |
|---|---|---|---|
| 540x960 | 10.9 / 6.5 | 4.6 / 2.2 | **2.3 / 1.4** |
| 1080x1920 | 31.1 / 26.7 | 10.2 / 8.5 | **6.2 / 4.7** |
| 1920x1080 | 31.4 / 26.2 | 11.2 / 8.8 | **6.1 / 4.7** |

Two independent steps, and it is worth keeping them apart:

- **The crate swap was 3.1x and cost nothing.** `jpeg-encoder` is portable Rust
  with no SIMD; libjpeg-turbo is nothing but SIMD. If the hardware path ever has
  to be turned off, this is what it falls back to and it is still three times the
  old speed.
- **The hardware was a further 1.6x**, for 5.0x end to end.

**Read the 1.6x with the breakdown, because it is the useful part.** Per
1080x1920 frame the hardware path is 1.6 ms of RGBA-to-NV12, 2.2 ms of upload
into a VA surface, and 2.0 ms of actual encoding. The fixed-function encoder is
**a third** of its own path; the rest is carrying pixels to a chip that already
had them — the compositor renders into a GPU texture, and the preview reads
8 MB back to system memory only to send 3 MB straight back.

So, as with the export, the next win is not a faster encoder. It is
`render::nv12` (colour conversion in a compute shader, which also halves the
readback) and then DMA-BUF export, which is `docs/research/zero-copy-encode.md`.

**That was done on 2026-07-27, and the direction that works is the opposite of
the one everything above assumed.**

The obvious route — we allocate the NV12 buffer, describe it as
`DRM_FORMAT_MOD_LINEAR`, the driver imports it — is what the export does with
`h264_vaapi` and it measured 2.4–3.1× here too. It produces a mosaic, because
Intel's fixed-function **JPEG** engine reads such a surface as though it were
32-row tiled while the **video** engine reads the same file descriptor in the same
process correctly. Declaring a different modifier cannot help: iHD **ignores**
`VASurfaceAttribDRMFormatModifiers` and returns `I915_FORMAT_MOD_Y_TILED`
whatever is asked for, on every entrypoint.

The route that works turns it round. **VAAPI allocates the surface,
`vaExportSurfaceHandle` exports it, Vulkan imports the two planes as colour
attachments through `VK_EXT_image_drm_format_modifier`, and the compositor draws
NV12 into them** — the same trick `render::dmabuf::import_plane` has been doing
for decoded frames since July 26, pointed the other way. Nothing on our side
declares a layout because the layout is not ours. Measured, quiet machine:
**11.93 → 4.36 ms** at 1920×1080 and **11.41 → 3.98 ms** at 1080×1920, and right
per pixel at 1440, 1360, 700 and 394 — 41.8 to 47.5 dB against libjpeg-turbo,
against the 37 dB the *copying* hardware path scores.

The evidence, the driver capability tables, the row-ramp probe, what was ruled
out and what is still unknown are in
**`docs/research/preview-zerocopy-jpeg.md`**. Reproduce in thirty seconds with
`cargo run --release --example preview_zerocopy`.

Four things about it that will otherwise cost an afternoon:

- **A PSNR cannot diagnose this and a row ramp can.** Tiling, an ignored pitch
  and a range mismatch all score badly and want three different responses. A
  picture whose luma *is* its row number tells them apart at a glance: tiling
  collapses each 32 rows to their mean (row 0 comes back 15), an ignored pitch
  shears progressively, a range mismatch puts black at 16.
- **Ask the driver, in C, before theorising.** Thirty lines against
  `libva`/`libva-drm` produced the surface-attribute list, the modifier iHD
  actually hands back, and its plane pitches at six sizes, in ten minutes. The
  same questions asked through Rust and FFmpeg would have taken a day and
  answered less.
- **WebGPU has no storage-writable `R8Unorm` — and does not need to.** That fact
  is why `render::nv12`'s compute pass writes a buffer, and it was also why this
  looked expensive. Both `R8Unorm` and `Rg8Unorm` are ordinary *colour
  attachments*, so `shaders/nv12_planes.wgsl` draws the planes instead, one
  full-screen triangle each, and measures the same as the compute version.
- **The decision to use the path is taken before the frame is composited, not
  after.** The destination is device-local on both arms, so once a frame has been
  drawn into one there is nothing on the CPU that can read it and no software
  fallback left. `preview::vasurface::encoder_reads_its_own_surface` and
  `preview::zerocopy::encoder_can_read_linear` therefore each encode a known ramp
  once per process and only enable their path if the picture comes back
  right — they prove the encoder rather than hoping, which is the answer to
  `zero-copy-encode.md`'s own warning that a "zero-copy" path which quietly
  copies is worse than none.

**The reduced playback resolution is gone.** `PreviewSession` briefly carried
two sizes — native for a parked frame, reduced for playback — purely because the
JPEG encode overran the budget at 1080x1920. At 6.2 ms it does not, and the
encode runs on a thread that overlaps compositing besides, so playback renders
at the project's own resolution again. What is left in the frame budget is
compositing, the readback, and decoding.

Two things about the hardware path that are not obvious and cost time to find:

- **swscale cannot do RGBA to NV12 usefully.** It was the first implementation
  and measured 37 ms per 1080x1920 frame — more than the entire frame budget,
  and three times what libjpeg-turbo needs for a whole JPEG. There is no SIMD
  path for that format pair. `preview::vaapi::rgba_to_nv12` replaces it and is
  about twenty times faster.
- **JPEG is full range and every YUV routine defaults to limited.** There is no
  range field in a JPEG file, so decoders assume 0-255, but swscale and friends
  produce 16-235 by default. The result is a valid file with grey blacks that
  reads as the editor having washed out the footage. It scores 27 dB against the
  software encoder instead of 37.

## Known defects, found but not yet fixed

Found by the integration suite. Each is real and reproducible; none has a red
test left behind, so `cargo test` is green despite them.

- **Undo cannot reach the start of a session.** `timeline::history::MAX_DEPTH` is
  500 and `History::apply` drops the oldest command off the bottom of the stack
  when it is exceeded, so after 1000 edits only the last 500 can be undone and the
  earlier ones are gone with no indication that they ever existed. Measured by
  `chukcut-bench --filter project`, which applies 1000 edits and reports how many
  the history still held. Half an hour of editing followed by "undo back to where
  I started" stops silently in the middle. Either the limit should be far higher —
  the commands are small, 1000 of them cost 2.7 ms in total to apply, so depth is
  not what makes this expensive — or the UI has to say that history has been
  truncated. Silently is the one option that is wrong.
- **`tests/decode.rs::decoding_sequentially_is_dramatically_cheaper_than_seeking_to_each_frame`
  is a timing assertion and it flakes on a busy machine.** It asserts a *ratio*
  between sequential and random-access decoding and fails with "sequential decode
  took 78 ms and random access 153 ms; sequential reads appear to be seeking".
  Reproduced **1 run in 12** on `71f11cf` with nothing else changed, and 0 in 32
  on the same machine minutes later. The property it is testing is real and worth
  testing; the threshold is not survivable under load. It wants either the
  `chukcut-bench` load guard or a much wider margin. **Do not spend time chasing
  it after an unrelated change** — check whether it reproduces on the tip with
  the change stashed first.
The preview hang that used to be listed here is **fixed**, and it was not the
shared device. It was a rayon reentrancy deadlock in the hardware JPEG path;
the account is under "The hang that was not the device" below, and hardware
JPEG is on by default again as a result.

## The document layer's defects, fixed

The five defects this section used to list are fixed, each with a test that
fails without the fix. What is worth carrying forward is *why* they were
invisible, because the same blind spots will produce the next ones.

- **A non-finite float is silent data loss, and only the save path can see
  it.** `serde_json` cannot write a NaN or an infinity — it writes `null` — so
  a project that took one saved successfully and then failed to load forever
  after with "invalid type: null, expected f32". Nothing in memory is wrong,
  which is why every in-memory assertion passed. Non-finite values are now
  refused at the edit-command boundary (`timeline/ops.rs::check_finite`), are
  errors in `validate()`, and `project/migrate.rs::repair_non_finite` opens the
  files that already have them: a `null` at a key is dropped so the field's
  documented default applies — an opacity comes back at 1.0, not 0.0 — and the
  user is told which values were reset. **Every float in the document now
  carries a `serde` default**, which is what makes that repair possible.
- **`schema_version` now gates loading.** `project/migrate.rs` is the only
  place allowed to turn bytes into a `Project`. A newer format is refused with
  a sentence naming both versions and what to do; an older one walks the (still
  empty) `STEPS` ladder. Adding version 2 is a bump and one table entry.
- **`SetSpeed` moves the source range with the speed.** The invariant
  `source_range.duration = target_range.duration × speed` is what `split_at`,
  the mixer and the exporter all read; the command used to change only the
  factor, so a sped-up clip cut in the wrong place. The clip keeps its position
  and length on the timeline and its source range changes, which is the
  direction the rest of the codebase already assumed (`audio/mixer.rs`,
  `export/audio.rs` both derive the source span from the speed). Every command
  that writes a source duration now writes
  `document::source_duration_for(target, speed)` — one function, so undo stays
  byte-exact instead of drifting a microsecond per edit.
- **`validate()` covers what the fuzzer would have found**: negative
  `target_range.start`, source-range sanity, duplicate segment *and* track ids,
  non-finite values, the speed invariant, and keyframe tracks (sorted, unique
  times, no empty track). `edit_fuzz.rs` asserts those invariants
  independently of `validate()` after every one of its thousands of commands,
  because a fuzzer that only asks the checker whether the checker is happy
  tests nothing when the checker stops looking.
- **`RemoveTrack` checks the lane it names is at the index it names.** A stale
  index used to delete a different lane and report success. `AddTrack` now
  refuses an out-of-range index for the same reason rather than clamping: a
  clamped insert produces a delete that cannot undo it.
- **Autosave exists.** `project/autosave.rs` writes the open document to
  `workspace::paths::autosave_file()` after every edit, import, save and undo,
  and `project_get` restores it on the first call after launch. It is not an
  undo step, and it is not the user's file — the path they last saved to is
  remembered in a sibling `autosave.path` so a restored session still knows
  where Ctrl+S goes. Writes go to one background thread through a single-slot
  mailbox, so a burst of edits costs one write, the newest wins, and no edit
  waits on the disk.

The same pass added the four keyframe commands the editing UI sends —
`AddKeyframe`, `RemoveKeyframe`, `MoveKeyframe`, `SetKeyframeEasing`. Two
things about them are load-bearing and not obvious:

- **A `KeyframeTrack` exists exactly while the property is animated.** The last
  keyframe of a property takes the track with it, because both sides read "is
  animated" as "a track exists". That makes undo harder, not easier, which is
  why `AnimatableProperty` is now `Ord` and a segment's tracks are kept in its
  declaration order: re-creating a dropped track has to put it back where it
  was, or undo rewrites the file.
- **A keyframe past the end of its clip is legal.** Trimming a tail produces
  them, the document keeps them so that undoing the trim brings the animation
  back, and refusing them would make the undo of an ordinary delete fail. The
  only positional rule is that a time is not negative; `validate()` reports the
  rest as a warning.

### What happened when the fuzzer was pointed at transitions

`tests/edit_fuzz.rs::transitions_survive_the_edits_that_move_the_clips_they_join`
places transitions on real cuts and then lets the ordinary generator move, trim,
split and delete the clips they join. It found two defects within two hundred
random edits, and both are now fixed:

- **An ordinary clip move orphaned a transition.** A transition is centred on a
  cut, the cut is "the previous clip ends exactly where this one starts", and
  moving either clip destroyed it — after which `validate()` correctly called the
  document inconsistent, over an edit the user was entitled to make. The
  primitive cannot fix itself (`MoveSegment` does not carry the transition, so
  it could not put one back on undo), so `ops::detach_broken_transitions` wraps
  the incoming command into a `Composite` with the removals it implies. One undo
  step, and undoing it brings the transition back. `History::apply` calls it, so
  every path into the document is covered. `transitions::edit::detach_around`
  was written for this and nothing had called it.
  - The subtlety that cost an hour: only a transition that is **still attached**
    to a segment and has lost its cut may be detached this way. One whose
    segment went with the edit — a deleted clip, a deleted lane — is already off
    the timeline, and the removal command carries a *snapshot* of that segment.
    Prepending a detachment there makes undo restore the transition twice, and
    the id appears in `extras` twice.
- **`AddTransition` was not the exact inverse of `RemoveTransition`.** `remove`
  used `retain` and `add` used `push`, so removing the first of two transitions
  and undoing it put it back second, rewriting the file on an undo. The pool is
  keyed by id and nothing reads it positionally, so `add` now inserts in id
  order — the same fix, for the same reason, as the keyframe-track ordering
  above.

Two transition-related holes the fuzzer did **not** close, recorded because they
are one line of thought away from the above:

- **`split_at` clones `extras`,** so splitting a clip that carries a transition
  leaves the id on both halves. Neither half is unjoined, so nothing flags it.
- **`extras` order is not restored** by remove-then-add when a segment carries an
  effect *and* a transition: `remove` retains and `add` pushes, so the
  transition ends up last. The list is documented as being in application order,
  so the fix is for the command to carry the index, as `RemoveSegment` does.

Two things found along the way that were worse than the list said:

- **Trimming was as capable of breaking the speed invariant as `SetSpeed`
  was**, and the fuzzer generates trims eighteen times more often. A trim that
  scales one range and not the other is now refused with a message naming the
  arithmetic, and the frontend already computes it correctly
  (`Timeline.tsx` multiplies the drag delta by the speed), so nothing that
  works today starts failing.
- **`split_at` truncated where it should have rounded**, so cutting a clip at
  an odd speed lost up to a microsecond of source at every cut, compounding
  with each split.

## Text rasterisation, measured

Measured 2026-07-26, `cargo run --release --example text_bench`. The engine is
`src-tauri/src/modules/text/`: parley + fontique + harfrust + skrifa for
shaping, zeno for scan conversion, everything above that ours. Full working, the
traps, and the things it cannot do are in `docs/research/text-rendering.md`.

**The machine was at load 40 on 12 cores while several agents built**, and the
scheduler dominates: the same case measured 4.7 ms and 15.1 ms minutes apart.
The benchmark reports the **minimum of 25 runs**, which is the closest thing to
an uncontended number available here. Treat these as an upper bound and
re-measure on a quiet machine before quoting them.

| Milliseconds, minimum of 25 | 1920×1080 | 1080×1920 |
|---|---:|---:|
| Short title, 26 glyphs | **5.8** | 6.5 |
| Short title + 4 px outline | 19.9 | 20.9 |
| Short title + outline + shadow + background box | 29.3 | 32.7 |
| Paragraph, 196 glyphs | 39.1 | 25.9 |
| **Any of them, warm cache** | **0.0005** | 0.0005 |

Three things worth carrying forward:

- **Shaping is 0.1 ms and is not the problem.** All of parley, harfrust and
  fontique — bidi, ligatures, Arabic joining, CJK line breaking — costs 0.1 ms
  for the title and 0.2 ms for the 196-glyph paragraph, at every size. Every
  other millisecond above is our own rasterisation.
- **The cache is the whole story for playback.** A title does not change between
  frames, so a warm frame costs a hash and an `Arc` clone. The cold cost is paid
  once per edit, and 30 ms of that is not felt in an editor.
- **The outline costs four times the text.** Scan-converting the stroke is what
  an outlined title spends its time on, and both obvious levers were the wrong
  way round: `kurbo::stroke` — which `rust-crate-survey.md` §4 recommends — was
  **1.8× slower** than letting zeno stroke during scan conversion, and
  `Join::Miter` was **4× slower** than `Join::Round` (54–62 ms against 15 ms).
  The next idea is to dilate the fill mask rather than to stroke faster.

## Titles, end to end

Added 2026-07-26. `modules/text/` already turned a `TextMaterial` into pixels;
what did not exist was any way for a user to make one. Now:

- **`Text` tab in the media library** — one button plus four word presets.
  `text_add` mints the material, chooses the lane and applies an
  `InsertSegment` through the history, so a title appears at the playhead,
  selected, with a placeholder in it and an outline already on.
- **The lane is a `TrackKind::Text` one, appended to the track list.**
  `ops::reindex` derives `render_index` from track order and lower tracks paint
  first, so appending is what puts the title *on top* of the picture. A second
  title reuses the lane and slides to the first free instant rather than being
  refused — a button press aims at "now", not at a microsecond.
- **Inspector panel** (`src/modules/inspector/components/TextInspector.tsx`) —
  multi-line content, the machine's own font list, size, colour with alpha,
  bold, italic, alignment, outline, shadow, background box, and three quick
  placements. Writes are debounced 140 ms into one `text_set`; the preview's own
  restart debounce (160 ms) then turns a typed sentence into one re-render.
- **The placement policy is in Rust** (`text/edit.rs`, pure, 9 unit tests):
  duration, the size as a fraction of the canvas's short edge, the default
  outline, which lane, and the free-slot search. Policy in the webview is policy
  in two places.

### What is not undoable, and why

**Changing a title's words, font or colour is not on the undo stack.** There is
no `EditCommand` variant carrying a `TextMaterial`, and `timeline/ops.rs` was
owned by other work while this landed, so `text_set` writes the material pool
directly and schedules an autosave. *Adding* and *deleting* a title are ordinary
undoable edits; only the parameters are not.

The fix is small and known: an `EditCommand::SetTextMaterial { id, before,
after }`, mirroring `SetTransition` exactly — which is the variant `transitions`
already has for this shape of edit. Note when doing it that a per-keystroke undo
step is not wanted; the debounce above is the natural granularity.

### Preview and export are the same pixels, proved

`src-tauri/tests/text_clip.rs`, 14 tests, no fixtures needed and no `ffmpeg`
binary — it draws its own title and encodes with libavcodec in process.

- At the same target size the two paths are **byte-identical**: 0 of 921,600
  pixels differ between a frame composited the way `preview/server.rs` does and
  one composited the way `export/job.rs` does. Both build a
  `MediaSourceProvider::from_project` and call `Compositor::render_frame`; the
  only difference is the size, which is what makes this the whole claim.
- When the preview *is* smaller — a 4K canvas previews at 1920 — the title's ink
  rectangle lands within **2 preview pixels** of the same normalised position.
  That is the property "font size is in document pixels" exists to give, and its
  failure mode (a preview that looks right and an export with a half-size title)
  is invisible to any test that renders one path only.
- Through the **real exporter**: `run_export` to H.264, decoded by
  `VideoDecoder`, scored against the preview's frame at the same instant. Above
  30 dB, with two controls scored the same way (a blank frame, and the same
  timeline with the title moved off the playhead) both far below it.
- A title **does not vary with time** — two instants of one clip are the same
  bytes — which is what stops it flickering during playback if the provider's
  generic cache ever starts serving text.

### And it is a segment, verified rather than assumed

`split_at` does arithmetic on `source_range` and a title has no source to read
from, so it is checked directly: both halves keep the material, the durations
sum, the right half reads from where the left stopped, `validate()` is clean,
the split undoes in one step, and **both halves draw the same pixels**. Trim,
move, copy and delete go through `History` in one test and each undoes back to
the original range. Deleting the last clip using a title leaves the material in
the pool, deliberately — `RemoveSegment` carries the segment, not the material,
so undo needs it to still be there.

## Multi-selection, the clipboard and multi-clip edits

Added 2026-07-26. `useTimelineStore.selectedSegmentId` held one id, there was no
clipboard for clips at all, and four items in the Edit menu were
`Gate::Unimplemented` because of it. All four are now gated on real state.

What exists:

- **A selection, not a selected clip.** `selection: Id[]` plus a
  `selectionAnchor`. Ctrl/Cmd+click toggles, Shift+click takes the run between
  the anchor and the clip *along the one lane they share*, a rubber band over
  the lanes takes everything it touches, Ctrl+A takes everything on every
  unlocked lane. Every consumer that used to read the one id now calls
  `soleSelection(selection)`, which answers `null` when there are several —
  deliberately, because with four clips selected there is no "the" clip and the
  inspector would otherwise edit one the user is not looking at.
- **Cut, Copy, Paste, Duplicate**, on `Ctrl+X`/`C`/`V`/`D`, and in the Edit
  menu.
- **Moving, trimming and deleting a selection is one undo step**, through the
  new `timeline_apply_many` command.
- **Link** is offered on any multi-selection, from the clip's context menu and
  from the toolbar. It drives the `timeline_link` that already existed.

### Four decisions worth not re-litigating

**The clipboard is in the webview store, not in the document.** A cut clip is a
`Segment` with nowhere to be: a segment belongs to a track, and every
`EditCommand` that touches one names the track it is on. Giving the document a
holding pen for detached segments would reach the file format, the validator and
the exporter in order to store something the user does not think of as part of
their project — and would put copying on the undo stack and in the saved file.
It lives in `timeline/store.ts` with the zoom and the playhead instead, which is
also what makes it survive closing one project and opening the next. Rust learns
one bit about it, `MenuState::has_clipboard`, which is all the bar needs.
`src/modules/timeline/lib/clipboard.ts` has the full argument.

**A batch is a new command, not a `Composite` the webview built.** `timeline_apply`
would have taken one. The reason it must not is in `ops::mirror_linked_edits`:
`History::apply` mirrors a bare command onto its link partners and deliberately
does **not** recurse into a composite, because the two composites the app
already builds — a split and an import — place their own links. So a batch
arriving as a composite would silently stop mirroring. `ops::compose_edits` is
the one place that expands the partners of every part *and* drops the ones the
batch already names, which is what stops a selection holding both halves of a
linked pair from moving the sound twice.

**The parts of a batch are ordered, and this is not cosmetic.** Segments on a
track may not overlap even for the instant between two commands of one
composite. A block of clips moving one second later has to be applied
right-to-left; the same block moving earlier, left-to-right. Applied naively the
first command is refused, the composite rolls the whole thing back, and the user
sees a drag that did nothing.
`a_block_of_clips_moving_together_does_not_trip_over_itself` is that test.

**A batch of one is returned unwrapped.** `compose_edits` hands back the single
command untouched, so single-clip editing stays on exactly the path it has
always taken — including `History::apply`'s own link mirroring, which a
composite would have taken away.

### One behaviour that changed

**Dragging across empty lane space now draws a rubber band instead of
scrubbing.** A click on a lane still seeks and clears the selection; only the
drag is new. There was nowhere else a band could start, and scrubbing still has
the ruler strip and the playhead handle.

### Where a paste lands

At the playhead, keeping each clip's offset from the earliest one in the batch,
on the lane it was copied from when that space is free, on the next free lane of
the same kind when it is not, and on a lane created for it when every one of
them is busy. It never overlaps: `planPaste` checks each destination against the
lane *including the clips the same paste has already placed*, which a per-clip
check misses when a batch is pasted onto itself. Copies keep their source range,
speed, transform, effects and keyframes; a pair copied whole is relinked into a
group of its own, and half a pair pastes as a plain clip — the same rule
Duplicate has always had.

**Pasting into a different project brings the file over.** A material id
identifies a file inside one document and nothing in the next, so a clip pasted
across projects would reference a material that document has never heard of —
an error in `validate()` and nothing to draw for the renderer. So the clipboard
carries the material's *path* as well, and `edits.ts::adopt` imports it through
`project_import_media`, which is keyed by path and returns the existing material
when there already is one. Nothing is imported in the ordinary same-project
case. The one thing that cannot cross yet is a **title**: a `TextMaterial` has
no file, and carrying one would need an edit command that writes the material
pool. Those clips are dropped from the paste rather than pasted broken.

## Audio fades, ripple edits and track management

Added 2026-07-27, all three timeline-side.

- **Audio fade handles.** Every clip that is heard from where it sits (its
  material carries audio and the sound is not on a linked lane) gets a drag
  handle at each top corner; dragging inward draws the ramp and writes plain
  `Volume` keyframes on release — `(0,0)→(len,1)` for a fade-in, the mirror for
  a fade-out, one shared apex when they meet. There is no "fade" anywhere in
  the document or the engine: both mixers already sampled the `Volume` track
  (`audio/mixer.rs::a_volume_keyframe_fades_across_the_segment`,
  `export/audio.rs::volume_keyframes_fade_the_segment` are the proof, not an
  assumption), so preview and export follow for free. The keyframe value
  multiplies the segment's own volume, so 1.0 is neutral.
  `timeline/lib/fades.ts` reads fades back *by pattern* and its command builder
  diffs current-against-wanted — hand-authored volume automation between the
  ramps is left alone, and a fade dragged to zero removes its keyframes rather
  than leaving a zero-length ramp that shows the property as animated.
- **A finding worth keeping: `split_at` used to copy keyframes verbatim onto
  the right half.** Keyframe times are segment-relative, so the right half
  replayed the *whole clip's* animation from the cut onward — a fade-out
  authored for the tail played `left_duration` too late, and the head's
  keyframes existed twice. Fixed in `ops.rs::rebase_keyframes_for_split`:
  times at or after the cut shift onto the right half's own clock, a cut
  mid-ramp gets an anchor keyframe holding the interpolated value (so a fade
  crossing the cut stays continuous), and a track fully consumed by the left
  half collapses to a single anchor holding its last value. The left half is
  deliberately untouched: it keeps keyframes beyond its new end, which is the
  same warning a tail trim leaves and exactly what makes it play unchanged.
- **Ripple delete and close gap.** Context menu on a clip and on a lane's
  empty space. Both are one `timeline_apply_many` batch — one undo step — and
  the frontend (`timeline/lib/ripple.ts`) sends the parts already ordered:
  removal first, then leftward moves left-to-right, because `compose_edits`
  deliberately leaves a mixed batch in the caller's order. Link partners come
  along on the Rust side, exactly once; a ripple whose mirrored move has
  nowhere to land (a music bed in the way) is refused wholesale and the
  timeline is untouched. "Close gap" only offers on a *bounded* gap.
- **Track management.** "Add track" (video/audio/text) below the headers, lane
  reorder by dragging the header grip, delete via the header's context menu
  with a confirmation dialog when the lane holds clips. Reorder is a new
  `EditCommand::MoveTrack { track_id, from_index, to_index }` whose `to_index`
  is the position in the *resulting* list, so its inverse is itself with the
  indices swapped; `reindex_render_order` runs on apply, so restacking the
  composite *is* the reorder and `render_index` needs no separate bookkeeping.
  Tests pin the restack and the byte-exact undo. One known wart: deleting a
  track whose clips are linked to clips on other lanes leaves those partners
  in a one-member group until undo — the link badge shows with no partner.

## Crop, rotation and per-clip colour (2026-07-27)

The inspector now has all three, and the honest headline is what was *already
there*: **crop and rotation were fully implemented in the renderer and wired to
nothing.** `layout::place_quad` had been cropping (UV rect, re-fit to the
cropped aspect) and rotating (pixel-space, both decode paths, since the MVP is
path-independent) all along, with geometry tests — what was missing was any UI
and, for crop, any command. Before building anything, check what the
compositor already does.

What was added, and where the reasoning lives:

- **Crop UI** — four inset sliders plus reset, `inspector_set_crop`. The
  panel speaks insets, the document keeps the kept-rectangle; `insetsOf` /
  `cropFromInsets` in `src/modules/inspector/lib/adjust.ts` translate, and the
  slider cap (45% per edge) is what makes the empty crop unreachable.
- **Rotation UI** — the slider existed; ±90° step buttons were added, wrapped
  into `-180..180` so four turns land back on exactly 0.
- **Colour adjustments** — brightness, contrast, saturation, temperature as a
  new typed pool category, `ColorAdjustMaterial`, referenced from
  `Segment::extras` exactly like a transition and for the reasons written on
  the struct; opacity stays a `Transform` field but renders in the colour
  panel. The GPU work is one branch in `quad.wgsl` (`apply_color`, encoded
  space, order documented there), so transition layers, the preview and both
  export tiers get it from the same draw. **Identity is a flag, not
  arithmetic**: an ungraded or identity-graded clip takes the exact pre-colour
  shader path, and `an_identity_grade_renders_byte_identical_to_no_grade_at_all`
  asserts full-frame byte equality.
- **Undo without touching `timeline/ops.rs`** — both edits are a `Composite`
  of `RemoveSegment` + `InsertSegment` of the same segment, built in Rust from
  the live document (`modules/inspector/edit.rs`, module docs say why this is
  the command model used as designed and not a trick). Colour additionally
  never mutates a material in place: each commit mints a fresh material and
  swaps the segment's reference, so undo is a reference swap back to a
  material still in the pool. See `docs/decisions/0007-colour-as-materials.md`.

Verified with pixel tests in `compositor.rs`, all run on **both decode
paths** via split-tone sources (a solid colour cannot show *which part went
where*): crop keeps only the kept region and re-fits its aspect; 90/180/270
put each half where a clockwise turn says; 45° draws the exact diamond;
crop-then-rotate composes in document order; grades match a spelled-out
reference to ±2 code values; and preview (`render`) and export
(`render_nv12`) agree on a graded frame to ±3, the same harness the
transitions work used.

### .cube LUTs (same day, on top of the above)

Per-clip 3D LUTs, applied **after** the scalar grade — grade first, look
second, the order looks are authored against and the order `quad.wgsl`
implements. What exists and the decisions inside it:

- **Parser and reference sampler** in `modules/render/lut.rs`: 3D `.cube`
  (TITLE, LUT_3D_SIZE 2..256, DOMAIN_MIN/MAX, red-fastest data), tolerant of
  comments, CRLF and trailing whitespace; every malformed file is refused
  with a message naming the line; 1D LUTs are refused by name.
  `Cube::sample` is the CPU trilinear the shader mirrors, so tests compare
  the two instead of letting a change cancel out.
- **Document**: `LutRef { path, intensity }` on `ColorAdjustMaterial` — not
  a category of its own, because grade and look are one gesture, one
  material, one uniform block, and a second category would ask the ordering
  question across two materials. Only the **path** is stored. The parsed
  cube is runtime cache (`LutCache`, keyed path+mtime, one `stat` per graded
  clip per frame), so editing the file externally shows on the next frame
  with no invalidation plumbing, and a project whose LUT file is gone
  **opens, warns (`validate()`), and renders the clip unadjusted** — pinned
  byte-identically in a test, missing and malformed files both.
- **GPU**: `Rgba32Float` 3D texture, trilinear **by hand from eight
  `textureLoad`s** rather than the sampler. Not squeamishness: hardware
  filtering quantises interpolation weights (8-bit subtexel typically),
  which fails the identity requirement — an identity `.cube` at full
  intensity renders **byte-identical** to no LUT, asserted at sizes 2 and
  17. `lut_active` gates the whole thing exactly as `color_active` does, so
  pre-feature documents take the untouched shader path.
- Analytic pixel tests on both decode paths (invert, channel swap, ×0.5
  gradient — numbers a reviewer recomputes in their head), intensity as an
  exact lerp with 0 byte-identical to absence, grade-then-look order pinned
  against the reversed order's number, mtime reload, and preview/export
  parity over a graded+LUT frame.
- **UI**: a LUT row in the Colour section — file picker (validated by
  `inspector_lut_probe` at pick time, so a broken file is refused with the
  parser's line message before touching the document), filename display,
  0..1 intensity slider, remove. Same mint-and-swap undo as the sliders.
- **Cost, measured**: `chukcut-bench --filter composite` grew `grade+lut`
  rows (a 33-point warm LUT plus a non-identity grade on every layer) priced
  against the `plain` rows. Run 2026-07-27 with `--force` at load ~7 (other
  agents building — treat as indicative; spreads were nevertheless ≤ 1.3×):
  GPU-only per 1080p frame, plain → grade+lut: 1 layer 1.54 → 2.08 ms,
  3 layers 2.67 → 4.23 ms, 10 layers 6.43 → 12.56 ms — i.e. **the colour
  pass costs ~0.5–0.6 ms per full-canvas graded layer** on this iGPU. Not
  free, which is why both halves are flag-gated (`color_active`,
  `lut_active`): ungraded clips pay exactly nothing, a grade without a look
  skips the eight LUT taps, and a look without a grade skips the scalar
  arithmetic. Re-run on a quiet machine before quoting these anywhere.

## The app shell: recent projects, project settings, shortcuts, transport keys (2026-07-27)

Four additions, all workspace/app-side; the timeline and export modules were
not touched. What the next person needs to know:

- **The menu table supports one level of submenu.** `menu.rs` gained
  `Entry::Submenu` and `describe` gained a `recent: &[RecentEntry]` parameter —
  the recent-projects rows are the one part of the bar that is the user's data
  rather than the `const` table, so they are injected there and nowhere else.
  A submenu never nests another (enforced with an `unreachable!`), missing
  files draw greyed with "file is gone" rather than being pruned, and the
  trigger is forced off when the list is empty. The webview's dedup on
  `MenuState` cannot see the recent list, so `installMenu` additionally
  subscribes to the workspace store's `recent` — without that, Clear List
  leaves a bar offering entries Rust has forgotten (`menu.ts` says why).
- **Project settings (File → Project Settings…) are one undoable step on the
  same stack as timeline edits.** `project_configure` +
  `project::ConfigureCommand`, recorded by `state::DocumentHistory`, which
  replaced `timeline::History` behind `AppState.history` with identical method
  signatures. **Read `docs/decisions/0008-one-undo-stack-two-command-kinds.md`
  before touching either history** — `DocumentHistory::apply` deliberately
  reproduces `History::apply`'s two expansions, and that coupling is the
  documented cost. Changing fps re-times nothing (times are micros); the
  dialog says so out loud. The dialog speaks sRGB hex, the document stores
  linear RGBA; the conversion is the real transfer function and the test pins
  that mid grey is ~0.216, not 0.5.
- **The shortcuts overlay is generated, not curated.**
  `workspace/lib/shortcuts.ts::buildShortcutGroups` reads the accelerators out
  of the same `MenuSectionView[]` the title bar draws, so a key added to the
  menu table appears in Help → Keyboard Shortcuts with no second edit; only
  keys the menu cannot advertise (transport, tools, mouse chords) are still
  a hand list, `EXTRA_SHORTCUTS`, annotated with each binding's owner. The
  test pins that no accelerator in the input can fail to appear.
- **J/K/L are bound in `src/lib/shortcuts.ts`, app-level.** J is deliberately
  *not* reverse play — the engine decodes forward and the audio clock is the
  master — so J pauses and steps back one frame, and both the code and the
  overlay say so. The one-owner-per-key rule held: Space/←/→/Home/End belong
  to `Preview.tsx` (Shift+←/→ is *ten frames* there, not one second — changing
  that means editing a preview-owned file), the tool letters to
  `Timeline.tsx`, and J/K/L had no owner before this.

## The native inspector (GPUI, 2026-10-03)

`crates/app/src/editor/inspector/` is CapCut's right-hand panel: Details with
a "Change" form (`project_configure`), and per clip the tabs Video (Basic:
transform, alignment, blend opacity) · Audio · Speed · Animation · Adjust, or
Basic · Voice changer · Speed for sound. Every edit is an engine command.

- **A slider drag is one undo step.** While the thumb moves, the command is
  built against the snapshot from before the drag and applied to a *copy*,
  which becomes the editor's drawing snapshot. On release the copy is dropped
  and the same command goes through the command layer once. No history
  coalescing was needed (`inspector/mod.rs`, `Preview`).
- **Speed is `inspector_set_speed`, not `SetSpeed`.** The primitive keeps the
  timeline length and does not mirror onto link partners, so a sped-up picture
  drifts from its sound. The new command keeps the source, retimes the clip
  and its partners and ripples the later clips on their lanes, as one step.
- **The `Volume` keyframe track is the fade envelope** — it multiplies the
  clip volume. The volume row therefore has no keyframe diamond; the fade rows
  read the envelope by pattern and rewrite it whole.
- **Keyframe diamonds** on scale, position, rotation and opacity use
  `AddKeyframe`/`RemoveKeyframe`. Once a property has keyframes, a value change
  edits the keyframe at the playhead (or adds one), because the static value
  is no longer what is shown.
- **Plain-key shortcuts carry the context `!Input`** (`main.rs`). Without it,
  typing "s" into any text field split the clip and Backspace deleted it.
- Not in the engine, drawn disabled: blend modes, pitch, animations, masks,
  voice changer, stabilise and the other AI sections. The whole Adjust tab
  works since the colour grading below; Brilliance was dropped from it for
  Exposure and Vibrance.
- Driving the window with `xdotool`: compute the window id into a variable
  and check it. A helper that also printed a path made `--window` garbage, and
  a drag then silently did nothing, which looked like a GPUI drag bug.

## The native app shell (GPUI, 2026-10-03)

`crates/app/src/editor/{shell,home,lifecycle,settings,shortcuts,playback}.rs`,
decision 0004 ("The native app"). Verified on Xvfb with isolated XDG
directories: start screen, preset → editor, recent card with poster and
missing state, crash → prompt → restore, Ctrl+Q and the close button
(`WM_DELETE_WINDOW`) both guarded, Save in the guard writes the file,
Don't save leaves it alone and deletes the working copy.

- **The root view is `Shell`, not `Editor`.** Every document gets a fresh
  `Editor`; the editor asks for Home / Open / Quit through `EditorEvent`
  after `guard_unsaved`. The engine's state is open before `Editor::new`.
- **Test against your own config.** The app reads and writes
  `~/.config/chukcut` (settings, recent list, working copy). Run test
  instances with `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and `XDG_STATE_HOME`
  pointed into `_scratch/`, or a test session claims the owner's working copy
  as a crash.
- **Trap: an app-wide action handler cannot update the window it came
  from.** GPUI dispatches it while that window is borrowed, so
  `window.update` fails. The first Quit fell back to `cx.quit()` on that
  failure and quit without asking. `shell::quit` defers.
- **J/K/L above 1× and in reverse are silent**: the audio engine has no rate
  control, so a driven shuttle pauses the clock and moves the playhead from
  the tick. In/out marks and loop live in `playback.rs`; the timeline ruler
  does not draw them yet (`Editor::play_range` is there for it).
- File dialogs need xdg-desktop-portal; on a bare Xvfb they fail, so Open…
  and Save as were not exercised there.

## Proxy policy and cache limit take effect (2026-10-03)

The two Settings rows the shell wave stored but nothing read now act. Both are
engine-side; the app only writes the settings.

- **`workspace_settings_set` applies what it saves** through
  `workspace_settings_apply`, which `crate::init` also calls with the stored
  settings at startup. No shell has to remember to push them.
- **Proxy policy lives on the queue** (`proxy::policy`, `ProxyQueue::policy`).
  Off: nothing is queued, the preview never decodes a proxy, and turning it
  off cancels running jobs. Automatic: `decision::decide` unchanged. Always:
  every video a proxy would make meaningfully smaller — the shrink clause
  survives, because a 720p "proxy" of a 720p file decodes no faster. A policy
  change bumps `ProxyQueue::generation()`.
- **The shared queue starts Off** until settings are applied. Integration tests
  open projects through the command layer and must not transcode their
  fixtures into the real `~/.cache/chukcut/proxies`.
- **Enqueueing**: `project_open`, `project_new`, recovery restore, the working
  copy restore and `project_import_media` call `proxy_request_media`, which
  probes on its own thread and returns at once. Import requests only the new
  file. Turning the policy on from Off considers the open project's media.
- **The preview half is not wired in the app yet.** `ProxyQueue::preview_source`
  honours the policy, but `crates/app/src/player.rs` still builds
  `MediaSourceProvider::from_project`, so the player decodes originals whatever
  the policy says. That file belongs to the perf agent, who plans "proxies on
  by default for heavy files"; the patch is decision 0003's provider seam plus
  folding `proxy_generation()` into the player's provider key. When it lands,
  drop "The player does not switch to proxies yet." from the Settings hint.
- **Cache limit**: `workspace::trim`, command `workspace_trim_cache(limit)`.
  Least recently used first, by the later of mtime and atime, except proxies,
  whose index records every lookup to the millisecond. Kept: the open
  project's thumbnail and waveform directories and its proxies (the engine
  learns the media from the project commands via `workspace_cache_in_use`),
  dot-prefixed / `.part` / `.tmp` files being written, `proxies/index.json`,
  and the whole of `whisper/` — a downloaded model is not derived data, so it
  is outside the count too. Voice cleanup renders are ordinary LRU candidates:
  their key needs the clip's strength, which is not cheap to know here.
  Protected files still count; a limit below what the open project needs
  deletes everything else and stops.
- **When it runs**: after every proxy lands (the shared queue's ready hook),
  when the limit changes, and once at startup — but **not before the first
  project is opened, created, restored or closed**. `crate::init` runs before
  the startup project is open, and a trim there raced the open and could
  delete the thumbnails it was about to show. A session that never leaves the
  start screen does not trim.
- The proxy cache keeps its own 20 GiB cap underneath; the setting is the
  whole cache's ceiling. `ProxyCache::forget_missing` reconciles the index
  after a trim so the Settings readout stops counting deleted proxies.
- Tests: `proxy::policy::tests::the_policy_decides_what_gets_enqueued`,
  `queue::tests::the_policy_gates_enqueueing_and_the_preview_switch`,
  `workspace::trim::tests::trimming_removes_the_oldest_first_and_leaves_protected_files`
  and the pure `plan` tests. Their scratch directories are under
  `target/<profile>/test-scratch/`, not the system temp directory.

## Colour grading (2026-10-03)

The Adjust tab is complete: Basic, HSL, Curves and Colour wheels, plus `.cube`
LUTs (3D and 1D) with a library. Everything runs in `quad.wgsl`, so the
preview, both export tiers and transition layers get the same pixels.

- **Model.** One new field, `ColorAdjustMaterial::grade`
  (`project/grade.rs`), skipped on save when at rest. Old projects open with
  the grade at rest and save byte-identical; a test pins that. Units are
  meanings, not slider positions: exposure in stops, most controls `-1..1`,
  curves as `[x, y]` points (empty = identity), wheels as a puck in the unit
  disc plus luma. Decision 0007 has an addendum.
- **Order of operations** is written once, in `render/grade.rs`: sharpen,
  clarity (measured) and exposure in linear light on the source; clarity,
  tint, the original four sliders, whites/blacks, shadows/highlights, wheels,
  HSL, vibrance, curves, LUT and fade in encoded space; vignette and grain in
  linear light after. Each stage has a bit in `QuadUniform::features`. A clip
  at rest takes the old shader path; `an_extended_grade_at_rest_changes_no_byte`
  asserts full-frame byte equality on both decode paths.
- **Tests compare against a CPU reference**, `grade::reference_encoded`, on
  both decode paths, per stage and all at once
  (`render/grade_tests.rs`, ±3 code values; the slack is the quantised
  readback the input is taken from). Two deliberate shader mutations both
  failed the suite. The neighbourhood stages (sharpen, clarity) are tested
  structurally: a flat source does not move, an edge gains contrast.
- **Preview/export parity** over a frame with every stage live, grain and
  vignette included, both decode paths: 2×2 block means agree within 4 code
  values (grain is per pixel and NV12 chroma is per 2×2 block).
- **Grain** is a PCG hash of the output pixel and the frame time in ms. No
  clock, no counter: the preview and the export must draw the same grain.
- **Curves** bake into a 1024×1 `Rgba32Float` table (r, g, b, master),
  cached by the points' bits. The table is within 0.01 code value of the
  exact monotone cubic. Fritsch–Carlson, so a curve never dips between
  rising points.
- **1D LUTs** fold into the same 3D texture binding as cubes, rows of 1024
  entries in one slice, so up to 65536 entries fit under the device's width
  limit. `LUT_1D_INPUT_RANGE` / `LUT_3D_INPUT_RANGE` are read as the domain.
  A file with both a 1D and a 3D table is refused by name. Identity LUTs
  (3D 33 and 65, 1D 2 and 4096) move no pixel by more than one code value.
- **LUT library**: `inspector_lut_import` validates and copies into
  `$XDG_DATA_HOME/chukcut/luts` (`workspace::paths::luts_dir`, a new data
  root — not the cache, so "clear cache" never takes a look away). Same file
  twice is one entry; a name clash gets "Name 2". Errors name the file and the
  line.
- **Commands**: `inspector_set_grade` (whole grade), `_set_grade_control`
  (any single control by `GradeControl`), `_set_curve`, `_set_wheel`,
  `_set_lut`, `_reset_grade` (by section), `_lut_library`, `_lut_import`. All
  funnel into `edit::set_grade_command`: validate, clamp, canonicalise, mint,
  swap. `inspector_set_color` keeps the extended grade, so a filter preset no
  longer wipes a drawn curve. Apply to all shares the material; paste carries
  the grade as values (`ClipAttributes::grade`).
- **UI** (`app/.../inspector/grading.rs`): curve editor and wheels are GPUI
  canvases. Their move/up listeners are registered on *every* paint, not only
  mid-drag: a press, its moves and its release can all arrive before the next
  frame, and a missed release left the drag stuck.
- Not done: a per-clip "Save as preset", Auto adjust / Colour match, and a
  hue-vs-hue curve. The `Import LUT…` button uses the desktop file portal,
  which does not exist under Xvfb; the import itself is covered by tests.

## Effects, layouts and the transition library (2026-10-03)

Built-in effects, picture in picture and split screens, and 125 library
transitions. Engine in `modules/fx/` and `modules/transitions/library/`; UI
in `editor/assets/effects.rs`, the Transitions tab in
`editor/assets/library.rs`, and `editor/inspector/effects.rs`. Decision 0016
has the model.

- **Model.** An effect is an `EffectMaterial` (`project/effects.rs`) in the
  new pool category `materials.effects`, skipped on save when empty. On a
  clip its id sits in `extras`, in order; as an **effect clip** it is the
  material of a segment on an effect lane and applies to everything
  composited beneath it. `kind` is a string and `params` a sparse map, so an
  unknown effect loads and renders as nothing, and a default is never
  stored. Keyframes and the animation clock are in the clip's **source
  time**, so a split keeps one continuous shake across the cut.
- **Edits** (`fx/edit.rs`) mint a material and swap the reference with a
  `RemoveSegment` + `InsertSegment` composite — decision 0007's rule, no new
  `EditCommand` variant. Commands: `fx_add`, `_remove`, `_set`,
  `_set_param` (at the playhead when animated), `_toggle_keyframe`,
  `_set_enabled`, `_reset`, `_move`, `_add_clip`, `_layout_split`,
  `_layout_pip`, and the tile commands.
- **Effects** (17): blur, zoom blur, glow (threshold, source channel, tint),
  light sweep, shake (seeded), RGB split, glitch (seeded), VHS, pixelate,
  mirror, kaleidoscope, grain (the grade's PCG grain), halation, bloom, gate
  weave, letterbox, and **frame** — rounded corners, border and drop shadow
  for picture in picture, as signed distances in the clip's own frame, so no
  blur pass. Vignette is not an effect: the grade has one.
- **Rendering.** The compositor draws an effected clip into a layer (the
  transition layer pipeline), runs the passes over it in premultiplied
  linear `Rgba16Float` and composites the result where the clip would have
  been; a transition side runs its clip's effects before the blend; an
  effect clip ends the composite pass, runs over the target so far and
  carries on in a fresh target. All in the frame's one command encoder. A
  frame with no live effect takes the old path exactly. Lengths are
  fractions of the frame's shorter side, so preview and export match.
- **Tests** (`fx/render_tests.rs`, 24, plus an ignored measurement): blur, pixelate, RGB split, every
  mirror mode, letterbox, shake, gate weave, light sweep, grain and the
  frame against CPU references written from the same description; glow,
  bloom, halation, zoom blur, kaleidoscope, glitch and VHS structurally
  (flat stays flat, light spreads past the threshold and not below it,
  symmetry, a seed repeats and another differs); effect clips apply below
  and not above; a hidden effect lane does nothing; preview/export parity
  on a frame with an effect clip, an effected clip and grain; at rest,
  switched off or unknown is byte-identical. Two deliberate shader
  mutations both failed the suite.
- **Layouts.** Split screens (top and bottom, side by side, three rows,
  three columns, grid) crop each selected clip to its cell's shape and
  scale it to fill the cell, topmost lane first, one undo step. Picture in
  picture scales a clip into a corner and gives it a frame.
- **Transition library.** 120 gl-transitions ported to WGSL by naga through
  `transitions/library/port.py` (committed output, licence headers kept,
  `LICENSE-gl-transitions.md`, `NOTICE.md`); five left out with reasons.
  Five seamless transitions of ours move both clips with shutter motion
  blur: zoom in, zoom out, spin, whip pan, push. All are `TransitionKind::
  Library` with a `preset`; the existing draw path dispatches to a
  per-preset pipeline compiled on first draw. Every preset is checked to
  validate, to start on the outgoing clip and to end on the incoming one
  (one documented exception).
- **Tiles.** Every effect, transition and layout tile is the real thing,
  rendered by the compositor over a procedural sample landscape and cached
  as a PNG under `~/.cache/chukcut/fx-tiles`, keyed by a hash of its shader
  source.
- **Cost.** `fx::render_tests::measure_effect_cost_per_frame` (ignored;
  `--release --ignored --nocapture`). Measured under a load average of
  about 30, so only an upper bound: no effect added more than ~2.5 ms to a
  1080x1920 frame on the RTX 3060. Re-measure on a quiet machine.
- **Traps.** Some gl-transitions add 1 to alpha as if it were a colour;
  GL clamps on write, so the harness clamps alpha before unpremultiplying,
  or the frame darkens by half. A WGSL `let` named after a builtin (`step`)
  is legal and confusing; `fx.wgsl` avoids it. `naga-cli` is only needed to
  regenerate the ports: `cargo install naga-cli --version 30.0.0`.
- Not done: a colour picker (colour parameters offer nine swatches), keyframe
  easing per effect parameter (linear only), effect presets (saved
  parameter sets), and motion blur that follows a clip's own keyframed
  motion.
## Captions and auto captions (2026-10-03)

A **Captions** tab in the asset panel, CapCut's "Untertitel": auto captions,
a caption list, styles, and SRT/VTT in and out. Engine side, three modules:

- **`modules/captions`** — a caption is a text clip on a `Captions` lane whose
  `TextMaterial` carries `caption: Some(CaptionData)`: the spoken words with
  times (segment source time, so they follow a move or a head trim) and an
  optional karaoke colour. SRT and VTT parse and write (tolerant of BOMs, CRLF,
  markup, VTT notes and cue settings); words group into **word captions** (1–4
  on screen) or **sentence captions** (characters per line, lines, longest
  duration; a full stop or a 0.7 s pause always breaks). Place, split, merge,
  retext, restyle, regroup and clear are pure functions returning
  `EditCommand`s, so **every caption edit is one undo step**. That needed the
  long-promised `EditCommand::SetTextMaterial { before, after }` in
  `timeline/ops.rs` (see "What is not undoable" above: the variant now exists;
  `text_set` still writes the pool directly and could switch to it).
- **`modules/cloud`** — the first piece of the provider registry from
  `docs/research/integrations.md` §7: accounts (`~/.config/chukcut/accounts.toml`)
  and their keys (`secrets.toml`, created 0600 in a 0700 directory, tightened
  at load, `CHUKCUT_KEY_<ACCOUNT>` overrides). One kind so far,
  OpenAI-compatible, with a `Transcribe` capability and a connection test
  (`GET {base}/models`). `User-Agent: chukcut/<version>` and nothing else about
  the user; keys never reach a `Debug` print, an error message or a project.
- **`modules/speech`** — transcribes the **timeline mix** at 16 kHz mono (the
  playback mixer, so cut material is not captioned and times need no mapping).
  Cloud: WAV chunks of at most ten minutes (19 MB, under the 25 MB limits),
  cut at the quietest 50 ms in the last 30 s, sent with `verbose_json` and
  word + segment granularities; a server that rejects the granularity field is
  asked again without it, and segment-only or text-only answers are turned
  into estimated words. Local: whisper.cpp (decision 0012), models downloaded
  with a pinned SHA-256 and a progress bar.

The karaoke highlight is drawn by the rasteriser: `TextRequest::highlight`
fills one byte range a second time in its own colour, and
`media::provider` caches a karaoke caption's upload **per lit word** under its
own key, so the time-blind text cache does not freeze the first word.

Colour emoji needed nothing new: the rasteriser already paints CBDT bitmap
strikes (Ubuntu's Noto Color Emoji). The trap is the symbol blocks: ☕ ⭐ ✅ ⏰
are *text* presentation unless followed by U+FE0F, and then they are drawn
monochrome from the caption font. Every emoji in `captions/emoji.rs` carries
the selector where it needs one, and a test checks it.

Measured: `cargo run -p chukcut-engine --features local-whisper --example
captions -- jfk.wav --local tiny` transcribes the 11 s JFK sample with correct
word times in 3.7 s in a debug build at load 30.

**The player's provider did not see new titles.** `player.rs` keeps one
`MediaSourceProvider` while the set of files is unchanged (it holds open
decoders), but the provider copies text materials by value — so any title or
caption added after the first frame was drawn as the red offline placeholder,
and a retyped one kept its old pixels. `MediaSourceProvider::sync_texts` now
runs before every preview frame and drops the cached upload of a changed title.

**The timeline's split divides a caption's words** (glue, 2026-10-03).
`split_at` asks `captions::edit::split_material` for two materials: the left
half keeps its material (shortened by a `SetTextMaterial`), the right half
gets a new one through `AddMaterial(PoolMaterial::Text)` — the first text
material minted *inside* an undoable command, so S, Ctrl+B, the blade, split
all and silence cuts all divide captions and undo exactly. Word times stay in
source time; the right half's source starts at the cut. A cut with no text
on one side (a one-word caption) keeps the whole caption on both halves.
`tests/split_glue.rs` splits a document where clips carry a grade, an
animation, keyframes, transitions, a link, a follower and captions, a few
hundred seeded times, and checks validate, every `extras` reference, every
follow link, every caption word's timeline time and byte-exact undo/redo. It
found one real bug: undoing a split whose left half lost its animation put
the animation id back at the end of `extras`; `SetAnimation` now carries the
`slot` it removed the id from.

Known gaps: the emoji *picker* is drawn by GPUI, which shows some emoji as
monochrome outlines (the caption itself is colour, drawn by our rasteriser);
a font
from an online library has a hook (any family registered with the text
renderer shows up in the font list) but no library yet; the drag frame on the
player is a rectangle, not handles.

## Speed curves and keyframe easing (2026-10-03)

Speed → **Curve**: six ramp presets (Montage, Hero, Bullet, Jump cut, Flash
in, Flash out; our own shapes), Custom, and a log-scale curve editor (0.1x to
10x; click adds a point, drag moves it, right-click removes it) with the
resulting duration. Keyframe easing: right-click an animated diamond in the
inspector, or a keyframe on the timeline, for the list (linear, hold, ease
in/out, and the motion library's smooth, snap, anticipate, overshoot,
elastic, bounce); the Video tab's **Keyframe easing** section draws the move
the playhead is in with two Bézier handles to drag. Decision 0018.

- **The model.** Points are source instants with a speed; the slowness is
  interpolated by a smoothstep, so a clip's length is a closed-form integral
  (`project::speed`). `TimeMap` (`materials.time_map(segment)`) is the one
  mapping between timeline and source time, and every caller of
  `Segment::source_time_at` now goes through it: compositor, prefetch,
  transitions (borrowed frames past a cut keep the edge speed), tracking
  follows and track start, effect keyframe clock, freeze frame, caption words,
  silence cuts, cloud placement, the app's tracking overlay and trims.
  `Segment::source_time_at` itself still ignores curves; do not call it.
- **Verified.** `tests/speed_curve.rs`: the compositor shows the counter frame
  the curve reaches at every timeline frame (±1), an export has exactly as
  many frames as the curve is long and the right frames in it, a split ramp
  plays frame for frame like the whole one with a continuous speed, a trimmed
  ramp keeps its slow motion on the same footage, every preset undoes and
  redoes to the byte, a linked sound takes the same curve, a ramped project
  saves and opens unchanged. `project::speed` tests the integral against a
  brute-force sum and the inverse against the forward map;
  `document::easing_tests` checks Bézier easing against independent reference
  values (CSS `ease` at 0.5 = 0.80240).
- **Easing everywhere.** `Easing::apply` gained `Curve(Ease)` and
  `Bezier { x1, y1, x2, y2 }`; transform, opacity, volume and effect
  parameter keyframes all sample through it, and transitions too. Old files
  read unchanged (the five old easings are still plain strings). Grade
  controls are not keyframable, so there is nothing to ease there.
- **Rough.** A curved clip is muted (no pitch-preserving stretch); the Curve
  tab says so. No frame blending or motion blur in slow sections: blending
  needs two source frames per output frame, and the sequential decoder would
  seek backwards for the earlier one on every frame — a measured 25–200 ms per
  seek. It wants a two-frame cache in the provider first. A slip of a curved
  clip changes its length (another stretch of curve is under it); the app has
  no slip gesture yet, and a `TrimSegment` that keeps the length is refused.
  Thumbnails and the waveform of a curved clip step at its average speed. Effect parameters have no easing picker yet (the
  engine eases them; the Effects tab writes linear keys).

## Not built yet

Both keyframe editing and audio waveforms landed overnight and this line was
stale within hours of being written — a reminder that a status document is
only worth what its last edit is worth.

The inspector now creates, moves and eases keyframes with a curve view, and
the timeline draws min/max envelopes with an RMS body, on audio lanes and
along the bottom of video clips that carry sound. What is *not* built is the
Rust side of the keyframe edit commands — `AddKeyframe`, `RemoveKeyframe`,
`MoveKeyframe` and `SetKeyframeEasing` — so those payloads are currently
rejected as an unknown variant. Everything else about the feature is done.

**Titles are built end to end.** This line used to say "titles render; there is
still no UI for making one". There is one now: see "Titles, end to end" below
for what exists, the one gap, and what the identity between preview and export
is proved by.

**Transitions render**, in the preview and in the export, on
software-decoded and hardware-decoded clips alike. The compositor hook that this
section used to say was missing is applied: `collect_draws` asks
`transitions::instant_for` per visible segment, draws each side into a
canvas-sized pooled layer with the **same** quad pipeline and `QuadUniform` — so
`planar`, `matrix`, `range` and `turns` travel through untouched and an NV12
surface blends like anything else — and the blend is one fullscreen draw inside
the composite pass, at the place in the painter's order the segment would have
had. `docs/architecture/transitions.md` has the shape and the reasoning; the
stale patch that used to live at the bottom of it is gone, because a patch that
no longer applies is worse than none.

Three things there that will otherwise cost an afternoon:

- **The layer pipeline has blending switched off, and it must.** A layer holds
  one clip over a transparent clear, and `ALPHA_BLENDING` over a transparent
  destination leaves *premultiplied* colour, which `transition.wgsl` then
  premultiplies again. Two opaque clips look perfect; a clip at 50% opacity
  darkens the instant the window opens.
  `a_half_transparent_clip_does_not_change_brightness_when_the_window_opens`
  fails by exactly a factor of two if it is switched back.
- **A quad's uniform slot is explicit, not positional.** A transition is one
  draw and two quads, so "the nth draw reads the nth uniform block" stopped
  being true. Getting it wrong draws the right clips with each other's
  transforms.
- **Preview and export agree because the hook is inside `render_to_texture`**,
  which `render_frame`, `render_nv12` and `render_nv12_into` all call.
  `the_preview_and_the_export_agree_on_a_frame_mid_transition` renders three
  instants of one crossfade — one side hardware-decoded, one software — through
  both entry points and compares the pixels.

**What is still missing is one element in the timeline.** The transition UI is
`src/modules/transitions/`: the geometry mirror of `resolve.rs`, the marker and
`+` button (`TransitionLane`), the parameter panel (wired into the inspector),
and the drag-a-clip-over-its-neighbour arithmetic in `lib/edits.ts`. Mounting
`<TransitionLane>` inside the timeline's lane, and calling `applyOverlap` from
its drag handler, are the two lines left; `src/modules/timeline/` was owned by
other work when this landed. Until then a transition is created from the
inspector and from the commands, and renders correctly, but the timeline draws
nothing at the cut. `MaterialPool.transitions` is also `?`-optional in
`src/modules/project/types.ts` only because two timeline test fixtures build a
pool as a literal and predate it.

Two things in that document are worth reading before touching either
`project/document.rs` or `render/`: a transition is centred on the cut and
**neither clip moves** (overlap would break the no-overlap track invariant and
shift the whole timeline on every duration change), and it hangs off the
**incoming** clip rather than the outgoing one, which is the opposite of CapCut
and reduces the cost of splitting a clip under a transition to one line in
`split_at`.

**Proxy media is built** — `src-tauri/src/modules/proxy/`, decision 0003 — but
two seams outside that module are still open and it does nothing until they are
closed: `media/provider.rs` needs the `from_project_for_preview` constructor
(the exact patch is at the bottom of decision 0003), and `preview/commands.rs` needs
to fold `proxy::ProxyQueue::shared().generation()` into the fingerprint its
provider cache is keyed on, or a proxy finishing will not be picked up until the
next edit.

The effect runtime is partly built — see "The effect runtime (Phase 3),
measured" above. It loads packages, compiles their shaders and runs their Lua,
and it cannot open a package that ships its assets in the binary encoding,
which is nearly all of them.

## One GPU device and one VAAPI device, per process

`src-tauri/src/modules/gpu/` owns both, and nothing else may open either. The
constructors that would let it — `render::RenderContext::open` and
`media::hwdecode::VaapiDevice::open` — are crate-private and called from exactly
one place each, which is that module.

| | opened one before | asks `gpu` now |
|---|---|---|
| wgpu device | `preview::server`'s render loop, `export::commands`' lazy compositor, `workspace::hardware`'s report, two `texture_pool` tests, `tests/support`, three examples and `chukcut-bench` | `gpu::render_context()` |
| VAAPI display | `preview::vaapi` (a fresh one per JPEG encoder, and the encoder is rebuilt on every size or quality change), `export::encoder` (one per rate-control rung it tried), `media::hwdecode` | `gpu::vaapi_device()` |

Both halves are done: **no non-test path opens either device.** The last VAAPI
holdouts went with them — `preview::vaapi` takes its display through
`export::hwframes::HwDeviceContext::shared_vaapi`, and `export::encoder`
through `HwDeviceContext::for_kind`, both of which are `gpu::vaapi_device()`
underneath. `HwDeviceContext::vaapi` survives as `#[cfg(test)]` only, because
the tests that check what a bad device node does have to try to open one.

The wgpu device is handed out as an `Arc`, because wgpu's `Device` and `Queue`
are `Sync` and internally counted. The VAAPI display is handed out **by value**,
each hand-out a fresh `av_buffer_ref` on one `AVBufferRef` — the shape
`media::hwdecode` already had, and the reason it works is that libavutil's
refcount is atomic, so a caller can pass its handle to a codec context, clone it
or drop it without coordinating with anyone.

Each has a tripwire, and they count different things on purpose.
`render::context::LIVE_DEVICES` counts devices *alive* and decrements on drop;
`hwdecode::LIVE_DISPLAYS` counts displays *opened* and never decrements, because
a `VaapiDevice` is `Clone` and an ordinary hand-out would otherwise look like a
close. Either logs an error the moment a second one appears.

Two devices were not a theoretical problem. Measured on the unit-test binary
with the preview-server timing tests skipped (`--skip modules::preview::server`,
319 tests, the render, export, media and VAAPI ones), 500 runs of each build,
interleaved so that a machine getting busier cannot flatter one of them:

| | runs | hard crashes |
|---|---|---|
| Two or more devices (before) | 500 | **9** — seven `SIGSEGV`, two `SIGABRT` |
| One device (after) | 500 | **3** — two `SIGSEGV`, one `SIGABRT` |

That is 1.8% against 0.6%, and it is a direction rather than a proof: nine events
against three is not significant at this sample size (Fisher's exact ≈ 0.08), and
one device is evidently **not** sufficient on its own. Every remaining crash is
mid-run in the `render::dmabuf` and `render::context` tests, which is where a
DMA-BUF is exported and a device is interrogated from several test threads at
once. The same 30-run comparison on `tests/compositor.rs`, `tests/export.rs` and
`tests/decode.rs` found no failure in either build, so what is left is specific
to the unit binary's parallelism.

**Those last three are very probably fixed, and the cause was `vkDeviceWaitIdle`.**
`ExportableBuffer::drop` called it directly to make sure nothing was still
reading the memory it was about to free. Vulkan requires host access to **every**
`VkQueue` on the device to be externally synchronised across that call, and
nothing in that `Drop` can synchronise against wgpu's own submissions or against
a second buffer being dropped on another thread. The export tears down one ring
at a time on one thread, so it never showed at 1-in-160; six rings torn down
concurrently by `preview::zerocopy`'s tests aborted the binary with a double free
or a `SIGSEGV` in **two runs out of five**, which is the same fault with the
volume turned up. It now waits through `wgpu::Device::poll`, which takes wgpu's
own locks and waits for the same thing: 0 failures in 20 runs of the zero-copy
set and 0 in 20 of the wider GPU set (`render::dmabuf`, `render::nv12`,
`render::context`, `preview::{zerocopy,encoder,vaapi}` — 44 tests). Nobody has
re-run the 500, so this is "the reproducible version of it is gone", not "the
3-in-500 is gone".

### The ordering bug a slow path was hiding

`tests/preview.rs::position_updates_arrive_in_order_as_the_playhead_advances`
began failing on nearly every run, and it was right to. The render thread
announces a *scrub* frame the moment its JPEG exists, off the render thread, and
the parked frame of a new session is a scrub. Press play immediately and the
pacer announces frames 1 and 2 before frame 0's announcement is emitted — the
frontend's playhead jumps backwards for one frame. Opening a device per server
took long enough that the scrub always won the race.

For calibration on how latent this was: that test failed **24 runs in 25** when
run on its own against the *old* code. It was already broken and only passed in a
full run because of the ordering a device open imposed.

The first fix compared the frame against the clock and then emitted, and **that
is a check the other thread can invalidate before the send happens**. Both
threads reach `Channel::send` through an `RwLock` *read* guard, so nothing
serialises them: the window between deciding and sending is exactly the window
the pacer needs to overtake. It is narrow, and a narrow window on something that
runs thirty times a second is a bug that shows up in a bug report and not in a
test.

`Shared::emit_position` now decides and sends under one lock. It also says which
announcements are *authoritative* — the pacer's and the transport commands',
which always go out and reset the mark — and which are subordinate, which is
only the scrub. That distinction is what keeps a backwards seek and a replay
from the end announceable while still dropping an overtaken scrub, and it is
why the rule is not simply "frame numbers never decrease".

### The hang that was not the device

`tests/preview.rs` used to hang in about half its runs, and this file blamed the
shared wgpu device: every thread parked in `futex_wait` with no GPU work in
flight, so a lost wakeup rather than a driver stall. That was the wrong
suspect. It is a **deadlock in the rayon pool**, it belongs entirely to the
hardware JPEG encoder, and it is now fixed.

`preview::encoder` holds one process-wide mutex around the one hardware
encoder. The RGBA→NV12 conversion used to run inside that lock — and that
conversion is a rayon parallel iterator. There are two ways in, and each on its
own is a hang:

- **From a rayon worker.** A rayon worker that blocks inside a parallel iterator
  does not idle; it joins the work-stealing loop and runs *any* other job in the
  pool. Encodes were dispatched with `rayon::spawn`, so the job it steals is
  another encode, which asks for the mutex that very thread is holding.
  `parking_lot::Mutex` is not reentrant, so it parks forever and never releases
  the lock. Every other worker queues behind it.
- **From any other thread.** The dispatch waits for a free worker. If the pool
  is meanwhile full of jobs blocked on that mutex, no worker will ever be free,
  and the lock holder waits behind the threads that are waiting for it. This is
  the one that survived the first fix and hung the unit-test binary at default
  parallelism, with the preview's own encode thread holding the lock.

The argument this file and the code both used — "nothing under `rgba_to_nv12`
takes a lock, so the thread holding the lock always makes progress" — is
plausible and wrong. **The job a blocked worker steals is not a piece of the
inner loop's work.** It is an unrelated task that happened to be in the same
pool.

Three changes, and the first is the one that closes the class:

- The conversion happens **before** the lock is taken, into
  `vaapi::Nv12Scratch`. Nothing under the mutex touches rayon at all now.
- Encodes go to `server::encode_queue`'s own thread rather than to the rayon
  pool, so the pool cannot fill with jobs that want that lock. One thread, not
  several: the encoder is one non-reentrant device whose mutex serialised them
  anyway, and one encode is 6 ms against a 33 ms budget.
- Frame-URL requests get their own small pool, for the same reason one level
  down. `serve_uri` parks on a condition variable for up to 60 ms when a frame
  is not ready; answering those on the global pool means that *exactly* when the
  renderer falls behind, every miss takes a worker away from the conversion that
  would let it catch up. Bounded, so never a deadlock — but it is the same
  feedback loop `request_frame` documents, arrived at through the scheduler.

Measured on this machine, `CHUKCUT_PREVIEW_JPEG=hardware`, one binary run per
row:

| | before | after |
|---|---|---|
| `tests/preview.rs` | **7 hangs in 7** | 0 in 25 |
| unit binary, default parallelism | **6 hangs in 6** | 0 in 25 |
| unit binary, `preview::` only | — | 0 in 30 |
| `tests/compositor.rs`, `tests/export.rs` | 0 in 25 | 0 in 25 |
| `tests/decode.rs` | 0 in 25 | 1 in 25, and it is a timing assertion |

**So `CHUKCUT_PREVIEW_JPEG` is back to hardware by default.** The 4 ms it saves
on a 9.4 ms frame is worth having, and it is no longer bought with a preview
that can stop.

Two things to carry forward from it, because they generalise past this module:

- **A benchmark cannot see a scheduling deadlock.** `preview_jpeg_bench` and
  `chukcut-bench` both encode one frame at a time and measured this path at
  6.2 ms for months. The state needs two encodes at once and it is not
  reachable serially.
- **`std::thread::spawn` in a test does not exercise a rayon hazard.**
  `concurrent_encodes_all_finish` ran eight threads through the encoder and
  passed throughout, because an OS thread has nothing to steal and nothing to
  wait for. `encodes_dispatched_onto_the_rayon_pool_all_finish` is the test that
  can reach it, and it is the one that found the second deadlock above.

## The native timeline (2026-10-03)

`crates/app/src/editor/timeline.rs`, with `timeline/ripple.rs` (multi-clip
gestures as `EditCommand` batches, unit-tested) and `timeline/media_cache.rs`
(filmstrips and waveforms). Laid out after CapCut: toolbar, track headers with
lock / eye / mute, a Cover box before the main lane, filmstrip video clips,
waveform audio clips, trim handles, snapping, Ctrl+wheel zoom around the
pointer, a scrollbar, a resizable split (42% of the window by default).

- **The main track magnet is on by default**, as in CapCut. The first video
  lane stays gapless: delete, trim and Q/W ripple through it, a drag along it
  reorders, a clip lifted off it leaves no hole. A reorder goes through
  `timeline/batch.rs::arrange`, which parks every moving clip past the end of
  everything and then places it, and moves link partners by each clip's net
  distance itself. (`compose_edits` mirrors only the first move of a clip that
  moves twice — the gap the first version had for linked clips on this lane.)
- **Dropping above the video lanes or below the audio lanes makes a new lane;
  a move that empties an overlay lane removes it.** One undo step each.
- **Filmstrips are one strip per material and zoom bucket** (a power of two of
  tiles over the whole file), decoded off the UI thread, at most two jobs at a
  time; the nearest ready bucket stands in while the right one loads.
- **The app embeds `gpui::assets::AllAssets`** (the full Lucide set). The
  component default bundle lacks scissors, locks, magnets and most of a
  timeline's icons, and an icon whose SVG is missing draws as nothing, silently.
- **The timeline's shortcuts are scoped `!Input`** (2026-10-03), like the
  global ones in `main.rs`: Q, W, M, P, N, A, B and Ctrl+C/X/V/D/A belong to a
  focused text field, not to the timeline.
- **Selections** (2026-10-03, wave 2). Ctrl+click toggles, Shift+click takes
  the run along a lane from the primary clip, a drag on empty lane space draws
  a rubber band (a click there still seeks), Ctrl+A takes every clip on an
  unlocked lane, Escape clears. `Editor::selected` stays the *primary* clip —
  the one the inspector shows — and `TimelineState::selection` the whole set;
  when another panel sets `selected` outside the set, the set becomes that
  clip. Moving, trimming and deleting a selection is one undo step
  (`batch::group_move` / `group_trim` / `group_delete`); with the magnet on
  the main lane is packed again, and a selected sound linked to a packed
  picture follows the picture, not the drag. The drag ghost is the drop's own
  placement (`batch::group_places`).
- **Clipboard.** Ctrl+C/X/V and Ctrl+D (`timeline/clipboard.rs`). Copying a
  clip copies its link partners; paste lands at the playhead on the clip's
  own lane, else the next free lane of its kind (never the main lane), else a
  new lane, and never overlaps. On the magnetic main lane it is a ripple
  insert at the nearest cut. Duplicate pastes right after the selection
  without touching the clipboard. A pasted title gets its own material
  (`text::commands::text_duplicate`), or editing one title would edit both.
- **A video's sound is inside the clip until "Detach audio"**
  (`timeline/links.rs`), CapCut's model on top of decision 0005: the import
  still makes one clip, the main lane draws its sound as a strip under the
  filmstrip, and detaching makes the 0005 pair — a linked clip of the same
  material on an audio lane — and sets the picture's volume to 0 in the same
  step, so a later Unlink leaves a silent picture and an independent sound
  rather than the same sound twice. Right-click a clip for split, delete,
  duplicate, copy, cut, paste, detach audio, link, unlink, reset speed and
  select all, and freeze frame.
- **Freeze frame** (right-click a video clip under the playhead;
  `timeline::freeze`, command `timeline_freeze_frame`). Not a speed-0 clip
  (`check_speed` refuses it) but a still: the frame at the playhead is
  decoded from the original file (software, off the UI thread, no project
  lock held) into a PNG under `paths::freeze_frames_dir()` —
  `$XDG_DATA_HOME/chukcut/freeze-frames`, not the cache, because the project
  references it. The edit is one `Composite`: `AddMaterial` (image), the
  `split_at`, right-to-left `MoveSegment`s of everything from the cut on the
  clip's lane and its linked lanes (`silence::cut::rippled_lanes`, the same
  rule as silence cutting: music on other lanes stays), then the still in the
  gap. The still carries the clip's crop and its *animated* transform at the
  playhead, no keyframes, and the clip's colour/effect extras. Undo leaves
  the PNG on disk on purpose, so redo still has it; nothing collects orphaned
  stills yet. Default length 3 s (`freeze::DEFAULT_FREEZE`); there is no UI
  to choose another.
- **Transitions draw as a badge over their cut**, as wide as the stretch they
  cover. Click selects it (Del removes it); its ends change the length
  symmetrically, clamped to `transitions::edit::allowed_duration`, through
  `transitions_retime` on release.
- **The primary clip shows its keyframe diamonds** along its bottom edge, one
  per instant across every property but `Volume`. Click selects one and seeks
  to it, a drag retimes every property keyed there (`MoveKeyframe`s, one
  step), Del deletes the instant.
- **Sound clips have CapCut's fade handles** at their top corners while
  selected or hovered, the faded corners shaded. They read and write the
  `Volume` envelope by the same pattern as the inspector's fade rows —
  `timeline/envelope.rs` holds a copy of `inspector::fades`/`fade_command`;
  change both together.
- **A drop from the media panel shows a line at its start time** (snapped like
  a dragged clip's head). It does not highlight a lane: which lane takes the
  tile depends on its kind, and only the media panel knows what is dragged.
  Exporting `assets::media::MediaDrag` would let the timeline draw the clip's
  real ghost through `on_drag_move::<MediaDrag>`.
- **A mouse wheel glides** (a third of the way per frame, on
  `request_animation_frame`); a touchpad's pixel deltas scroll directly.
- **To look at the app without touching the desktop**, run it on a private
  Xvfb with Mesa's software Vulkan:
  `Xvfb :77 -screen 0 1920x1080x24 &` then
  `DISPLAY=:77 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json ./target/debug/chukcut …`.
  `xdotool` and `import -window` on `:77` can then click, drag and capture
  freely. It is slow (pointer moves lag), and do not press Space there: the
  audio still goes to the real speakers.

## Motion tracking: broken follow links and deleting the tracked clip

- **A follow link is broken** when its tracking material is gone, or its
  named clip is gone *and* no clip of the tracked file is left on the
  timeline (`tracking::validate::link_status`; any clip of the file stands in,
  as `follow::target_segment` does at render time). `Project::validate` warns
  once per follower clip; the inspector's Tracking tab shows "Target missing"
  with only "Stop following". The link is kept, so undoing the delete brings
  the motion back.
- **Delete on a tracked clip asks first** ("Bake and delete" / "Delete" /
  "Cancel") when an overlay follows it by name, or follows a track of the
  same file (`validate::dependent_followers`, deliberately broad). "Bake and
  delete" is `tracking_bake_and_delete`: one `TrackingCommand::Composite` of
  every `bake` followed by the delete gesture's own commands run through
  `compose_edits`, `mirror_linked_edits` and `detach_broken_transitions` —
  one undo step. The bakes must come first: a follow is evaluated through the
  tracked clip, so after the delete there is nothing left to bake. Only the
  Delete key / menu / toolbar ask; **Ctrl+X does not** (it deletes plainly).

## The log file, and what an export writes into it

Until 2026-07-26 the app logged to stdout and nowhere else, which is fine when
it is started from a shell and useless when it is started from a launcher — and
a launcher is how everyone but us starts it. A broken export was reported and
neither the user nor anyone reading over their shoulder could say which of the
three frame paths had run, because the three produce the same file when they
work. That is the gap the file closes.

- **Where.** `$XDG_STATE_HOME/chukcut/logs`, or `~/.local/state/chukcut/logs`.
  Not under the cache root, deliberately: "clear cache" must not delete the log
  of the export somebody is about to ask about. One file a day,
  `chukcut-YYYY-MM-DD.log`, the newest seven kept.
- **What.** stdout keeps its old behaviour exactly — `RUST_LOG` still works and
  still defaults to `chukcut=debug,warn`. The **file** is fixed at
  `chukcut=info,warn` and deliberately ignores `RUST_LOG`, so a log somebody
  mails us has the same shape whatever their profile says. `RUST_LOG=error`
  silently emptying the file is the exact failure this exists to prevent.
- **Nothing is buffered.** Every event is one `write`, so the log ends at the
  last thing that happened rather than a few kilobytes before it, which is the
  only property that matters when the process died.
- Settings → Storage names the current file and reveals it in the file manager.
  `workspace_log_path` is the command; `revealItemInDir` needs no capability
  beyond the `opener:default` we already have.

Every export now writes two blocks, in `export::job`. The first, before the
first frame: output path, container, canvas, fps, codec, quality **and the
bitrate a rate-control mode that insists on a number would be given**, the
encoder and whether it is hardware or software, which of the three tiers was
chosen, **why the tier above it was not**, and the NV12 `y_stride`,
`uv_stride`, `uv_offset` and `total_bytes`. The second, after the last frame or
the one that stopped it: frames written, wall clock, mean fps, the tier it
*ended* on, the strides the compositor really produced, and any mid-export
fallback with the frame number it happened at.

The strides are in there because a stride disagreement is invisible in every
other symptom except the picture, and the picture is the thing being described
over a support channel by someone who cannot run a debugger. The start block
prints what `Nv12Layout` says the size should be; the end block prints what the
first rendered frame actually had. Those two being different is a bug, and
nothing else in the system would say so.

## Traps that have already cost time

- **GPUI Component theme colours set through `Theme::global_mut` never reach
  the widgets.** A Button reads the resolved `theme.tokens`, and the tokens
  only follow `theme.colors` when the edit goes through `Theme::update`, which
  reconciles them. Edited in place, the primary Export button stayed white.
  `theme::apply` now uses `update`; also set `button_primary*`, which is its
  own colour, not `primary`.

- **A shortcut bound to a plain key fires while the user types.** GPUI matches
  a key binding before a text field sees the character, so "s" split the clip
  and Space played while typing in the asset search. Plain-key bindings take
  the context `!Input` (`main.rs`). The editor root takes focus back on every
  mouse down in the capture phase, so a click outside a field restores the
  shortcuts; a field under the pointer refocuses itself afterwards.

- **A field focused from a mouse-down handler loses focus again** unless the
  handler calls `window.prevent_default()`: the editor root tracks focus and
  takes it in the bubble phase, after the handler. The timeline's inline text
  box (double-click a title or caption, `timeline/inline_text.rs`) does this.
  Its "click elsewhere commits" uses `on_mouse_down_out`, because the field's
  `InputEvent::Blur` did not arrive for a click on the lanes (Xvfb,
  2026-10-03).

- **Do not drive the app on the shared desktop.** Other sessions run their own
  `chukcut` windows there, and one of them on top of yours swallows the
  clicks — `xdotool mousemove --window` moves the real pointer, so the click
  lands on whatever window is on top at that spot. Run UI checks on a private
  Xvfb display with lavapipe (`VK_ICD_FILENAMES=.../lvp_icd.json`; GPUI and the
  engine both render there, NVENC detection still works) and close stray
  instances with `xkill -id` on their windows: a PID from one tool call may not
  exist in the next call's PID namespace. Pick an unusual display number; a
  second session on the same Xvfb puts its window over yours.

- **The frontend suite's flakiness was vitest's 5 s default, not the
  components.** Eight tests across six files failed at 5.1–6.9 s, and the set
  changed between runs — App, the export dialog, the preview, the media
  library, settings. Every one of them passes with `--testTimeout=25000`.
  Mounting these trees in jsdom costs seconds by itself: whole files take
  17–103 s, and the suite spends more time in `environment` than in the tests.
  Anything that waits after a mount was racing the timeout. `testTimeout` and
  `hookTimeout` are now 20 s in `vitest.config.ts`, which is still far above
  what any of them need, so a genuine hang still fails. If you see a
  "flaky component" here, measure before believing it.

- **FFmpeg will demux a plain text file as video.** The `tty` demuxer matches
  on the extension alone — `.txt`, `.nfo`, `.asc` and friends — and reports an
  `ansi` "video" stream with a size and a frame rate, so a stray text file
  dropped on the media panel imported as a video material whose card could
  never render (verified: `ffprobe notes.txt` prints `Video: ansi, pal8,
  640x400`). `project::commands::is_text_art` refuses the tty/bintext family by
  format and codec name; `tests/media_import.rs` pins it, along with the other
  refusals — text bytes wearing `.mp4` fail at `avformat_open_input` with the
  path in the message, and a subtitle file is refused as "no video, image or
  audio stream" rather than imported as an empty card.

- **Do not describe your memory to a media driver. Let it allocate, and import
  what it gives you.** The rule that came out of a day on the preview's JPEG
  path, and it generalises past this one bug. If *we* allocate the surface, every
  fact about its layout — the modifier, the pitch, the plane offsets — is a claim
  we are making that some fixed-function engine may quietly disregard, and Intel's
  JPEG engine disregards `DRM_FORMAT_MOD_LINEAR` and reads the buffer as the
  Y-tiling it always uses. Nothing errors. The picture is a mosaic. If the
  *driver* allocates and we import its exported descriptor, there is no claim to
  be wrong about: the modifier, pitch and offset all come from the thing that will
  read them. `VK_EXT_image_drm_format_modifier` makes this cheap on the Vulkan
  side, and `render::dmabuf::import_plane` had already been doing it for decoded
  frames for a day before anyone thought to point it the other way.

  Two corollaries worth having in mind before the next one of these:

  - **A capability list will not save you.** `vaQuerySurfaceAttributes` for the
    JPEG encode entrypoint reports NV12, `DRM_PRIME_2` and a 16384 maximum, all of
    which the *broken* path satisfied. `VASurfaceAttribDRMFormatModifiers` is
    write-only, so there is nothing to query — and iHD ignores it on the write
    side too, returning `I915_FORMAT_MOD_Y_TILED` whatever you ask for. Only a
    known picture pushed through the real engine and checked row by row answers
    this, which is what both probes do.
  - **Write the thirty-line C probe first.** `libva` and Vulkan both answer these
    questions directly and in minutes; the same questions asked through
    `ffmpeg-next` and `wgpu` take a day and answer less. Full tables in
    `docs/research/preview-zerocopy-jpeg.md`, part two.

- **A GTK menu accelerator shadows typing, and a bare letter therefore cannot be
  one. This no longer applies, and it is here so nobody re-derives it.**
  `gtk_window_key_press_event` consults the window's `GtkAccelGroup` *before* it
  propagates the key to the focused widget, so registering `C` for Split Clip
  stopped the letter `c` reaching a text field, and registering `Delete` stopped
  it deleting characters. It bit the clipboard items worst. While
  Cut/Copy/Paste/Select All were permanently disabled they could safely carry
  `Ctrl+X`/`C`/`V`/`A`, because `gtk_widget_can_activate_accel` refuses an
  insensitive widget and reports the key unhandled, so it fell through to the
  webview and text editing kept working. The moment they became clickable that
  stopped being true: an enabled item with `Ctrl+C` in the accel group takes the
  key away from every text field in the app, and copy in the project-name box
  broke as soon as a clip was selected. The bar therefore spelled its keys into
  the labels — `Copy (Ctrl+C)`, `Split Clip (C)` — and registered nothing.

  **The accel group went with the native menu.** The bar is drawn in the webview
  now (`decisions/0006-in-app-menu-bar.md`), there is no `GtkAccelGroup` in play,
  and a keystroke reaches the focused element the way it does in a browser. So
  `Ctrl+C` can be bound normally, the labels are plain again — the key is drawn
  in its own column from `Item::accelerator` — and the only rule left is the
  ordinary one: **one handler per key.** Two `keydown` listeners claiming
  `Ctrl+C` both fire, which is why the bindings still live next to the thing they
  act on (`Timeline.tsx`, `Preview.tsx`, `App.tsx`) and the bar only advertises
  them. `menu.rs`'s `every_key_the_app_binds_is_advertised_by_the_item_that_shares_it`
  holds the advertisement to the binding, and
  `workspace/lib/menu.ts::acceleratorAction` owns the five keys — Ctrl+I, Ctrl+Q,
  Ctrl+=, Ctrl+-, Ctrl+0 — that nothing else in the app binds because the native
  menu used to bind them itself.

- **An undecorated GTK window has no resize border, and nothing tells you.**
  `decorations: false` is what makes the menu bar themeable, and on GTK the
  resize frame *is* the decoration: with it gone the window manager has nothing
  to hit-test, so the window can be moved (Tauri's `data-tauri-drag-region`) and
  never resized. There is no error and no warning — you find out by trying to
  drag a corner. `src/app/components/ResizeEdges.tsx` puts eight transparent
  grips around the window that call `startResizeDragging`, which is the same
  thing a GTK client-side-decorated app does, and it needs
  `core:window:allow-start-resize-dragging` in `capabilities/default.json` or it
  is silently rejected at the boundary. The other four window permissions there
  are the same story for dragging, minimising, maximising and closing: none of
  them is in `core:default`, because a decorated window never asks for them.
- **A busy machine is not a slow machine, it is a different machine.** The same
  suite, same binary, same commit, measured minutes apart: at load 2 a whole
  preview frame is 25 ms; at load 44 it is 258 ms. Not 2× — **ten times**. Worse,
  it is not uniform, so ratios distort too: at load 44 the export's tier 3 came
  out *slower* than tier 2, which is impossible on the mechanism. This is why
  `chukcut-bench` exits rather than measuring above a load average of 4, and why
  it prints the load before and after and a worst/best spread on every row. Check
  `/proc/loadavg` before believing any performance number on this machine, and if
  you have used `--force`, do not put the result in this file.
- **A decode number without a bitrate next to it is not a number.** The first
  version of the benchmark's fixture generator used `noise=alls=10`, which
  produced a near-incompressible **30 Mbit/s** 1080p30 clip against the 1.5 Mbit/s
  of the real footage every earlier measurement used. That single difference
  **reversed a conclusion**: on the noisy clip, hardware decode into system memory
  measured 1.8× *faster* than software, because software entropy decoding scales
  with coefficient count and a fixed-function block barely notices. Re-run at a
  realistic 4–11 Mbit/s, the two are a wash — which is what this document says
  everywhere else. The fixtures are now light on noise and every decode row prints
  its clip's bitrate.
- **A benchmark group is affected by what ran before it in the same process.**
  Observed at `fc15c71`, when the export ran last after a minute of software
  decoding had the CPU and iGPU hot: tier 3 measured 34 fps against tier 2's 55,
  and the tell was that the *decode* stage inside it had gone from 10.4 to 19.4 ms
  per frame — the one thing the tier flag cannot touch. The iGPU shares its power
  and thermal budget with the CPU cores. Read *ratios* from a full run and take
  *absolute* figures from `--filter <group>`. This is also the argument for the
  per-stage breakdown: a single fps figure could not have been diagnosed.
- **`sws_getContext` ignores the file's colour tags.** A scaler built from
  formats and sizes alone converts YUV to RGB with `SWS_CS_DEFAULT`, which is
  BT.601, on a BT.709 file, forever, silently. FFmpeg's own `scale` filter sets
  it from the frame, so our output and `ffmpeg`'s disagreed by an amount small
  enough to read as rounding. Fixed in `decoder.rs::apply_colour`; the full story
  is under "Hardware decode through the compositor" above. Assume any new
  swscale context has the same defect until it calls
  `sws_setColorspaceDetails`.
- **The Khronos validation layer segfaults this driver on a DMA-BUF image
  import.** `vkBindImageMemory` through `VkLayer_khronos_validation` into
  `libvulkan_intel`, on the first hardware-decoded frame the compositor imports —
  signal 11, no message, no wgpu error, nothing in the log. wgpu turns that layer
  on for debug builds, so it killed `cargo test --test compositor` while the same
  code in release was fine and verified correct. `render::context::instance_flags`
  now leaves the layer off unless `WGPU_VALIDATION=1`; wgpu-core's own validation
  is unaffected and still catches our mistakes with a readable message. If you
  ever need the layer back, run with `CHUKCUT_DECODE=software` as well.
- **A hardware decoder does not report itself as one until it has decoded a
  frame.** `VideoDecoder::is_hardware` is set from the first frame that comes
  back as a surface, so a provider that branches on it takes the copying path for
  frame zero — and *caches the copy*, so the next request for the same instant is
  answered from it. `provider.rs` branches on `acceleration()` instead, which
  reports the request before the first frame and the truth after it. This cost a
  measurement run that reported "the provider did not import a surface" for every
  file on a machine that imports perfectly well.
- **`pnpm tauri dev` builds Rust unoptimized.** The preview's JPEG encoder
  needed ~130 ms per frame at `opt-level = 0` and ~7 ms in release, which made
  playback look fundamentally broken when it was merely unoptimized. `Cargo.toml`
  now sets `opt-level = 1` for our crate and `3` for dependencies, but the
  release build is still the one to judge by. Less of a cliff now that the
  encode is on the GPU and libjpeg-turbo is a C library compiled optimized
  either way — but compositing and the colour conversion are still ours.
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
- **`avcodec_find_decoder` does not return the decoder that can drive the GPU.**
  On an ordinary Ubuntu build, `avcodec_find_decoder(AV_CODEC_ID_AV1)` returns
  **`libdav1d`**, which has no hardware support at all. Attaching a VAAPI device
  to it does nothing observable: `get_format` is never offered
  `AV_PIX_FMT_VAAPI`, every frame decodes on the CPU, and the code believes it is
  on the GPU. The native `av1` decoder is the one that works, and FFmpeg's own
  CLI says so out loud — *"Selecting decoder 'av1' because of requested hwaccel
  method vaapi"*. `media/hwdecode.rs::hardware_decoder` walks `av_codec_iterate`
  and picks by `avcodec_get_hw_config` rather than by name. The same shape of
  trap exists for VP9 (`libvpx-vp9` versus `vp9`). Assume it for every codec.
- **A hardware *decoder* being in the FFmpeg build means nothing either**, for
  the same reason as the encoder below. `media/hwdecode.rs::capabilities` probes
  by actually decoding one embedded 320×240 frame per codec — the bitstreams are
  in `media/probe_streams/`, 3.3 KB in total, and are fed to the decoder
  directly so no demuxer or temporary file is involved. Result cached for the
  process.
- **A hardware encoder being in the FFmpeg build means nothing.** `av1_vaapi` is
  in every modern build, and this chip decodes AV1 and cannot encode it —
  `vainfo` lists `VAProfileAV1Profile0` for `VAEntrypointVLD` only. `h264_qsv`
  is in the build too and QSV cannot open a device here at all. So
  `export/hwaccel.rs` answers "does it work" by opening the device and encoding
  a 320×240 frame, not by looking. The result is cached for the process; the
  probe costs about a tenth of a second per encoder.
- **VAAPI rate-control modes are the driver's, not FFmpeg's.** `rc_mode=CQP`
  opens fine on the Intel iHD driver and fails on several AMD ones, and there is
  no way to ask which are supported short of trying. `encoder.rs` walks a ladder
  — constant quality, then VBR, then CBR, then the driver's default — and opens
  with the first rung that works. A bitrate has to come with the fallback rungs,
  because the user's request was a CRF and a driver that refused constant
  quality needs a number.
- **Do not set `bit_rate` on a VAAPI context in CQP mode.** It is treated as a
  target and the QP is ignored, which is how a hardware export comes out at a
  bitrate nobody asked for. The ladder's first rung deliberately leaves it zero.
- **Never hold a lock across a rayon dispatch.** Not "try not to" — it is a
  deadlock, twice over, and both were observed here. A rayon worker that blocks
  inside a parallel iterator runs *other jobs from the pool* while it waits, so
  it can steal a job that wants the lock it is holding; and a non-rayon thread
  that dispatches under a lock waits for a worker the pool may never free,
  because the pool is full of jobs waiting for that lock. The symptom is every
  thread in `futex_wait` with no work in flight, which reads as a lost wakeup
  and is not one. The corollary is just as important: **do not block a rayon
  worker on I/O or a condition variable either** — a pool whose workers are
  parked is a pool that cannot run the work somebody is waiting for. Full
  account under "The hang that was not the device".
- **Never open a GPU or a VAAPI device.** `modules::gpu` owns one of each and
  hands out references; the two constructors are crate-private and called from
  there and nowhere else. Concurrent Vulkan instances crash this driver,
  `vaInitialize` costs tens of milliseconds, and a VAAPI driver has a finite
  number of contexts. Both constructors log an error if a second one is ever
  created in a process — `RenderContext::open` counts live devices,
  `VaapiDevice::open` counts opens — which is the cheapest available tripwire
  for whoever adds one by accident. See "One GPU device and one VAAPI device"
  above for what two of them measured, and for the bugs that sharing exposed.

## The effect runtime (Phase 3), measured

Built 2026-07-26. `src-tauri/src/modules/effects/`, written up in
`docs/research/effect-runtime.md`.

**An effect authored in their format renders.** 45 tests in
`modules/effects/`, including a two-pass chain on the real GPU that samples
`share://input.texture`, ping-pongs through a `.rt` target, and has its uniforms
driven from a Lua script through the `Amaz` API. The fixture is
`modules/effects/fixtures/tint/` and it was written here.

**The shader pipeline works, and it was supposed to be the risk.**
`rust-crate-survey.md` §6b said naga's GLSL frontend rejects every shader in
the corpus and proposed a four-step replacement. That pipeline — our ES1→450
rewriter, then glslang, then `spirv-webgpu-transform`, then naga spv-in —
compiles **24 of 24** real corpus shaders to validated WGSL, including a
`sampler2D` passed as a function parameter and `gl_FragData[]` multi-output
shaders. The survey's estimate of "an afternoon" for the conformance run was
right.

**What actually blocks a downloaded effect is the binary asset container.**
Structural assets (`.xshader`, `.material`, `.rt`, `.scene`, `.mesh`) ship in
two interchangeable encodings and only the YAML twin is read here. That twin is
about **2% of shipped assets** — 4 of 263 `.material`, 4 of 213 `.xshader`. So
an effect authored in the format renders (there is one in
`modules/effects/fixtures/tint/`, and a test that produces pixels from it), and
every package in the surveyed cache is refused at its first `.xshader` with a
message saying exactly why. Decoding that container is the next piece of work
and it has an oracle for every field, which is the strongest position to do it
from.

Traps found building it, both of which produce a *compile error* rather than a
wrong picture — which is the redeeming property of this whole approach:

- **`sample` is an ordinary identifier in GLSL ES 1.0 and a keyword in 450
  core.** A Sobel filter in the corpus declares `vec3 sample;`. There is a
  family of these: `layout`, `buffer`, `shared`, `uint`, `smooth`, `centroid`.
- **A shader may overload a builtin, and renaming it breaks the builtin's
  callers.** `LumiGrain` defines its own `float mix(float, float, float)` and
  calls the builtin `mix(vec3, vec3, float)` in the same file. The reserved-word
  rename therefore has to skip declarations followed by `(`: a function
  overloads, only a variable shadows.

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

## Talking-head tools: silences, fillers, voice cleanup, loudness (2026-10-03)

Engine: `modules/silence`, `modules/voice`, `modules/loudness`. UI: the
Audio tab's Normalize loudness / Reduce noise / Remove silences sections
(`editor/inspector/voice.rs`), the review panel (`editor/silence.rs`), and a
Loudness row in the export dialog (`editor/export/loudness.rs`).

What works, verified:

- **Silence detection** on a 10 ms RMS envelope (threshold, shortest pause,
  padding; padding only on the side that touches speech). A suggested
  threshold from the recording's own floor. Optional "voice" mode adds
  RNNoise's per-frame voice probability. Re-detection runs on the stored
  envelope, so the sliders are live. `tests/talking_head.rs` finds a 1.5 s
  pause in a generated take within 20 ms of where it is.
- **Cutting** (`silence::cut::remove_ranges`): one `Composite` built by
  running `split_at`, `RemoveSegment` and leftward `MoveSegment`s on a copy.
  Linked picture and sound are cut at the same instants, each kept pair gets
  its own link group, fades crossing a cut stay on their piece, speed maps
  source to timeline time, slivers under one frame go with their cut. One
  undo restores the take (unit and integration tests).
- **Voice cleanup**: denoise is rendered through `nnnoiseless` (RNNoise) into
  `cache/voice/<hash>.wav` with the network's one-frame delay removed
  (correlation test), normalise is a measured gain capped by true peak. Both
  sit in one block in `MaterialPool::extras` and both mixers read it through
  `voice::effective_source`; the integration test checks the preview plan and
  the export resolver pick the same file. Export re-renders a missing cache.
  Why RNNoise: `docs/decisions/0015-voice-cleanup-engine.md`.
- **Loudness target on export** (`ExportOverrides::loudness_target`): EBU R128
  via the `ebur128` crate, gain, look-ahead true-peak limiter at −1 dBTP, a
  second measure-and-correct pass. FFmpeg's `ebur128` measured the test
  exports at **−14.0 and −23.0 LUFS** for targets −14 and −23.

Rough or missing:

- **"Keep everything in sync"** (the panel's default, glue 2026-10-03):
  `remove_ranges_in_sync` cuts the same stretches out of every unlocked lane.
  A title or caption a cut runs through is shortened, and a caption's words
  are re-timed through the closed-up timeline, so it stays one caption on
  its words; music and overlays are split at the cut's edges and the inside
  removed; a clip wholly inside a cut goes; locked lanes stay. Off, only the
  clip's own and linked lanes ripple, as before. One undo step either way.
- **Filler words read the captions.** `captions::words::CaptionWordTimings`
  implements `silence::filler::WordTimings`: caption words (timeline time)
  through the cut clip's placement and speed into its source time.
  `AppState::new` registers it. Captions from an `.srt` have no word times
  and give no transcript — estimating them would cut beside the filler.
  `captions::TimedWord` is the one word type.
- Normalize and Reduce noise act on one clip. After a silence cut the pieces
  share the cleanup block (splitting clones `extras`) but a normalise applied
  afterwards lands on the selected piece only.
- Denoise strength is three steps, because every strength is a full render.
- The mix is clamped by `AudioMixer::finish` *before* the loudness target, so
  a mix that already clips is normalised clipped.
- No Silero VAD and no DeepFilterNet; both are documented as upgrade paths.

Trap: while several agents test on one machine, an app instance that dies
with exit 143/144 and nothing in its log was most likely killed by another
agent's `pkill chukcut`. Run your copy under another process name
(`cp target/debug/chukcut _scratch/ccsil; exec -a ccsil ./_scratch/ccsil`).

## Cloud integrations with the user's own key (2026-10-03)

Settings → Accounts holds the user's keys for ElevenLabs, fal.ai, Pexels,
Pixabay, Freesound, DeepL and any OpenAI-compatible server (list, add, edit,
test, delete; keys in `secrets.toml`, 0600, shown by their last four
characters). On top of it:

- **Audio tab:** Text to speech (ElevenLabs voices with previews, model,
  stability, similarity, speed; word timing becomes captions at the
  playhead; or an OpenAI-compatible `/audio/speech`), Sound effects and Music
  (ElevenLabs).
- **Stock tab:** Pexels and Pixabay videos and photos, Freesound sounds with
  CC BY-NC hidden unless asked for. A "+" downloads with an `asset.json` and
  puts the item at the playhead.
- **Media tab → AI tools:** fal.ai remove background (BiRefNet, ProRes 4444
  with alpha), upscale ×2 (SeedVR2), smooth motion (RIFE). The price comes
  from fal's pricing API before the Run button is live; progress, cancel; the
  result goes on a lane above the clip or replaces it.
- **Captions tab → Translate:** DeepL or an OpenAI-compatible chat model, onto
  a new lane above the originals, one undo step.
- **Export:** a licence summary at the top of the dialog (non-commercial,
  share-alike, needs credit, unknown terms) and `<name>.credits.txt` beside
  the video when anything needs credit. Decision 0016.

Every provider is tested against a mock HTTP server
(`cloud::http::test_server`); no real key was ever used. For looking at the
panels without keys, a Python stand-in for all six vendors was used: write
`accounts.toml` with each account's `base_url` pointing at it (the field is
stored for every kind, only the OpenAI-compatible form shows it) and run the
app with its own `XDG_CONFIG_HOME`.

Not verified against the real services (no keys exist): the exact fal storage
upload (`rest.fal.ai/storage/upload/initiate?storage_type=fal-cdn-v3`, taken
from fal's own JS client), the fal model input enums (taken from the model
pages on 2026-10-03), ElevenLabs `eleven_v4*` model ids, and Pexels/Pixabay
answer shapes beyond what their docs show.

Rough or missing:

- **No job journal and no per-provider concurrency limit.** A fal job lives
  in the app's memory; quitting while it runs loses the result (fal may still
  bill it). Research §7.4 describes both.
- **The whole source file is uploaded** to fal, not the clip's trimmed range,
  and the estimate is for the whole file.
- **Alpha from "Remove background" depends on the decoder keeping it.** The
  result is ProRes 4444; whether the compositor draws its alpha was not
  checked.
- Auditions ("Play" on a voice or a sound) open their own output stream next
  to the timeline's audio engine.
- No ElevenLabs voice changer, dubbing, Stable Audio, Higgsfield or Freesound
  OAuth originals (research waves 7–8).
- Generated files are not offered to move into the project folder when it is
  first saved.

## The research

The documents under `docs/research/` were produced by dedicated agents and are
worth reading before making an architectural decision:

- `media-stack-options.md` — GStreamer/GES vs MLT vs building our own. Verdict:
  keep the custom stack; the frameworks give away the timeline we already have
  and withhold the hardware decode and proxy machinery we lack.
- `rust-crate-survey.md` — per-subsystem crate recommendations, with maintenance
  signals. Notably: hardware encode is *not* blocked by our FFmpeg version, and
  naga cannot consume the GLSL ES dialect CapCut ships. Its §2 was right: the
  wrapper took a day and no new dependency.
- `hardware-decode.md` — the VAAPI decode path, what this chip decodes and what
  a decoded frame costs three different ways. Read it before touching
  `media/` or before wiring the compositor to a decoded surface: it carries the
  measurements above, the `avcodec_find_decoder` trap, the evidence that a
  two-plane NV12 DMA-BUF import works on this driver (and therefore that a VAAPI
  VPP pass is *not* needed), and the exact patch `render/` needs.
- `zero-copy-encode.md` — written straight after building the VAAPI encoder,
  about the copy that is still in the path. It corrects an assumption the crate
  survey leaves implicit: the survey researched DMA-BUF *import* for decode, and
  encode needs *export*, which wgpu does not offer at all. Read it before
  starting step 2 of decision 0002.
- `vaapi-jpeg-preview.md` — hardware JPEG for the preview frame server. Read it
  before touching the preview encode path, and especially before using swscale
  for a colour conversion or trusting a YUV routine's default range. Its real
  conclusion is that the encoder is no longer where the time goes.
- `draft-format.md` — a full specification of CapCut's project format, with
  units and coordinate conventions established by evidence.
- `effect-runtime.md` — what happened when the effect format was actually run:
  the shader pipeline's conformance result on real shaders, the two GLSL
  dialect traps, the corrections to the crate survey's §6b, and the exact
  compositor patch the effect chain needs. Read it before touching
  `modules/effects/` or before estimating Phase 3.
- `effect-package-format.md`, `engine-symbols.md`, `ui-inventory.md` — how
  CapCut is built.
