# Preview pipeline

> **Webview era.** Written for the Tauri + React shell, which was removed on
> 2026-10-02 (decision 0011). Kept for its reasoning; the code paths it names
> under `src/` and `src-tauri/` no longer exist.

This is the one genuinely hard problem in putting a video editor in a webview,
and it is where naive Tauri video editors die.

## The problem

The compositor produces frames in Rust, on the GPU. The user looks at them in a
webview. There is a process boundary in between, and video is a lot of bytes:

| Resolution | Bytes/frame (RGBA) | At 30 fps |
|---|---|---|
| 1920×1080 | 8.3 MB | 249 MB/s |
| 1080×1920 | 8.3 MB | 249 MB/s |
| 960×540 | 2.0 MB | 62 MB/s |

Sending that through `invoke()` means JSON, which means base64, which means
~33% inflation plus a full parse per frame on the UI thread. It does not work
at any resolution.

## The four options

**1. `invoke()` with encoded frames.** Dead on arrival, see above.

**2. Custom URI scheme serving compressed frames.** Rust renders, encodes JPEG,
and serves bytes over a registered protocol. The webview fetches them like
images. At 960×540 quality 80 a frame is ~120 KB, so 30 fps is ~3.6 MB/s —
trivial.

The encode cost was the one thing this option could have died on, and the
estimate here was wrong for a while: `jpeg-encoder` is ~2–4 ms at 960×540 but
**31 ms at 1080×1920**, against a 33 ms budget. That is why the preview ran
reduced for a while. It is now ~6 ms on the iGPU's fixed-function JPEG encoder,
with libjpeg-turbo at ~10 ms as the fallback. See
`docs/research/vaapi-jpeg-preview.md`.

**3. Hole punching.** A native wgpu surface positioned underneath a transparent
region of the webview. This is what CapCut Desktop does and it gives full
resolution with zero copies. On X11 it is workable; on Wayland, subsurface
positioning under a GTK webview is fragile, and it breaks every time the window
manager has an opinion.

**4. Rendering inside the webview.** Compile the compositor to wasm32 and draw
straight to a canvas via WebGPU, decoding with WebCodecs. This is what CapCut
Web does. It requires a Chromium-class webview — on Linux, Tauri uses
WebKitGTK, where WebGPU and WebCodecs are incomplete.

## The decision

**Option 2 now, option 3 later if the preview quality ever becomes the
complaint.**

Full resolution is an export concern, and export never touches the webview — it
runs headless in Rust. The preview only has to be good enough to make editing
decisions on, which is exactly what every NLE's proxy mode is for.

The engine boundary is identical under options 2 and 3: the compositor renders
into a texture either way. Switching later replaces the sink, not the renderer.

## How it works

```
 playback clock (Rust)
        │  "I need frame N at timeline position t"
        ▼
   render graph ──────► RGBA texture (the panel's size)
        │
        ├─ if the hardware allows: a render pass writes NV12 into the two planes
        │  of a VA surface the media driver allocated, and the JPEG encoder reads
        │  that surface. Nothing crosses the bus.
        │
        └─ otherwise: readback to CPU ──► RGBA→NV12 on rayon ──► upload
        │
        ▼
     JPEG encode ──► frame cache (ring buffer)
   (VAAPI, else libjpeg-turbo)
                                              │
        chukcut-frame://preview/<session>/<n> │
                                              ▼
                                   webview <canvas>
```

Which of the two branches a machine takes is decided **once per process, by
trying it**: `preview::vasurface::encoder_reads_its_own_surface` draws a row ramp
through the real encoder and checks every row. It is worth 2.7–2.9× on a whole
frame and the reasoning is in `docs/research/preview-zerocopy-jpeg.md`. The
choice is made *before* the frame is composited, because the zero-copy
destination is device-local and there is no way back to the CPU from it.

### Sessions

A preview session is created when playback starts or the playhead moves, and
carries the render resolution and a monotonic id. **Changing the resolution is
therefore a new session** — the ring holds encoded frames at the old size and a
frame URL is answered from the ring without anything looking at how big the
picture in it is — which is why a panel resize and the re-render after a
degraded playback both supersede. The session id is in the URL so
a stale frame request from a previous seek cannot paint over the current one.

### The ring buffer

Rendered frames land in a fixed-size ring (default 90 frames ≈ 3 s at 30 fps).
Playback reads ahead into it; scrubbing invalidates it. The ring is what turns
render jitter into smooth playback — the same reason a terminal emulator keeps
a scrollback buffer instead of re-deriving the screen.

### Scrubbing vs playback

Two different jobs with different rules:

- **Scrubbing**: latency matters, throughput does not. Render exactly the frame
  under the cursor, at the *best* quality — the moment the user lets go, that
  frame is the one they sit and look at — and cancel any in-flight render for a
  superseded position. A request for a frame that is already being rendered is
  refused rather than queued behind it: the webview asks again while it waits,
  and each retry used to start the same expensive render a second time.
- **Playback**: throughput matters, latency does not. Render ahead into the
  ring, drop frames if the renderer falls behind rather than stretching time,
  and keep audio as the clock master.

### Audio

Audio is not part of the frame pipeline. It is mixed in Rust and played through
the system device, and its position is the authority for the playhead. Video
frames are matched to it and dropped when late — the standard arrangement,
because humans forgive a dropped frame and never forgive a stutter in audio.

## Render resolution

**The panel decides it.** The player measures its own canvas element — CSS size
times `devicePixelRatio` — and sends that to Rust through `preview_viewport`,
debounced 180 ms because a window drag is one layout per pointer event. The
frame is composited, read back and encoded at that size and displayed 1:1.

This is the difference between rendering what the screen shows and rendering
what the project is. A 1920×1080 project in the default 700 px-wide player
panel is 2.07 megapixels of work for 0.28 that can be displayed — **7.5× the
pixels**, on every stage of every frame, thrown away by a CSS downscale. On a
2× display the same panel is 1400 columns and asks for 53% of the canvas; that
is what `devicePixelRatio` is for. Zoom and fullscreen need no separate rule:
both change the displayed size, and the displayed size is what is measured.

Three things bound it, in this order:

| | |
|---|---|
| Never above the canvas | Rendering above the source invents no detail and costs a whole frame budget to do it |
| The table below, or `settings.preview_max_edge` | A cap on the long edge, from the canvas alone |
| The panel | Scaled down to fit, aspect preserved, rounded **down to even** |

| Canvas long edge | Cap |
|---|---|
| ≤ 1920 | native |
| > 1920 | 1920 |

`settings.preview_full_quality` overrides all of it and renders the canvas.

Even dimensions are not a rounding detail: NV12 cannot represent an odd one, so
`vaapi::size_is_encodable` refuses it and every frame falls back to
libjpeg-turbo — 31 ms against 6.2 ms on a 1080×1920 frame.

## The quality ladder

Playback may give up resolution and JPEG quality to keep up. **A paused frame
may not.** That asymmetry is the whole design, and it is the owner's
requirement stated as code: the frame he stops on has to look like the project.

| Rung | Size | Quality | 
|---|---|---|
| 0 | the panel size | `settings.preview_quality` (88 by default) |
| 1 | ¾ (56% of the pixels) | −8 |
| 2 | ½ (25% of the pixels) | −18, floor 60 |

It steps **down** after three frames over budget, or immediately on a dropped
frame, and **up** after three quiet seconds — deliberately asymmetric, because
a stutter is noticed at once and the softness of a lower rung only if you look
for it, and because symmetric thresholds oscillate between two rungs, which
reads as the picture breathing.

Pausing after a run that used a lower rung supersedes the session, which empties
the ring, and re-renders the frame under the playhead at rung 0 and
`SCRUB_JPEG_QUALITY`. A run that stayed at rung 0 keeps its ring, because the
picture in it is already right and a resume needs the read-ahead.

The rung is on the `preview::stats` summary line — `rung`, `ladder`,
`render_width`, `render_height`, `render_quality` — so a soft preview is
diagnosable from a log file rather than from a screenshot.

There used to be a *second*, smaller size used only while playing, and it was
removed when the JPEG encode moved onto the GPU. This is not that: it was
unconditional, and this is a response to the frame budget actually being
missed.
