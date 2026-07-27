# The preview cannot hand its frames to the JPEG encoder, and here is the proof

Written 2026-07-27, against `64e7afe`. This is the outcome of item 2 of
`docs/research/preview-performance.md` — "stop carrying the finished frame to the
CPU and back", the largest recoverable item in the preview and worth 60–76% of a
frame.

**It is built, it is measured at 2.4–3.1×, and it does not work on this
hardware.** The reason is a single fact that nothing in the repository predicted
and that no amount of reading could have produced:

> Intel's fixed-function **JPEG** encoder reads an imported linear NV12 surface
> as though it were 32-row tiled. The **video** encoder, on the same chip, in the
> same process, reads the very same buffer correctly.

So the path ships switched off, behind a self-check that proves the encoder
before using it, and the machinery is here for the machine where the answer is
different.

## Reproduce it in thirty seconds

```bash
cd src-tauri && cargo run --release --example preview_zerocopy
```

The first table is the whole finding. One DMA-BUF holding linear NV12 whose luma
*is* the row number, handed to three consumers:

| output row | 0 | 1 | 31 | 32 | 63 | 64 | 128 | 255 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| what it should be | 0 | 1 | 31 | 32 | 63 | 64 | 128 | 255 |
| `mjpeg_vaapi`, uploaded into a pool surface | 0 | 1 | 31 | 32 | 63 | 64 | 128 | 255 |
| **`mjpeg_vaapi`, this DMA-BUF** | **15** | **15** | **15** | **47** | **47** | **79** | **143** | **239** |
| `h264_vaapi`, this DMA-BUF | 0 | 0 | 31 | 32 | 63 | 64 | 128 | 255 |

Read the third row: every block of 32 output rows comes back as the *mean of the
32 source rows under it* — 15 is the mean of 0…31, 47 the mean of 32…63, 143 the
mean of 128…159. That is what linear memory read as 32-row-tiled looks like. The
picture is a fine mosaic of saturated colour; against the software encoder it
scores 4.4 dB.

The fourth row is what makes it a diagnosis rather than a symptom. The *same file
descriptor*, the same strides, the same `DRM_FORMAT_MOD_LINEAR`, the same
`av_hwframe_map` call, encoded by `h264_vaapi` instead — and it is exact.

## Why a row ramp and not a PSNR

Two hours went into looking at scrambled JPEGs and PSNR numbers, and twenty
minutes into the table above, because a PSNR says two pictures differ and will
not say how. Three faults produce a bad number and they need three different
responses:

| what it looks like | what it is |
|---|---|
| every 32 rows collapse to their mean | the surface is being read as tiled |
| rows shear progressively | the pitch is being ignored (this is `ROW_ALIGN`) |
| flat black comes back at 16, white at 235 | limited range in a full-range format |

A picture whose luma **is its row number** separates all three at a glance, and a
flat chroma of 128 means a colour fault cannot be mistaken for a layout one.
`preview_zerocopy`'s first phase is that probe and it stays in the repository.

## What was ruled out, and how

Each of these was a live hypothesis and each was killed by a measurement rather
than by reasoning.

- **The compute shader writes the wrong bytes.**
  `zerocopy::tests::the_compute_pass_leaves_correct_nv12_in_the_exported_buffer`
  copies the exported buffer back out through wgpu — a route that involves
  neither libva nor the encoder — and compares it byte for byte against
  `preview::vaapi::rgba_to_nv12`, the CPU converter the encoder has been fed for
  months. It agrees to one code value.
- **The full-range conversion is wrong.**
  `render::nv12::tests::full_range_agrees_with_the_cpu_converter_the_jpeg_encoder_used`
  makes the same comparison on the readback path, and
  `full_range_puts_black_at_zero_and_white_at_255` pins the two ends.
- **The stride is not what the encoder wants.** `ROW_ALIGN` is 128, so every
  stride already is; and the fault is identical at 1920 (aligned) and 1440 (32
  short). Deliberately padding to 256 and 1024 made it *worse* in the way a
  shear does, on top of the tiling, which is a second effect and not the first.
- **libavutil's import is wrong.** `export::hwframes`' own round-trip test reads
  an imported surface back with `av_hwframe_transfer_data` byte-exactly, and the
  `h264_vaapi` row above is the stronger version of the same claim.
- **The frame pool, the surface count, the `async_depth`, the render-target
  list.** All identical in shape to the export, which works.

## What it would have been worth

Both arms in one process, interleaved, same clip, same minute — `preview_zerocopy`
phase 3, best of 5 rounds of 24 frames, load 4.3. Serial: composite then encode,
nothing overlapping, which is one frame's latency and what a seek pays.

| canvas | readback + CPU NV12 + upload | straight from the compositor | ratio |
|---|---:|---:|---:|
| 1920×1080 | 10.02 ms (100 fps) | **4.08 ms (245 fps)** | **2.45×** |
| 1088×1920 | 11.36 ms (88 fps) | **3.84 ms (261 fps)** | **2.96×** |
| 1440×1080 | 8.71 ms (115 fps) | **3.61 ms (277 fps)** | **2.41×** |

Three runs at load 4.0, 5.2 and 8.5 gave ratios of 2.45/2.96/2.41, 2.45/2.98/2.35
and 2.75/3.09/2.67. **The ratio is stable across a twofold change in load and the
milliseconds are not** — the copying column ranged from 10.0 to 13.0 ms at
1920×1080 across those three — which is the usual rule on this machine.

That is the size of the prize and it agrees with the profile's arithmetic:
`preview-performance.md` measured readback 5.10 + convert 1.81 + upload 2.59 =
9.50 ms of a 15.82 ms frame, and deleting all three leaves a frame that is decode
plus compositing plus the fixed-function encode.

## What ships

Everything except the decision to use it.

- `render::nv12` converts in **either range**, selected per call. The compute
  pass was limited-range only, which is right for a video encoder and wrong for a
  JPEG — a JPEG file carries no range tag and every decoder reads it as 0…255, so
  limited-range samples give grey blacks and about 27 dB. `YuvRange` already
  existed for the inverse direction and is now used for both.
- `render::dmabuf::Nv12Ring::with_slots` and `::at`, so a caller that chooses its
  own slot can, and so the preview asks for six buffers rather than the export's
  sixteen — 19 MB at 1080p instead of 50.
- `preview::zerocopy` — the ring, the slot state machine, and the self-check.
- `preview::vaapi::VaapiJpegEncoder::encode_dmabuf` and
  `preview::encoder::encode_preview_jpeg_dmabuf`.
- `preview::server::composite` chooses the path per frame, **before** the frame
  is drawn. That ordering is load-bearing: the exported buffer is device-local,
  so once a frame is composited into one there is nothing on the CPU that can
  read it and no software fallback left.
- `examples/preview_zerocopy.rs`.

`zerocopy::encoder_can_read_linear` is what decides. It encodes a 256×128 row
ramp through the real path once per process, decodes it, and checks each row
against its own number. On this machine it fails at row 0 (luma 15, not 0), logs
one INFO line saying so, and every preview frame afterwards takes the readback
path exactly as before. On a machine whose encoder reads linear memory it passes
and the preview stops copying.

**The shipped preview is unchanged, and that was checked rather than assumed.**
`chukcut-bench --filter preview-frame` on a quiet machine: 10.40 ms at 1920×1080
and 10.36 at 1080×1920, against 10.71 and 10.38 before any of this. The gates in
`claim_exported` are ordered so that on a machine where the answer is no, the
per-frame cost is two atomic loads and an even-size test — the two gates that
touch the encoder's process-wide mutex are behind the `OnceLock` and are never
reached again after the first frame. Taking that mutex on the render thread would
block it behind whatever the encode thread is doing, which is the serialisation
`MAX_PENDING_ENCODES`' whole design exists to avoid.

That check is the answer to the trap `zero-copy-encode.md` names — a "zero-copy"
path that quietly does something else is worse than none. This one does not
guess in either direction: it proves the encoder or it says which row came back
wrong.

`zerocopy::tests::the_self_check_agrees_with_what_the_encoder_actually_does`
holds it to that. It cannot assert *which* answer a machine gives, so it asserts
the implication both ways: if the probe says yes, an independently built picture
must come back right; if it says no, that picture must come back wrong. A probe
that said yes and was wrong would put a mosaic in front of the user; one that
said no and was wrong would cost 60% of the frame budget for nothing.

## What would make it work, and why it was not done

The JPEG engine wants the driver's own tiling. Giving it that means the NV12
destination stops being a `VkBuffer` and becomes a `VkImage` allocated through
`VK_EXT_image_drm_format_modifier` with a modifier the media driver accepts —
which is precisely the dance `render::dmabuf`'s header explains this design
avoided, and it is avoided *because* a buffer has no tiling to negotiate. It also
runs into WebGPU having no storage-writable `R8Unorm`, so the compute pass could
not write such an image directly; it would write the linear buffer as now and
then `vkCmdCopyBufferToImage` into the tiled one, letting Vulkan do the swizzle.

That is a GPU-local copy, not zero-copy, but it would still delete all 9.50 ms of
CPU work. The same is true of the cheaper version: a **VPP blit**
(`VAEntrypointVideoProc`, FFmpeg's `scale_vaapi`) from the imported linear
surface into a driver-allocated one. Either is a real piece of work with an
unknown answer at the end of it — nobody knows whether ANV exposes a modifier for
`R8`/`R8G8` that iHD's JPEG encoder will take — and neither is what "medium
effort, every piece already ships" described.

**Do not start either without first re-running `preview_zerocopy` on the target
machine.** If its first table shows the JPEG row matching the h264 row, none of
this is needed and the path simply switches itself on.

## One bug found on the way, unrelated and now fixed

`render::dmabuf::ExportableBuffer::drop` called `vkDeviceWaitIdle` directly.
Vulkan requires host access to **every** `VkQueue` on the device to be externally
synchronised across that call, and nothing there can synchronise against wgpu's
own submissions or against a second buffer being dropped on another thread. The
export drops one ring at a time on one thread, so it never showed. Six rings torn
down concurrently by the new tests aborted the test binary with a double free or
a SIGSEGV in **two runs out of five**.

It now waits through `wgpu::Device::poll`, which takes wgpu's own locks and waits
for the same thing: 0 failures in 20 runs, and 0 in 20 of the wider GPU set
(`render::dmabuf`, `render::nv12`, `render::context`, `preview::{zerocopy,
encoder,vaapi}`, 44 tests).

This is very likely the "intermittent SIGSEGV in `render::dmabuf`/`render::context`
(3 in 500)" that `docs/STATUS.md` has been carrying.
