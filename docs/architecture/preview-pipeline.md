# Preview pipeline

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

**2. Custom URI scheme serving compressed frames.** Rust renders at proxy
resolution, encodes JPEG, and serves bytes over a registered protocol. The
webview fetches them like images. At 960×540 quality 80 a frame is ~120 KB, so
30 fps is ~3.6 MB/s — trivial. Encode cost with the pure-Rust `jpeg-encoder`
crate is ~2–4 ms per frame on one core, and it parallelizes across frames.

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
   render graph ──────► RGBA texture (proxy res)
        │
        ▼
   readback to CPU ──► jpeg-encoder ──► frame cache (ring buffer)
                                              │
        chukcut-frame://preview/<session>/<n> │
                                              ▼
                                   webview <canvas>
```

### Sessions

A preview session is created when playback starts or the playhead moves, and
carries the proxy resolution and a monotonic id. The session id is in the URL so
a stale frame request from a previous seek cannot paint over the current one.

### The ring buffer

Rendered frames land in a fixed-size ring (default 90 frames ≈ 3 s at 30 fps).
Playback reads ahead into it; scrubbing invalidates it. The ring is what turns
render jitter into smooth playback — the same reason a terminal emulator keeps
a scrollback buffer instead of re-deriving the screen.

### Scrubbing vs playback

Two different jobs with different rules:

- **Scrubbing**: latency matters, throughput does not. Render exactly the frame
  under the cursor, at reduced quality, cancel any in-flight render for a
  superseded position.
- **Playback**: throughput matters, latency does not. Render ahead into the
  ring, drop frames if the renderer falls behind rather than stretching time,
  and keep audio as the clock master.

### Audio

Audio is not part of the frame pipeline. It is mixed in Rust and played through
the system device, and its position is the authority for the playhead. Video
frames are matched to it and dropped when late — the standard arrangement,
because humans forgive a dropped frame and never forgive a stutter in audio.

## Proxy resolution

Chosen from the canvas so the aspect ratio matches exactly, capped on the long
edge:

| Canvas long edge | Preview long edge |
|---|---|
| ≤ 720 | native |
| ≤ 1080 | 720 |
| ≤ 2160 | 960 |
| > 2160 | 1080 |

The user can override it; a "full quality preview" toggle just raises the cap
and accepts the frame rate hit.
