# Why the preview plays at 11–20 fps, measured

Written 2026-07-27, against `3440e48`. The question was a gap: `chukcut-bench
--filter preview-frame` says a whole 1920×1080 preview frame costs **10.7 ms**,
and the owner's playback was **11–20 fps against a 24 fps demand**, which is
50–90 ms a frame. Per-frame DEBUG lines showed composite 5–10 ms and encode
6–19 ms, which adds to far less than that. This document is where the rest of
the time goes.

**The short answer, and it is not what the existing numbers suggested:**

1. **Nothing is serialised that was supposed to be pipelined.** The render
   thread and the encode thread overlap exactly as `server.rs` says they do,
   while there is headroom. Measured: with headroom, 0 of 155 frames were
   encoded on the render thread and it was idle 88% of the time.
2. **The frame is not 10.7 ms. It is 15.8 ms on an idle machine and 58.7 ms on
   a busy one**, at 1080×1920 on real 1080p footage — and the existing 10.7 ms
   is a *serial latency* on a generated fixture measured at load 2.4, which the
   STATUS table quotes under the heading "Preview playback".
3. **The pipeline has almost no margin at native resolution, and margin is the
   whole thing.** At 1080×1920 it renders 58 fps at load 7 and 34 fps at load
   40. The moment the machine does anything else, it drops under the 24 fps
   demand, the ring empties, and every frame request then blocks for up to
   `FRAME_WAIT` = 60 ms. That is the "lahm".
4. **60–76% of the frame is spent carrying finished pixels to the JPEG
   encoder** — the readback, the CPU RGBA→NV12 pass and the upload — for a
   picture the compositor already had on the GPU and that is then displayed in a
   panel a fraction of its size.

## How to reproduce every number here

```bash
cd src-tauri
cargo run --release --example preview_pipeline -- \
    --clip /home/user/git/editing/footage/money_tool_30fps.mp4 \
    --canvas 1080x1920 --seconds 10 --fps 24

# capacity rather than compliance: demand more than the pipeline can do, so the
# rendered rate *is* the throughput instead of being clamped to the clock
cargo run --release --example preview_pipeline -- \
    --clip /home/user/git/editing/footage/money_tool_30fps.mp4 \
    --canvas 1080x1920 --seconds 10 --fps 60 --live-only

# the owner's clip, both canvases
cargo run --release --example preview_pipeline -- \
    --clip "/home/user/Sunil Gurjar, CFTe - In 1999, Warren Buffett gave a 1-hour masterclass explaining why most... [2070839377962696704].mkv" \
    --seconds 10 --fps 24
```

`--long-edge <n>` sets the reduced-proxy phase (default 960, `0` disables it),
`--canvas WxH` is repeatable, `--live-only` skips the serial control.

Two pieces of apparatus were added and neither changes pipeline behaviour:

- **`src-tauri/src/modules/preview/probe.rs`** — relaxed atomic counters on the
  paths that already take a mutex and copy megabytes. It counts **occupancy**,
  not duration, which is the only thing that can answer the serialisation
  question: two stages that each fill half of *one* thread are serial and their
  costs add; the same two on different threads overlap and only the larger
  counts. No per-frame timing can distinguish those.
- **`src-tauri/examples/preview_pipeline.rs`** — starts a real `PreviewServer`,
  plays real media at wall-clock speed, and stands in for the webview by turning
  every position event into a `chukcut-frame://` request on a pool the same size
  as the real protocol handler's.

**The instrumentation and the harness are on the `agent/preview-perf` branch and
also present, uncommitted, in the main checkout.** Nothing is committed.

## What the machine was doing, which decides how to read this

This machine was **never quiet**. Other agents were compiling throughout;
`/proc/loadavg` ranged from 7 to 40 and `chukcut-bench` refuses to report above
4. `docs/STATUS.md` records that at load 2 a preview frame is 25 ms and at load
44 it is 258 ms — ten times, not two — so **every absolute figure below is
inflated and is stamped with the load it was taken at.**

That turned out to be a feature rather than a defect, because the load *is* the
finding: the two loads bracket "works" and "does not work" for the same binary
and the same media, and the owner's report sits between them.

What survives the noise, and is what the ranking at the bottom rests on:

- **Ratios inside a single run**, especially the native-vs-reduced-proxy pair,
  which the harness measures back to back in one process.
- **Occupancy fractions** — which thread was busy and which was waiting.
- **Counts** — inline encodes, ring hits, blank responses, read-ahead lead.

## Each stage in isolation

`Compositor::render_frame` plus `encode_preview_jpeg`, one frame at a time with
nothing overlapping. This is the same measurement `chukcut-bench --filter
preview-frame` takes, on real media instead of a generated fixture.

**1080×1920 canvas, `money_tool_30fps.mp4` (1080×1920 H.264, 9.7 Mbit/s), load 10.0**

| stage | per frame | share |
|---|---:|---:|
| decode to texture (`SourceProvider::frame`) | 0.81 ms | 5% |
| composite (uniforms, draws, submit) | 0.48 ms | 3% |
| **readback** — 1.45 blocked in `device.poll` + 3.54 unpadding | **5.10 ms** | **32%** |
| **JPEG encode (VAAPI)** — 116 KB a frame | **5.73 ms** | **36%** |
| whole frame, serial | **15.82 ms** | = 63.2 fps ceiling |

and the JPEG measured stage by stage on the same composited frame:

| | per frame |
|---|---:|
| RGBA→NV12 on the CPU (rayon) | 1.81 ms |
| upload into the VA surface | 2.59 ms |
| the fixed-function encoder itself | 1.59 ms |

The same clip and canvas at **load 33** — the case that reproduces the
complaint:

| stage | per frame |
|---|---:|
| decode to texture | 2.76 ms |
| composite | 1.52 ms |
| readback (3.32 poll + 10.74 unpad) | 14.70 ms |
| JPEG encode | 28.10 ms — of which NV12 20.78, upload 9.35, encoder 5.98 |
| whole frame, serial | **58.73 ms** = 17.0 fps |

**1920×1080 canvas**, `bench_v2_h264_1920x1080.mp4`: 14.74 ms whole frame at
load 19 (decode 0.82, composite 0.47, readback 4.32, JPEG 7.33) and 26.81 ms at
load 7 (decode 1.23, composite 0.73, readback 6.59, JPEG 14.99). Two samples
minutes apart differing by 1.8× on a machine this busy: read that row as "about
15–27 ms", not as a figure.

**The owner's clip is not a 1080p workload.** It is **720×540** H.264 at
0.37 Mbit/s in a 3568-second Matroska with a third (MJPEG cover-art) stream.
Decoding it costs 1.0–2.4 ms and it plays comfortably: 25.1 fps against a 24 fps
demand at load 38, 240 of 240 requests served straight from the ring. Whatever
he is seeing at 11–20 fps is not that file at 720×540 — it is a 1080p project,
or that file on a 1080p canvas with the machine busy.

### Three stages that cost nothing, measured so nobody looks there again

| | per occurrence | share of wall clock |
|---|---:|---:|
| `Channel::send` for one position event | 0.037–0.094 ms | 0.09–0.54% |
| `FrameCache::insert` (ring mutex + condvar) | 0.006–0.18 ms | — |
| `serve_uri` on a ring hit | ~0.00 ms | — |

The IPC is not the problem, the ring is not the problem, and the protocol is not
the problem. **In every run, zero requests came back 410** — the session
machinery is not throwing frames away during steady playback.

## Do the stages overlap or serialise? Both, and the switch is the bug

**They overlap, by design and in fact, for as long as the encode fits.** With
headroom (1080×1920, demand 24 fps, load 10):

```
render thread   composite 14.9% (5.77 ms x155)  inline encode 0.0% (x0)  waiting 88.0%
encode thread   encoding  16.7% (6.46 ms x155)  idle 86.1%
```

Two threads, each about one sixth busy, `MAX_PENDING_ENCODES` never reached,
`dispatch` 0.0%. The frame costs `max(5.77, 6.46)` and not their sum, which is
exactly what the header on `encode_queue` claims. **That claim is correct and
this is the evidence for it.**

**They stop overlapping under overload, and the mechanism is the back-pressure
itself.** `render_one` encodes inline when three encodes are already queued.
Once the encode is slower than the clock, that fires constantly:

| run | inline encodes | cost of one | render thread busy | waiting |
|---|---:|---:|---:|---:|
| 1080×1920, demand 24, load 33 | **39 of 216** | 71.47 ms | 88.8% | 14.1% |
| 1080×1920, demand 60, load 40 | **86 of 341** | 46.62 ms | 100.5% | 2.6% |
| 1080×1920, demand 60, load 7 | 65 of 580 | 28.59 ms | 74.9% | 27.7% |
| 1920×1080, demand 60, load 7 | 46 of 341 | 33.13 ms | 99.7% | 2.3% |
| 1080×1920, demand 24, load 10 | **0 of 155** | — | 14.9% | 88.0% |

For those frames composite and encode are strictly serial on one thread, so the
collapse is steeper than linear: the pipeline does not merely fail to keep up,
it converts itself into a serial one at exactly the moment it needed the
parallelism.

**One counterintuitive thing that must not be "fixed" without measuring.** In
the load-40 capacity run the encode thread reports 140.5% occupancy at 41.20 ms
a frame — a single encoder could deliver 24.3 fps — and the observed rate was
**34.1 fps**. That is not an accounting error. The inline encode runs
*concurrently* with the queued one, and `rgba_to_nv12` was deliberately moved
out from under the hardware encoder's process-wide mutex (see `encoder.rs`), so
the expensive half of two simultaneous encodes really does run in parallel. The
back-pressure path is accidentally load-balancing the encoder. Removing it, or
raising `MAX_PENDING_ENCODES` without making the encode cheaper, would make
things worse.

### Where the threads block on each other

Only three places, and the probe measures all three:

- **`encode_queue().try_send`** — bounded at 3. This is the one that matters and
  it is quantified above.
- **`encoder.rs`'s `HARDWARE` mutex** — one non-reentrant VAAPI JPEG encoder for
  the process. It serialises the *encode* step (1.59–5.98 ms) but not the NV12
  conversion or the upload, which are the expensive parts.
- **`vaapi::rgba_to_nv12` is a `par_chunks` over the *global* rayon pool.** It
  runs on the encode thread and takes all 12 workers while it does, so it
  competes with the render thread's own decode, `device.poll` and unpad copy.
  The signature is visible in the table above: between load 10 and load 33 the
  render thread's `composite` rose from 5.77 to 28.16 ms — 4.9× — while nothing
  about the compositing changed and the encode next to it rose 8.8×.

**The ring is never the thing that blocks.** `DEFAULT_READ_AHEAD` is 12 and
`DEFAULT_CAPACITY` is 90, so **78 of the 90 slots can never hold read-ahead**;
the measured lead is 10.2–10.7 frames of 12 whenever the renderer keeps up.
Sizing the ring larger buys nothing until the read-ahead is raised, and that is
not written down anywhere.

## What the webview costs

**The part that happens inside WebKit — the JPEG decode, `createImageBitmap`,
`drawImage` — cannot be measured from Rust, and this document does not guess at
it.** A GUI is required and `preview_pipeline` deliberately does not need one.

What *is* measurable is everything up to the bytes being ready: the pacer's
`Channel::send`, the frontend building the URL, the protocol handler, and
`serve_uri`. The harness measures announcement → response written, on a pool the
same size as the real handler's.

| 1080×1920, demand 24 fps | load 10 | load 33 |
|---|---:|---:|
| served from the ring on the first look | 145 / 145 | 123 / 239 |
| served after blocking in `cache.wait` | 0 | 15 |
| served a **neighbouring** frame | 0 | 37 |
| served **nothing** (204, viewer keeps the old picture) | 0 | **64** |
| mean time blocked in `serve_uri` | 0.00 ms | **29.16 ms** |
| announced → bytes ready, median | 0.11 ms | 6.31 ms |
| announced → bytes ready, p95 | 0.22 ms | **73.13 ms** |

At demand 60 fps and load 7 the p95 is **60.86 ms** against a 16.7 ms budget,
and at load 40 the *median* is 61.77 ms. That is `FRAME_WAIT` almost exactly, and
it is not throughput — it is one frame's worth of latency added to every frame
the renderer has not yet produced.

**And the wait cannot help.** During playback `request_frame` returns
immediately without queueing anything, precisely to avoid the decoder-seek
feedback loop it documents. So the 60 ms is spent waiting for a frame the
renderer was going to produce at its own pace regardless, instead of instantly
serving the neighbour that `nearest` would have given.

## Work nobody sees

Four kinds, all confirmed:

- **Rendering at canvas resolution into a much smaller panel.** `render_one`
  uses `ctx.clamp_size(session.size)`, and `session.size` comes from
  `proxy_size`, which only caps at 1920 on the *long edge*. `Preview.tsx` then
  sets the canvas element's **bitmap** to that size and scales it with CSS. A
  1080×1920 project therefore renders 2.07 megapixels per frame into a player
  panel that is typically a few hundred CSS pixels wide. Priced below.
- **Every frame under the playhead is composited twice.** `adopt` sets both
  `work.scrub = Some(frame)` and `work.cursor = frame`, so the scrub job renders
  frame N and then playback renders N again. Once per session — but the session
  restarts on **every edit** (`watchDocumentForPreview` → `preview.restart()`,
  160 ms debounce), so a drag is one wasted full-resolution frame every 160 ms.
- **Every edit throws the ring away.** `adopt` calls `cache.reset`, which drops
  up to 90 encoded JPEGs describing frames the user is about to look at again.
  This is Task #9 and it is still open.
- **Frames encoded and then not shown**: essentially none during steady
  playback. `pace` returns `Skip` *before* rendering, so late frames are
  abandoned rather than composited. The waste is in the three items above, not
  here.

## What capping the render resolution is worth

The harness measures native and reduced back to back in one process, so this
pair is the most trustworthy comparison in the document. "Capped" is
`long_edge = 960`, i.e. 540×960 for a 1080×1920 canvas — a quarter of the
pixels, and still more than the player panel usually shows.

| 1080×1920, demand 60 fps | native, load 7 | capped, load 7 | native, load 40 | capped, load 40 |
|---|---:|---:|---:|---:|
| rendered | 58.0 fps | **61.1 fps** | 34.1 fps | **61.1 fps** |
| composite, per frame | 9.71 ms | **3.41 ms** | 17.72 ms | **3.76 ms** |
| encode, per frame | 18.75 ms | **5.23 ms** | 41.20 ms | **6.77 ms** |
| inline encodes | 65 / 580 | 4 / 611 | 86 / 341 | 2 / 612 |
| ring at or behind the playhead | 32.4% | **0.3%** | 97.9% | **0.3%** |
| announced → bytes, p95 | 60.86 ms | **0.96 ms** | 73.05 ms | **1.02 ms** |
| blank (204) responses | 7 / 602 | **0** | 268 / 591 | **0** |

At 1920×1080 and demand 60 the same change is 34.1 → 44.9 fps, and at
1080×1920 demand 24 on a load-33 machine it is 21.6 → 25.0 fps with the starved
fraction falling from 47.8% to 1.1%.

**Every failure symptom disappears together.** That is the shape of a
bottleneck, not of a tuning knob.

## What to change, most time recovered first

### 1. Render at the size of the player panel, not the size of the canvas — DONE

**Recovers 2.8× of the encode and 2.8× of the composite; turns 34.1 fps into
61.1 fps on a busy machine.** The table above is the whole justification.

**Cheap.** `PreviewOptions.long_edge` already exists and already does exactly
this; the frontend already measures its stage with a `ResizeObserver`
(`Preview.tsx`'s `stage` state) and throws the number away. The work is passing
it to `preview_start`, debouncing it, and making the canvas bitmap follow the
served size instead of the proxy size. Perhaps fifty lines across both sides,
plus the rule that a **paused** frame must still render at full size — a still
is what the user sits and looks at.

**This has since landed**, built in parallel in the main checkout as
`session::Viewport` / `preview_size`, with `examples/preview_waste.rs` measuring
it from the other direction: **3.1×** the throughput for a 1920×1080 project in
a 700 px panel. Two harnesses, two methods, two agents, the same conclusion —
which is the useful thing about having measured it twice.

Read the two results together, because they disagree about *why* in a way that
matters. `preview_waste` measures 3.1× rather than the 7.5× the pixel ratio
implies, and attributes the shortfall to the decode being a fixed cost. This
document's measurement agrees and sharpens it: at the panel size the encode
falls to 5.23 ms and the render thread idles 80%, so what is left is decode plus
`device.poll`, and **the next lever is item 2, not the quality ladder**. The
ladder cannot reduce decode, which is why its rungs are worth only 1.5–1.6× once
the frame is already panel-sized.

### 2. Stop carrying the finished frame to the CPU and back — BUILT, AND BLOCKED BY THE HARDWARE

**60% of the frame at load 10, 76% at load 33.** Readback 5.10 + RGBA→NV12 1.81
+ upload 2.59 = 9.50 ms of a 15.82 ms frame; at load 33, 14.70 + 20.78 + 9.35 =
44.83 ms of 58.73. The fixed-function JPEG encoder is 1.59 ms of its own
5.73 ms path — **28%** — and the rest is delivering pixels to a chip that
already had them.

**Medium, and not a rewrite, because every piece already exists and ships in the
export.** `render::nv12` is the compute pass, `render::dmabuf` is the allocation
and export, `export/hwframes.rs::import_nv12_dmabuf` is the VAAPI import. The
export measured 1.7× from the compute pass alone and another 1.8× from
zero-copy (`docs/STATUS.md`, "What each step bought"). The preview needs
`Compositor::render_nv12_into` feeding a JPEG encoder instead of an H.264 one.
The obstacle is that `preview::vaapi::VaapiJpegEncoder` currently owns its own
staging frame and expects RGBA; it needs the `encode_nv12` entry point it
already half has.

This is `docs/research/zero-copy-encode.md` pointed at the preview, which both
that document and STATUS.md have been predicting for two days. The numbers now
say it is worth more here than it was in the export.

**Built 2026-07-27, measured at 2.4–3.1× on the serial frame, and it does not
work on this chip.** Intel's fixed-function *JPEG* encoder reads an imported
linear NV12 surface as though it were 32-row tiled; the *video* encoder reads the
same file descriptor, in the same process, correctly. The estimate above was
right and the obstacle was not the one this section names — it was not
`VaapiJpegEncoder` owning a staging frame, which was ten lines. Full working,
the row-ramp probe that identified it, what was ruled out and what would make it
work: **`docs/research/preview-zerocopy-jpeg.md`**.

The path ships behind `preview::zerocopy::encoder_can_read_linear`, which encodes
a known ramp once per process and only enables itself if the picture comes back
right. On this machine it does not, one INFO line says so, and the preview reads
frames back exactly as it did before. **So the 60–76% is still on the table and
is still the largest item — it is just not reachable from here.** Anyone picking
this up should run `cargo run --release --example preview_zerocopy` first: if its
first table shows the `mjpeg_vaapi` row matching the `h264_vaapi` row, the work
is already done and switches itself on.

### 3. Take `rgba_to_nv12` off the global rayon pool

**Cheap, and only worth doing if item 2 is deferred** — item 2 deletes the pass
entirely. The evidence is that the render thread's own work inflates 4.9×
between load 10 and load 33 while its inputs did not change, and the encode
thread is running a 12-way parallel loop over the whole frame the entire time.
A dedicated two- or four-thread pool would bound the interference. Ten lines.

Note this is the *third* time the global pool has caused a preview problem: the
deadlock in `encode_queue`'s header, the request-pool starvation in
`frame_protocol_async`'s header, and now this. That pattern is worth a decision
document.

### 4. Do not block a frame request for 60 ms during playback

**Cheap, and it is latency rather than throughput — which is what "lahm"
actually describes.** Measured: mean 29–56 ms blocked per request when the ring
is behind, median announced → bytes 54–62 ms against a 16.7–41.7 ms budget.
Since `request_frame` is deliberately a no-op while playing, the wait cannot
make the frame arrive sooner; it only delays the neighbouring frame that
`nearest` would have returned immediately. One branch in `serve_uri`: if the
clock is playing and `nearest` has something within tolerance, serve it now and
skip `cache.wait`. Keep the wait for scrubs, where it is doing its job.

### 5. Raise the read-ahead, or stop sizing the ring as if it mattered

**Cheap, and mostly a documentation fix.** `DEFAULT_READ_AHEAD` is 12 and
`DEFAULT_CAPACITY` is 90; the measured lead is 10.2–10.7 of 12 whenever the
renderer keeps up, so 78 slots are dead weight and the ring's "three seconds at
30 fps" is not three seconds of anything. Once items 1 and 2 make the renderer
faster than the clock, a deeper read-ahead is what converts that speed into
tolerance for a bad second — and it costs nothing but memory that is already
allocated.

### 6. Stop composing the frame under the playhead twice

**Cheap.** `adopt` arms both the scrub and the cursor at the same frame. One
wasted full-resolution frame per seek and per edit; during a drag that is one
every 160 ms.

### 7. Key the frame cache to the document, not the session

**Medium; this is the existing Task #9.** Not on the critical path for the
frame rate, but it is what makes an edit and a backward seek stop costing a
decoder seek (130–227 ms) plus 90 discarded JPEGs.

## Claims in the repository that this contradicts

These are load-bearing for other work, which is why they are listed before
anything else gets built on them.

1. **`docs/STATUS.md`, "What works, verified": "Preview playback | 10.7 ms for a
   whole 1920×1080 frame … 32% of the 30 fps budget".** The number is a correct
   *serial latency* on a generated fixture at load 2.4. It is filed under
   "Preview playback", which is what made 11–20 fps look inexplicable. Measured
   live on real footage: 58 fps at 1080×1920 and 34 fps at 1920×1080 at load 7,
   21.6 fps at load 33 — and the serial frame is 15.8 ms at load 10, not 10.7.
   The row should say "one preview frame's latency, serially, on a quiet
   machine", and the playback rate should be its own row.

2. **`server.rs` module doc, the `FRAME_WAIT` doc comment, `lib/api.ts` and
   `Preview.tsx` all describe a 404.** `serve_uri` cannot return one: it returns
   200 (the frame or a neighbour), 410, or 204. So `FrameResult`'s `pending`
   variant is unreachable and the "one retry" in `Preview.tsx` never runs. Four
   places, all stale in the same direction.

3. **A 204 reaches the frontend as a *successful* fetch and is then swallowed as
   an error.** `response.ok` is true for 204, so `fetchFrame` calls
   `createImageBitmap` on an empty blob, which throws, and the frame is reported
   as `{status:"error"}` and dropped. In the load-33 run **64 of 239 frames** —
   a quarter of everything the user was told to display — took that path. The
   viewer keeps the previous picture, which is the intended outcome, but "the
   renderer produced nothing" is indistinguishable from a genuine fetch failure
   and neither is counted anywhere.

4. **`MAX_PENDING_ENCODES`'s doc comment: "with compositing around 17 ms and
   encoding around 10 ms".** Inverted at 1080p on this machine: composite 5.77 /
   encode 6.46 at load 10, composite 28.16 / encode 57.16 at load 33. The encode
   is the *larger* of the two and it is what saturates first, which changes the
   reasoning that comment is there to support.

5. **`docs/architecture/preview-pipeline.md`: "Rendered frames land in a
   fixed-size ring (default 90 frames ≈ 3 s at 30 fps)".** True of the
   allocation and false of the behaviour: the read-ahead cap is 12, so the ring
   never holds more than 12 frames ahead of the playhead. Measured lead
   10.2–10.7.

6. **`docs/research/zero-copy-encode.md`'s ranking is now out of date for the
   preview.** It ranks the readback first "if it is 4 ms of a 15 ms frame, the
   ceiling on all of this work is 35%". For the preview the readback plus the
   conversion plus the upload is 60–76% of the frame, so the ceiling is much
   higher here than that document allows for — and unlike the export, the
   preview's encoder wants NV12 in a *buffer*, which is the case that document's
   own 2026-07-26 update says is already solved.

## What was not measured, and would need a GUI

- WebKit's own JPEG decode, `createImageBitmap` and `drawImage`. Stated as
  unknown rather than estimated. A frame is 116–146 KB, so the bytes are not the
  issue; whether the decode lands on the UI thread is, and only a GUI can say.
- Tauri's serialisation of a `PreviewEvent` and its trip through the webview's
  IPC. The Rust half of it — `Channel::send` — is 0.037–0.094 ms and under 0.5%
  of the wall clock, which bounds it from below but not from above.
- Anything at a load average below 7. `chukcut-bench` requires 4 and this
  machine did not offer it in the several hours this took. Every table above is
  stamped; re-run them on a quiet machine before quoting an absolute.
