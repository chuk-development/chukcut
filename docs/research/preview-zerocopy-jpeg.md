# The preview hands its frames to the JPEG encoder, the other way round

**Updated 2026-07-27** (second session, same day). The first half of this
document stands unchanged and is still the diagnosis; what has changed is the
conclusion. In one sentence:

> Do not describe your memory to the media driver. Let it allocate, export the
> surface, and import *that* — then there is no layout to describe and nothing
> to get wrong.

It is built, it is on by default, it is measured at **2.6–2.9× on a whole
preview frame** on a quiet machine and **4.5× on the encode thread** of a running
server, and it is right per pixel at every width tested including 1440, 1360,
700 and 394.

The route that does not work — ours allocates, the driver imports — is still in
the tree and still switched off behind its own probe, because it is a different
set of driver assumptions and a machine that fails one may pass the other.

---

## Part one: what was found first, and is still true

Written against `64e7afe`. This began as item 2 of
`docs/research/preview-performance.md` — "stop carrying the finished frame to the
CPU and back", the largest recoverable item in the preview and worth 60–76% of a
frame.

The first attempt allocated the NV12 destination as a Vulkan buffer, exported it
as a `DRM_FORMAT_MOD_LINEAR` DMA-BUF and handed the descriptor to libavutil —
which is exactly what the export does with `h264_vaapi`, and which ships and
works there. It measured 2.4–3.1× and produced a fine coloured mosaic, because:

> Intel's fixed-function **JPEG** encoder reads an imported linear NV12 surface
> as though it were 32-row tiled. The **video** encoder, on the same chip, in the
> same process, reads the very same buffer correctly.

### Reproduce all of it in thirty seconds

```bash
cd src-tauri && cargo run --release --example preview_zerocopy
```

The first table is the whole finding. One DMA-BUF holding linear NV12 whose luma
*is* the row number, handed to three consumers — plus, now, the fourth row, which
is the fix.

| output row | 0 | 1 | 31 | 32 | 63 | 64 | 128 | 255 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| what it should be | 0 | 1 | 31 | 32 | 63 | 64 | 128 | 255 |
| `mjpeg_vaapi`, uploaded into a pool surface | 0 | 1 | 31 | 32 | 63 | 64 | 128 | 255 |
| **`mjpeg_vaapi`, this DMA-BUF** | **15** | **15** | **15** | **47** | **47** | **79** | **143** | **239** |
| `h264_vaapi`, this DMA-BUF | 0 | 0 | 31 | 32 | 63 | 64 | 128 | 255 |
| **`mjpeg_vaapi`, a surface it allocated itself** | **0** | **1** | **31** | **32** | **63** | **64** | **128** | **255** |

Read the third row: every block of 32 output rows comes back as the *mean of the
32 source rows under it* — 15 is the mean of 0…31, 47 the mean of 32…63, 143 the
mean of 128…159. That is what linear memory read as 32-row-tiled looks like. The
picture is a fine mosaic of saturated colour; against the software encoder it
scores 4.4 dB.

The fourth row is what makes it a diagnosis rather than a symptom. The *same file
descriptor*, the same strides, the same `DRM_FORMAT_MOD_LINEAR`, the same
`av_hwframe_map` call, encoded by `h264_vaapi` instead — and it is exact.

The fifth row is part two.

### Why a row ramp and not a PSNR

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

### What was ruled out, and how

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

---

## Part two: what the driver actually advertises, asked rather than guessed

The obvious next move is "declare the right modifier instead of LINEAR". It
cannot work, and the reason is worth writing down because it took one small C
program to find and would otherwise have taken a day of Rust.

`vaQuerySurfaceAttributes` for `VAProfileJPEGBaseline` + `VAEntrypointEncPicture`
on iHD 24.1.0 / VA-API 1.20, Raptor Lake:

```
PixelFormat   NV12  YUY2  UYVY  Y800  ABGR
MinWidth 16   MaxWidth 16384   MinHeight 16   MaxHeight 16384
MemoryType    0x40000001 = VA | DRM_PRIME_2
```

Three things follow, and only the third is a surprise.

1. **`VASurfaceAttribDRMFormatModifiers` is not in the list.** It is write-only
   by specification — it is an input to `vaCreateSurfaces`, not something a
   driver reports — so there is no capability list to consult here. Nothing about
   the linear path was refused; it was accepted and then misread.
2. **`DRM_PRIME` (the old one) is not supported for either encode entrypoint,
   only `DRM_PRIME_2`.** VPP is the only entrypoint that takes `USER_PTR` and
   `DRM_PRIME`.
3. **iHD ignores the modifier you ask for.** Asking `vaCreateSurfaces` for
   `DRM_FORMAT_MOD_LINEAR`, `I915_FORMAT_MOD_X_TILED`, `Y_TILED`, `4_TILED`, or
   even `DRM_FORMAT_MOD_INVALID` returns, in every case, a surface that
   `vaExportSurfaceHandle` reports as **`0x0100000000000002` —
   `I915_FORMAT_MOD_Y_TILED`**. Identical for the JPEG, H.264 and VPP
   entrypoints.

So "declare `I915_FORMAT_MOD_Y_TILED` on our linear buffer" would have been a
lie, and "allocate a Y-tiled buffer through VAAPI and declare it truthfully" is
just part three by a longer route. Y-tiling on this generation is a 4 KB tile of
**32 rows by 128 bytes**, which is precisely the 32-row block mean the ramp
showed. The engine was reading the buffer as the tiling it always uses.

Its exported layout, for the record — one object, two layers, one pitch:

| surface | object size | layer 0 | layer 1 |
|---|---:|---|---|
| 1920×1080 | 3 194 880 | `R8` @0 pitch 1920 | `GR88` @2 088 960 pitch 1920 |
| 1440×1080 | 2 555 904 | `R8` @0 pitch **1536** | `GR88` @1 671 168 pitch 1536 |
| 700×1080 | 1 277 952 | `R8` @0 pitch **768** | `GR88` @835 584 pitch 768 |
| 394×1080 | 851 968 | `R8` @0 pitch **512** | `GR88` @557 056 pitch 512 |

Note the driver pads the pitch to a multiple of 128 on its own, which is the same
number `render::nv12::ROW_ALIGN` arrived at empirically and for the same reason.

The probe that produced all of the above is thirty lines of C against
`libva`/`libva-drm` and took ten minutes to write. **Write it before theorising
about a driver.** It is not kept in the tree — it needs no build system, and
`vainfo` plus this table is what anyone would want from it — but it is
reconstructible from this section.

---

## Part three: what works, and why it was always the more likely answer

Turn the arrangement round.

```text
  av_hwframe_get_buffer      VAAPI allocates an NV12 surface
       │                     — Y-tiled, because that is what it always does
       │  av_hwframe_map(READ|WRITE|DIRECT) ──► vaExportSurfaceHandle
       ▼
  two DRM planes: R8 at offset 0, GR88 further into the same object
       │
       │  wgpu-hal texture_from_dmabuf_fd, VK_EXT_image_drm_format_modifier
       ▼
  two wgpu colour attachments the compositor draws NV12 into
       │
       ▼
  the *same* VA surface, handed straight to mjpeg_vaapi
```

Nothing on our side declares a layout, because the layout is not ours. The
modifier, the pitch and the offset all come from the driver's own export
descriptor and are handed to Vulkan verbatim — which is exactly what
`render::dmabuf::import_plane` has been doing for hardware-decoded frames since
July 26. This is the same trick, pointed the other way.

### What had to be true on the Vulkan side, and is

A driver may expose a modifier for *sampling* and refuse to render into it, so
this was asked too, with `vkGetPhysicalDeviceFormatProperties2` and
`VkDrmFormatModifierPropertiesListEXT`. ANV, Mesa 25.0, Raptor Lake:

| format | modifier | tiling features |
|---|---|---|
| `R8_UNORM` | LINEAR, X_TILED, **Y_TILED**, Y_TILED_GEN12_RC_CCS, Y_TILED_GEN12_MC_CCS | SAMPLED, STORAGE, **COLOR_ATTACHMENT**, BLEND, transfer both ways |
| `R8G8_UNORM` | the same five | the same |
| `G8_B8R8_2PLANE_420_UNORM` | LINEAR, X_TILED, Y_TILED | SAMPLED and transfer **only** — no colour attachment |

and `vkGetPhysicalDeviceImageFormatProperties2` with
`VkPhysicalDeviceImageDrmFormatModifierInfoEXT` + external DMA-BUF confirms
`R8_UNORM` and `R8G8_UNORM` at `COLOR_ATTACHMENT | SAMPLED | TRANSFER_SRC` under
Y_TILED, importable, up to 16384². `I915_FORMAT_MOD_4_TILED` is **not** supported
by ANV on this chip, which is worth knowing before reaching for it.

The third row is why the planes are imported separately as `R8` and `GR88` rather
than as one `NV12` image: the multi-planar format cannot be a render target here
at all. That is the same reason `media::dmabuf` gives for splitting the decode
side, arrived at independently.

### Why a render pass and not the compute pass

WebGPU has no storage-writable `R8Unorm` or `Rg8Unorm` — the fact that made
`render::nv12`'s compute shader write a *buffer* in the first place, and the
reason the first version of this document said a tiled destination would need a
`vkCmdCopyBufferToImage`. It does not. Both formats are perfectly ordinary
**colour attachments**, so `shaders/nv12_planes.wgsl` draws one full-screen
triangle per plane: luma at picture size, chroma at half size with each fragment
averaging its own 2×2 block. The hardware does the tiling swizzle on write and
nothing copies.

It costs nothing against the compute version. Both arms in one process, best of
5 rounds of 24 frames, load 7:

| canvas | readback + CPU NV12 + upload | **drawn into the driver's surface** | exported linear buffer |
|---|---:|---:|---:|
| 1920×1080 | 10.13 ms (99 fps) | **3.73 ms (268 fps) — 2.72×** | 3.49 ms (286 fps) — 2.90× |
| 1088×1920 | 9.00 ms (111 fps) | **2.91 ms (344 fps) — 3.10×** | 3.21 ms (312 fps) — 2.80× |
| 1440×1080 | 8.04 ms (124 fps) | **3.13 ms (320 fps) — 2.57×** | 3.22 ms (310 fps) — 2.49× |

The third column is the broken one, timed anyway: it produces a mosaic, and it is
there to show that the render pass is not paying for the layout it is not
declaring. The two zero-copy columns are the same speed within this machine's
noise.

`LoadOp::Load` on the attachments costs nothing and is the only correct choice.
`wgpu_core::Device::create_texture_from_hal` constructs the texture with
`init: false` and `TextureClearMode::None`, which makes wgpu treat an imported
image as already initialised — so `Load` inserts no clear, and `Clear` could not
be honoured anyway. The initial tracker state is `UNINITIALIZED`, so the first
barrier transitions from `VK_IMAGE_LAYOUT_UNDEFINED` and discards contents we are
about to overwrite entirely.

### Verified by pixels, at widths that are not multiples of 64

`preview_zerocopy` phase 2. The same composited frame encoded four ways, decoded,
and compared per channel. 1440, 1360, 700 and 394 are 32, 16, 60 and 10 short of
a multiple of 64; 1920 and 1088 are the aligned controls.

| size | driver pitch | drawn vs libjpeg-turbo | drawn vs the copying GPU path | exported vs libjpeg-turbo |
|---|---:|---|---|---|
| 1440×1080 | 1536 | **44.2 dB**, max Δ 33, 0.77% over 8 | 44.2 dB, max 37, 0.75% | 5.4 dB, max 255, 60.3% |
| 1360×768 | 1408 | **42.8 dB**, max 34, 1.10% | 42.8 dB, max 33, 1.05% | 4.4 dB, max 255, 78.1% |
| 700×394 | 768 | **41.8 dB**, max 29, 1.43% | 41.8 dB, max 31, 1.38% | 4.4 dB, max 255, 80.8% |
| 394×700 | 512 | **45.3 dB**, max 35, 0.64% | 45.4 dB, max 32, 0.61% | 8.8 dB, max 255, 33.0% |
| 1920×1080 | 1920 | **43.5 dB**, max 35, 0.91% | 43.5 dB, max 36, 0.87% | 4.4 dB, max 255, 78.0% |
| 1088×1920 | 1152 | **47.5 dB**, max 32, 0.38% | 47.5 dB, max 35, 0.36% | 9.2 dB, max 255, 28.0% |

For calibration, and the reason these numbers mean something: an unaligned stride
the encoder read differently scored **19 dB with a max delta in the hundreds**
before `ROW_ALIGN` existed, a limited-range mismatch scores about **27 dB** with a
max delta near 16 in flat black, and the shipped *copying* hardware path agrees
with libjpeg-turbo at **37 dB** (`docs/STATUS.md`). So the drawn path is not
merely acceptable at these sizes — it agrees with the software encoder **better
than the path it replaces**, at every one of them, and the unaligned widths are
not distinguishable from the aligned ones.

### On a running server, both arms in one process

`examples/preview_pipeline` now runs the live phase twice per canvas, with
`vasurface::set_enabled` and `zerocopy::set_enabled` off and then on, in the same
process a few seconds apart. 1920×1080, 8 s of playback at 60 fps demand, load
3.5:

| | reading every frame back | drawn into the encoder's surface |
|---|---:|---:|
| encode thread, per frame | 5.32 ms | **1.18 ms** |
| render thread `composite`, per frame | 11.01 ms | **8.92 ms** |
| serial frame | 16.33 ms (61.3 fps) | **10.10 ms (99.0 fps)** |
| pipelined ceiling | 90.9 fps | **112.1 fps** |
| ring at or behind the playhead | 14.5% | **0.2%** |
| requests served only after blocking | 6 of 482 | **0** |
| `announced → bytes` p95 | 0.22 ms | 0.20 ms |

The **encode thread falls by 4.5×** and the render thread's own work by 2.1 ms —
the unpad copy out of the mapped readback buffer, which is the half of the
readback that is CPU memory bandwidth rather than a stall.

Two things about that table are worth reading carefully, because they are what
decides how much of this the owner will feel.

- **The throughput gain is smaller than the latency gain**, and that is correct
  rather than disappointing. The two stages pipeline, so the frame costs
  `max(composite, encode)`; deleting most of the encode moves the bottleneck onto
  the render thread, which is now decode plus compositing plus the GPU wait. At
  200 fps demand the observed rate went 110.7 → 125.1 fps, only 1.13×, because at
  that point the render thread is 72% busy and the encode thread is 18%.
- **The interference is gone, and that is the part that matters on a busy
  machine.** `preview-performance.md` item 3 is "take `rgba_to_nv12` off the
  global rayon pool", and its evidence was that the render thread's own work
  inflated 4.9× between load 10 and load 33 while its inputs did not change,
  because the encode thread was running a 12-way parallel loop over the whole
  frame the entire time. **That loop no longer exists on this path.** Item 3 is
  closed by deletion rather than by tuning.

### And on the bench suite

`chukcut-bench --filter preview-frame`, load 3.2–3.4, spread under 1.26 on every
row. The drawn row is measured immediately after the copying row in the same
process, which is what makes the ratio worth more than the milliseconds.

| | copying, what shipped | drawn | |
|---|---:|---:|---:|
| 1920×1080 whole frame | 11.93 ms | **4.36 ms** | 2.74× |
| 1920×1080, share of the 60 fps budget | 72% | **26%** | |
| 1080×1920 whole frame | 11.41 ms | **3.98 ms** | 2.87× |
| 1080×1920, share of the 60 fps budget | 68% | **24%** | |

**Read the ratio and not the absolutes.** Three runs of that group over an hour,
1920×1080 then 1080×1920: 11.13→3.96 and 9.87→3.80 (2.81×, 2.60×); 11.93→4.36 and
11.41→3.98 (2.74×, 2.87×); 12.42→4.72 and 12.62→4.79 (2.63×, 2.63×). The copying
column moved by 28% across those and the ratio by 10%, which is the usual rule on
this machine.

The copying column is also the check that the shipped fallback did not regress:
this suite recorded 10.66 and 11.37 ms for it at `4da5cff`, and all three runs
above are within that spread.

---

## What ships, and what decides

- `render::nv12::Nv12PlaneWriter` and `shaders/nv12_planes.wgsl` — the render
  pass that writes the two planes.
- `render::dmabuf::import_plane_with(PlaneUse::Write)` — the same import
  `import_plane` has always done, with colour-attachment usage and an
  `UNINITIALIZED` initial state.
- `media::dmabuf::DmabufFrame::map_writable` — `AV_HWFRAME_MAP_READ | WRITE`, so
  `vaExportSurfaceHandle` is asked for `READ_WRITE` rather than `READ_ONLY`.
- `preview::vasurface` — the rotation of VA surfaces, the slot state machine, and
  the probe.
- `preview::vaapi::VaapiJpegEncoder::encode_va_surface` and
  `preview::encoder::encode_preview_jpeg_va_surface`. The failure and
  encoder-rebuild bookkeeping that had been written out twice is now
  `encoder::with_hardware`, called by all three entry points.
- `compositor::render_nv12_into_planes`.
- `preview::server::Destinations` — **two** rings of VA surfaces, keyed by exact
  size, most-recently-used last. A VA surface has a fixed extent, so unlike an
  exported buffer a smaller frame cannot borrow a bigger surface; and `render_one`
  alternates between two sizes whenever the quality ladder is off rung 0, because
  a scrub always renders at the session's own size. One ring would thrash.

`preview::vasurface::encoder_reads_its_own_surface` is what decides, and it is the
same shape as the check it sits beside: it draws a 256×128 row ramp through the
real path once per process, decodes the JPEG, and checks every row against its own
number. It does not consult a capability list, and part two above is why —
`vaQuerySurfaceAttributes` reported NV12, `DRM_PRIME_2` and a 16384 maximum for
the path that produced a mosaic.

`preview::server::composite` chooses per frame, **before** the frame is drawn.
That ordering is load-bearing on both zero-copy arms: the destination is
device-local, so once a frame has been composited into one there is nothing on the
CPU that can read it and no software fallback left. The gates are ordered so that
on a machine where the answer is no, the per-frame cost is two atomic loads, an
even-size test and a `OnceLock` load.

The software path is unchanged, is still the fallback, and is still what a machine
without a hardware JPEG encoder, without DMA-BUF import, or with a driver that
refuses either arm will use.

## What is checked, and how often it was run

- `preview::vasurface::tests::the_self_check_agrees_with_what_the_encoder_actually_does`
  — the same implication both ways as its `zerocopy` twin. It cannot assert
  *which* answer a machine gives, so it asserts that a probe saying yes is
  followed by a correct picture at an independent size (200×96, not a multiple of
  64) and a probe saying no by an incorrect one.
- `preview::vasurface::tests::every_slot_encodes_its_own_picture_lap_after_lap` —
  thirteen frames through six slots, each drawing a *different* ramp. One frame
  proves nothing about the second: a slot is reused, its Vulkan image is
  transitioned again, and the encoder read it in between. A frame that came back
  holding its predecessor's picture passes every single-frame check and fails
  this one.
- The three claim tests: a claim reserves a surface, a drop returns it, six
  claims name six surfaces, a seventh is refused rather than wrapping, and a ring
  is exactly one size.

Run counts, all on this machine: the GPU-touching lib set (`render::{dmabuf,
nv12, context}`, `preview::{vasurface, zerocopy, encoder, vaapi}`, 48 tests)
**20 times, 0 failures**; `tests/compositor.rs` (17 tests through the real
compositor and real media) **20 times, 0 failures**; `examples/preview_zerocopy`
end to end **20 times**, with the verdict `READS IT` and the ramp row exact in all
20; the whole suite (688 + 116 tests) green.

**One pre-existing flake was found and is not ours.**
`tests/decode.rs::decoding_sequentially_is_dramatically_cheaper_than_seeking_to_each_frame`
asserts a *ratio* between sequential and random-access decoding and fails when
the machine is busy — "sequential decode took 78 ms and random access 153 ms".
It reproduced 1 time in 12 on the unmodified `71f11cf` with the change stashed,
and 0 times in 32 with the change applied.

## What would still be worth knowing

- **Whether the linear route works anywhere.** Nothing here has run on AMD, on
  Nouveau, or on a newer iHD. If `preview_zerocopy`'s first table ever shows the
  `mjpeg_vaapi, this DMA-BUF` row matching the `h264_vaapi` row, that path
  switches itself on and both are available.
- **Whether the export should do the same thing.** `export::job` tier 3 uses the
  linear route with `h264_vaapi`, which reads it correctly, so there is no bug to
  fix — but the arrangement here needs no `ExportableBuffer`, no `ash` allocation
  and no modifier constant, and is the one that generalises to a driver we have
  not met. Measuring both in the export is a contained piece of work with a
  known-good control on either side.
- **Whether `VkImage` ownership needs a foreign-queue transfer.** Strictly,
  Vulkan wants `VK_QUEUE_FAMILY_FOREIGN_EXT` acquire/release barriers around
  handing an imported image to another API. Nothing here does that and 20 runs of
  everything are exact, which is the usual state of affairs on Mesa; it is
  recorded so that a future corruption report on another driver starts here
  rather than in the shader.

## One bug found on the way, unrelated and now fixed

`render::dmabuf::ExportableBuffer::drop` called `vkDeviceWaitIdle` directly.
Vulkan requires host access to **every** `VkQueue` on the device to be externally
synchronised across that call, and nothing there can synchronise against wgpu's
own submissions or against a second buffer being dropped on another thread. The
export drops one ring at a time on one thread, so it never showed. Six rings torn
down concurrently by the new tests aborted the test binary with a double free or
a SIGSEGV in **two runs out of five**.

It now waits through `wgpu::Device::poll`, which takes wgpu's own locks and waits
for the same thing: 0 failures in 20 runs, and 0 in 20 of the wider GPU set.

This is very likely the "intermittent SIGSEGV in `render::dmabuf`/`render::context`
(3 in 500)" that `docs/STATUS.md` has been carrying.
