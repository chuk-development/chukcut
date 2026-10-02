# Kiru teardown — what they use, and why their scrubbing is smooth

Reverse-engineered from the shipped Linux build `Kiru-0.5.4-linux-x86_64`
(`releases.getkiru.app`, sha256 verified against the AUR `kiru-bin` PKGBUILD).
Native, not stripped, so the crate symbols are all readable. This is a factual
teardown to guide chukcut stack decisions.

## Verdict up front

Kiru is **not** a webview app. The desktop build is a single 88 MB native Rust
binary that renders its whole UI on the GPU with Zed's toolkit. Their timeline
and scrubbing are smooth because the video decoder and the UI share one GPU
device in one process, so a preview frame never leaves the GPU and never gets
serialized. chukcut's engine is already fast; the loss is the last mile through
the Tauri webview.

## Kiru desktop stack (measured, not guessed)

| Layer | Kiru | Evidence in binary |
|---|---|---|
| Language | Rust, one native ELF, no webview | `usr/bin/kiru-next`, no asar/pak/webkit |
| UI toolkit | **GPUI** (Zed) + **gpui-component** (Longbridge) | `gpui` 13.8k syms, `gpui_component` 4.1k, `assets/vendor/gpui/` |
| Layout / text | `taffy` (flexbox), `cosmic_text` | symbols |
| GPU | **wgpu**, via a custom `gpui_wgpu` backend | `wgpu_core`/`wgpu_hal`, `gpui_wgpu` 219 |
| Windowing | direct wayland / x11 / xcb (no GTK webview) | `wayland` 2.5k, `x11` 1.3k, `xcb` |
| Video player | own crate `wgpui_video_player` | `media_engine`, `media_surface`, `MediaController` |
| Frame transport | wgpu textures via **dmabuf**, zero readback | `dmabuf` 47, `CachedPreviewFrame` |
| Scrub cache | thumbnail + scrubber frame cache | `scrubber_frames`, `thumbnail_scrubber`, `frame_cache` |
| Decode | FFmpeg native (`ffmpeg_sys`, bundled `libav*.so`) + hwaccel vaapi/cuda | `ffmpeg_sys`, `h264_vaapi`, `cuda` |
| Export/probe | also shells to system ffmpeg/ffprobe | `KIRU_FFMPEG_PATH`, `KIRU_FFPROBE_PATH` |
| DB / project | `sqlx` + sqlite | `sqlx_sqlite` 1.3k |
| Net | `zed_reqwest` + tokio; transcription is cloud | pricing is hours/month |

The web version at kiru.app/app is a separate Next.js build that uses WebCodecs
(`VideoDecoder`/`VideoEncoder`), OffscreenCanvas, WebGL2 and the `mediabunny`
JS media library. Different codebase, do not confuse it with the desktop app.

## The media engine (the interesting part)

Symbols spell out how the player works:

- `media_engine::clock`, audio as clock master, `PreviewAudioController` /
  `PreviewAudioRenderer` — audio drives timing, video chases it.
- `compositor_timing` with `requested_source_pts / normalized_source_pts /
  timeline_pts / timeline_frame_index` — one clean mapping from timeline time to
  source frame time.
- `preroll`, `seek`, `skip`, `flow` (`requested_secs` vs `decoded_secs`) — a real
  seek/preroll state machine, so a scrub request resolves to the nearest usable
  frame instead of blocking on a full decode.
- `media_surface::CachedPreviewFrame` + `scrubber_frames` / `thumbnail_scrubber`
  / `frame_cache` — decoded frames live as cached wgpu textures. Dragging the
  playhead samples the cache; it does not re-decode per pixel move.

## Why chukcut's timeline/scrub hurts and Kiru's does not

chukcut today: Tauri. The Rust engine decodes and composites well (README:
NV12 -> VA surface, dmabuf, 4.36 ms preview frame), then **encodes JPEG** and
ships it over a custom IPC protocol to the **webview**, which decodes the JPEG
and paints a canvas/DOM. Every preview frame crosses a process + serialization
boundary and is re-encoded and re-decoded. Timeline widgets are JS/DOM. That
round trip, not the decode, is the scrub latency. The JPEG step in chukcut's own
benchmark name is the tell.

Kiru: decoded NV12 -> wgpu texture (dmabuf, no readback) -> drawn by the *same*
wgpu context that draws the timeline. No JPEG, no IPC, no JS. Scrub = seek in
`media_engine` -> nearest texture from `scrubber_frames` -> draw.

Key point: Kiru did not solve a hard rendering problem. They chose a native
GPU UI toolkit (Zed's GPUI) instead of a webview, so preview and timeline sit in
one GPU context. That single decision is the whole advantage.

## Options for chukcut, ranked by payoff/effort

1. **Cheapest big win — kill the webview for the preview surface only.** Keep
   Tauri for the chrome, but render video into a native GPU surface (child
   window / GL area) composited with the webview via a transparent hole. Drop
   the JPEG-encode + IPC on the hot path entirely. The engine already outputs a
   dmabuf texture; draw it directly. Medium effort, removes the main scrub
   latency without a rewrite.

2. **Add a scrubber frame cache like Kiru.** A texture atlas of low-res frames
   keyed by pts (`scrubber_frames` equivalent). Playhead drag reads cached
   textures; full-res decode only on settle. Helps regardless of option 1.

3. **Do it like Kiru — GPUI rewrite.** UI in GPUI + gpui-component (both open
   source, the exact stack Kiru shipped). Biggest effort, but then preview,
   timeline and chrome all live in one wgpu context and the whole class of
   webview-boundary problems is gone. chukcut's engine can be reused as-is.

4. Keep audio as clock master (chukcut already does). Keep native FFmpeg +
   hwaccel (already does).

Recommendation: start with 1 + 2 to prove the scrub fix without betting the repo;
treat 3 as the real target if the webview keeps fighting the timeline.
