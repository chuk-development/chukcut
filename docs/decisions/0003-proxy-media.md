# 0003 — Proxy media

Status: decided 2026-07-26. Implementation in `src-tauri/src/modules/proxy/`.

## The decision

Build proxy media ourselves, as `modules/proxy`: a decision rule, a background
transcode queue, an LRU cache under `paths::proxies_dir()`, and a type-level
switch that lets the preview use a proxy and makes it impossible for the export
to.

Decision 0001 named this as one of the four things neither GES nor MLT gives
away — Pitivi transcodes its own proxies on top of GES, Kdenlive and Shotcut
theirs on top of MLT — so there was never an option to adopt one.

## The rule for when a proxy is worth making

Resolution alone is the rule everybody writes first and it is wrong in both
directions. The rule is a pure function of **source resolution, codec, and a
measured decode cost when one exists**, in `proxy::decision`:

```
budget    = 1000 / fps                              one whole frame, in ms
allowance = budget × 0.7 × (2.0 if intra-only)      what decode may have of it
cost      = measured, or megapixels × per-codec cost

build a proxy  ⟺  the proxy is at least 1.4× smaller on the long side
                  AND cost > allowance
```

- **0.7** because decode is not the only thing in a preview frame, but the JPEG
  encode overlaps compositing on another thread, so decode legitimately gets
  most of the budget rather than half of it.
- **The intra-only bonus** is the "an intra-only codec may not need one at all"
  clause. It is a multiplier and not an exemption because ProRes at 4K is
  genuinely slow and does deserve a proxy — it is *scrubbing* that intra-only
  codecs get for free, not throughput.
- **The shrink clause** comes first, because it is true regardless of cost: a
  proxy that is not smaller than the source is a slower copy of it.

The per-codec costs are calibrated on this machine and keep HEVC at twice H.264
and AV1 at twice HEVC, which is the ratio those codecs have on any CPU decoder.
A measurement, when one is passed, replaces the model outright.

The rule is tested against a twenty-one row table in `decision.rs`. Two rows sit
deliberately close to the line and are the two arguments worth having: **HEVC at
1080p30** (≈21 ms against a 23 ms allowance) and **H.264 at 1080p60** (≈10
against 12). Both are judged playable.

## The codec

**All-intra H.264, 4:2:0 8-bit, long side capped at 1280, in MP4, no audio.**

Long-GOP codecs are cheap per frame *because* their neighbours did the work,
which is exactly why reaching a frame means decoding the ones before it.
All-intra deletes that, and cheap seeking is what a proxy is for. Resolve and
Premiere both ship all-intra proxy formats for the same reason.

H.264 rather than ProRes or DNxHR, given that property:

- the fastest decoder in any FFmpeg build (measured: 5 ms per source megapixel
  single-threaded, against 7 for ProRes);
- the only codec with a hardware encoder on every platform, so proxy generation
  gets the GPU nearly everywhere — there is no hardware DNxHR encoder at all;
- a fraction of ProRes Proxy's bitrate, which is the difference between a 20 GiB
  cache holding four hours of footage and forty minutes;
- FFmpeg's `dnxhd` encoder refuses arbitrary resolutions and frame rates, so a
  generator built on it fails on odd footage.

`tune=fastdecode` on the software path: it trades CABAC and the deblocking
filter — both decode-side costs — for a slightly larger file, which is the right
direction for a file whose only purpose is to be decoded.

## The switch, and why it is a type

A proxy reaching the export is a silent 720p deliverable. Nothing fails, nothing
warns, and it is found after the upload. Comments do not prevent that and a
boolean threaded through six call sites prevents it until somebody adds a
seventh.

So `proxy::switch` has two types. `PreviewSource` is the only one that can hold
a proxy path, and the only accessor that returns it is called `decode_path`.
`ExportSource` has no field a proxy could live in, no `From<PreviewSource>`, and
a constructor that **refuses any path underneath `paths::proxies_dir()`**. The
second half is the belt under the braces and is the one that can be tested at
runtime; it is.

`MediaSourceProvider::from_project` keeps its name and its behaviour — no
proxies — and the preview gets a second constructor. The export path therefore
did not change at all.

## What it costs

- An RGBA round trip per frame in the transcode, because the loop is built out
  of `media::VideoDecoder` and `export::MediaWriter` rather than a second
  FFmpeg pipeline. It buys the seek policy, the VAAPI frame pool, the
  rate-control ladder and a correct encoder flush, all already tested. At proxy
  resolution it is a couple of milliseconds against a transcode dominated by
  decoding the 4K source.
- Disk, bounded by a 20 GiB cap with LRU eviction.
- The cache key is path + size + mtime, so a rewrite that preserves both size
  and mtime is invisible. Content-hashing a 40 GB source at import is not a
  trade worth making.

## What would change our minds

- **Hardware decode landing in the compositor.** Once `render/` can import a
  decoded VA surface as a texture (`docs/research/hardware-decode.md`), 4K HEVC
  decode drops towards 2 ms a frame and most of this rule's output flips to "not
  needed". The rule would then be re-calibrated against the hardware path rather
  than deleted — the machines without a usable VAAPI decoder are still there.
- **A measured cost at import.** The model exists because measuring every file
  at import costs more than it saves. If a cheap measurement appears — decoding
  ten frames during the probe, say — it should be passed to `decide` and the
  table becomes a fallback.
