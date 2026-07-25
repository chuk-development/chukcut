# Hardware JPEG for the preview

Written 2026-07-25 after moving the preview's per-frame JPEG encode off the CPU.
The short version: **the JPEG encoder was never the interesting part.** It is
now 2 ms and the two steps that carry pixels to it cost more than it does.

Code: `src-tauri/src/modules/preview/vaapi.rs` and `encoder.rs`.
Benchmark: `cargo run --release --example preview_jpeg_bench`.

## What the machine has

`vainfo` on the Raptor Lake iGPU (iHD 24.1.0, VA-API 1.20):

```
VAProfileJPEGBaseline           : VAEntrypointVLD
VAProfileJPEGBaseline           : VAEntrypointEncPicture
```

`EncPicture` is the one that matters — a fixed-function JPEG *encoder*, distinct
from the `EncSlice` used for video. FFmpeg 6.1.1 exposes it as `mjpeg_vaapi`,
which is in Ubuntu's build.

Unlike `av1_vaapi` — in every FFmpeg build, encodes on nothing — this one is
real, and it was verified the way `export/hwaccel.rs` verifies things: by opening
the device and encoding a frame, not by reading a capability list.

## The numbers

Median/best of 21 interleaved runs, 2026-07-25, load average 3, quality 88,
synthetic frame with real high-frequency content. Milliseconds per frame.

| | jpeg-encoder | libjpeg-turbo | VAAPI | KB (turbo/VAAPI) |
|---|---|---|---|---|
| 540x960 | 10.9 / 6.5 | 4.6 / 2.2 | **2.3 / 1.4** | 274 / 282 |
| 1080x1920 | 31.1 / 26.7 | 10.2 / 8.5 | **6.2 / 4.7** | 1079 / 1111 |
| 1920x1080 | 31.4 / 26.2 | 11.2 / 8.8 | **6.1 / 4.7** | 1082 / 1114 |

Two steps, both real:

- **jpeg-encoder to libjpeg-turbo: 3.1x.** Free — a crate swap. The pure-Rust
  encoder has no SIMD; libjpeg-turbo is nothing but.
- **libjpeg-turbo to VAAPI: 1.6x.** Not free, and smaller than it looks like it
  should be. See below.
- **End to end: 5.0x**, 31.1 ms to 6.2 ms at 1080x1920.

The frame is 1.03 MB either way, so this changes nothing about what the webview
has to fetch.

### Where the VAAPI time actually goes

Per 1080x1920 frame:

| step | ms |
|---|---|
| RGBA to NV12 (CPU, rayon) | 1.6 |
| upload to a VA surface | 2.2 |
| **the JPEG encode itself** | **2.0** |

**The fixed-function encode is a third of the hardware path.** The other two
thirds are moving pixels the compositor already had on the GPU: it renders into
a texture, the preview reads 8 MB back to system memory, converts it, and sends
3 MB straight back to the same chip.

So the remaining win here is not a faster encoder — there is nothing left to
win there. It is not making the round trip:

- `render::nv12` (another agent, in flight) does the colour conversion in a
  compute shader and halves the readback, since NV12 is 1.5 bytes per pixel and
  RGBA is 4. That removes the 1.6 ms *and* two thirds of the readback.
- DMA-BUF export from wgpu would remove the upload as well. That is
  `docs/research/zero-copy-encode.md`, written about the same copy on the export
  side.

## Traps

### swscale cannot do RGBA to NV12 at any useful speed

`sws_scale` was the obvious way to do the colour conversion and was the first
implementation. Measured at **37 ms** for a 1080x1920 frame — three times what
libjpeg-turbo needs to produce an entire finished JPEG, and on its own more than
the whole 33 ms frame budget.

libswscale has no SIMD path for that format pair and falls through to its
generic per-pixel C converter. The tell was that portrait was 2.5x slower than
landscape at the same pixel count, which is what a per-row C loop looks like.

`preview::vaapi::rgba_to_nv12` replaces it: an integer BT.601 transform over
rayon, about twenty times faster. **Anyone reaching for swscale on this path
should measure it before trusting it.**

### JPEG is full range, and every YUV routine defaults to limited

This is the expensive one, because it produces a working, valid, wrong-looking
JPEG and nothing reports an error.

JPEG carries no range tag. There is no field for it, and every decoder in
existence reads the samples as 0-255. But every RGB-to-YUV routine written for
video — swscale included — defaults to **limited** range, 16-235, because that
is right for a video stream.

Feeding limited-range samples to the JPEG encoder produces grey blacks and no
white. In an editor that reads as the preview having washed out the footage, and
the search for the cause starts nowhere near the encoder.

It shows up as a PSNR against the software encoder of about **27 dB**. With the
JFIF coefficients it is **37 dB**. The unit test
`black_and_white_land_on_the_ends_of_the_full_range` is the cheap guard: black
must be luma 0 and white must be luma 255.

### `global_quality` is a 1-100 quality here, not a lambda

Several FFmpeg encoders divide `global_quality` by `FF_QP2LAMBDA` (118).
`mjpeg_vaapi` does not — it takes 1-100 directly and errors outside that.
Established by sweeping it and watching the output size move monotonically:
50 → 71 KB, 80 → 105 KB, 90 → 136 KB, 95 → 176 KB.

If it had been a lambda, quality 95 would have become 0 and the encoder would
have refused to open.

### `async_depth` defaults to 2, which hides the frame you just asked for

VAAPI encoders hold frames back to keep the pipeline full. That is right for
throughput and wrong for a preview, where `receive_packet` returning `EAGAIN`
means the frame the user is waiting on never arrives. We open with
`async_depth=1`.

The cost is throughput we do not need: FFmpeg's own pipeline at the default
manages upload+encode in 5.5 ms per frame at 2 MP, against our 4.2 ms
serialised, so there is little in it either way for one frame at a time.

### NV12 has no odd sizes, so the fallback is a normal path

Chroma is subsampled by two in both directions, so an odd width or height cannot
be represented at all. Preview sizes come from rounding a scale factor, so odd
edges are routine.

This is worth stating plainly because it means **the software fallback runs on
every machine, not just ones without a GPU**, and it therefore has to be
correct rather than merely present. `encoder.rs` checks the size before it
checks the device.

### No JFIF header unless you ask

`mjpeg_vaapi`'s `jfif` option defaults to false, so the output is
SOI/DQT/SOF0/DHT/SOS/EOI with no APP0 segment. That is a legal JPEG — component
IDs 1/2/3 mean YCbCr by convention and both WebKit and libjpeg decode it — but
we set `jfif=1` anyway. Eighteen bytes is not worth a consumer that refuses.

## How the two encoders are compared

The same check `export/` runs on the hardware video encoder: encode the same
frame both ways, decode both JPEGs, and compute PSNR.

At quality 88-90 the hardware and software outputs agree at **37 dB**. For
calibration: a full/limited range mismatch scores about 27, and the software
encoder against its own input scores above 40. So 37 dB says "the same picture,
quantised slightly differently", which is what two conforming JPEG encoders at
the same quality should produce.

`encoder::tests::the_hardware_encoder_matches_the_software_one` runs this on
every `cargo test` and skips where there is no device.

## Measuring on this machine at all

Run-to-run spread is wide enough to invert a conclusion. The same encoder on the
same frame measured 8.4 ms and 31 ms within one session, depending on what else
was running.

Two things were needed to get numbers worth writing down:

1. **Interleave the candidates**, one round each, rather than running all the
   trials of A and then all of B. Sequential blocks gave a table where whichever
   encoder went second paid for the heat the first one generated — it made
   libjpeg-turbo look 3x slower than it is.
2. **Report the best as well as the median.** Every source of noise here only
   ever adds time, so the minimum is the better estimate of what the work costs;
   the median is what a frame costs on a busy machine. When they are far apart,
   neither should be quoted alone.

The benchmark does both. It waits for nothing, so check the load average before
believing it.

## What is still on the CPU in the preview path

After this work, per frame:

- compositing — GPU, but the result is **read back to system memory** (8 MB at
  1080x1920, the largest single copy left)
- RGBA to NV12 — CPU, 1.6 ms, until `render::nv12` lands
- upload to a VA surface — CPU-driven copy, 2.2 ms
- the JPEG encode — GPU, 2.0 ms
- decode — `media`, being moved to VAAPI by another agent
