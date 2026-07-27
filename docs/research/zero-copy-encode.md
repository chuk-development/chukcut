# Zero-copy from the compositor into the encoder

> **Superseded in one important respect, 2026-07-27.** This document's whole
> frame is "we allocate the NV12 destination and describe it to the driver",
> which is what `export::job` tier 3 does and what works with `h264_vaapi`. It
> does **not** work with Intel's JPEG engine, which reads a surface described as
> linear as though it were Y-tiled — and the fix is to stop describing memory to
> the driver at all: let VAAPI allocate, export, and import *that* into Vulkan.
> Read `docs/research/preview-zerocopy-jpeg.md` before building anything from
> here. In particular, the claim below that "the preview's encoder wants NV12 in
> a buffer, which is already solved" is the sentence that cost a day.

Written 2026-07-25, while building the VAAPI encode path in
`src-tauri/src/modules/export/`. It is the notes of the person who got closest
to this problem without solving it, which is the point: the next person should
not have to rediscover the shape.

This is step 2 of `docs/decisions/0002-ffmpeg-and-hardware-encoding.md`. Step 1,
the VAAPI encoder with a CPU round trip, is done and measured.

> **Update, 2026-07-26 — this is now built, and several things below are
> wrong.** The short version, with the measured numbers in `docs/STATUS.md`:
>
> - **Vulkan Video encode does not exist on this driver.** Not "is unreliable" —
>   Mesa 25.2.8 on Raptor Lake exposes no `VK_KHR_video_queue` at all and no
>   queue family with a video bit. The `gpu-video` route at the bottom of this
>   document is dead here and the example never needed running. `vulkaninfo |
>   grep -c VK_KHR_video` answers 0, which is a one-line check and belongs
>   before any of the rest.
> - **The readback was 5.1 ms of a 39 ms 1080p frame.** The right number to have
>   worried about was the swscale pass next to it, which was 14.8 ms.
> - **Doing RGBA→NV12 on the GPU first changed the shape of the rest.** Sections
>   "1", "2" and "3" below all assume the thing being exported is the
>   compositor's RGBA *image*, and so they are all about `VkImage` tiling, DRM
>   format modifiers and a VAAPI VPP blit. None of that turned out to be needed.
>   The compute pass writes NV12 into a **buffer**, and a buffer has no tiling:
>   it is `DRM_FORMAT_MOD_LINEAR` by construction. The VPP recommendation in
>   section 3 is therefore obsolete — there is no colour conversion left for VPP
>   to do.
> - **What is exported is a `VkBuffer`, not a `VkImage`.** That is the single
>   most useful correction in this update. It removes blocker 3 entirely rather
>   than working around it.
> - Section 4's advice — `device.poll(Wait)` and do not optimise the
>   synchronisation first — was right and is what shipped.
>
> The code is `src-tauri/src/modules/render/dmabuf.rs` (allocation and export),
> `render/nv12.rs` (the compute pass), and `export/hwframes.rs`'s
> `import_nv12_dmabuf` (the VAAPI side). Read the rest of this document for the
> reasoning that led there; read those files for what it became.

## What the export path does today, and where the pixels go

```text
  wgpu texture (RGBA8, on the iGPU)
        │
        │  ① Compositor::render_frame — copy to a buffer, map, read back
        ▼
  Vec<u8> RGBA in system memory
        │
        │  ② copy_packed_rows — into a padded AVFrame
        ▼
  AVFrame RGBA
        │
        │  ③ swscale — RGBA → NV12, on the CPU
        ▼
  AVFrame NV12 in system memory
        │
        │  ④ av_hwframe_transfer_data — upload
        ▼
  VA surface (NV12, on the iGPU)
        │
        │  ⑤ avcodec_send_frame
        ▼
  H.264/HEVC packet
```

Steps ① through ④ all exist to move data the GPU already had. On a discrete
card that is a PCIe round trip; on the Raptor Lake iGPU this was measured on,
the two "sides" are the same physical DRAM, which makes it worse to look at, not
better — we are paying to copy memory to itself, four times.

The full numbers are in `docs/STATUS.md`. The decomposition that matters here,
1920×1080, VAAPI H.264, medians of three:

| | per frame |
|---|---|
| Whole export (24.2 fps) | 41.3 ms |
| Steps ② to ⑤ alone, `export_smoke --encode-only` (80.6 fps) | 12.4 ms |
| Decode, composite and readback — the difference | **≈ 29 ms** |
| The encoder itself, `ffmpeg` CLI encoding NV12 it already has | ≈ 3 ms |

So of a 41 ms frame, the encoder is 3 ms. About 9 ms is steps ② to ④ — a 8 MB
row-by-row copy, an RGBA→NV12 swscale pass, and a 3 MB upload — and the
remaining 29 ms is decode plus compositing plus the readback in ①, which this
session did not separate and should have.

That last unseparated number is the important one and it is why the ranking at
the bottom of this document puts "measure the readback" first. If the readback
is 4 ms of that 29, zero-copy is worth much less than it looks and hardware
*decode* is the real prize.

## What the target looks like

```text
  wgpu texture ──► DMA-BUF fd ──► VA surface ──► encoder
```

VAAPI can *import* a DMA-BUF as a surface, exactly as it can export one. The
FFmpeg-side call is `av_hwframe_map` with `AV_HWFRAME_MAP_DIRECT` from an
`AV_PIX_FMT_DRM_PRIME` frame carrying an `AVDRMFrameDescriptor` into the
encoder's VAAPI frames context — the same machinery as the decode direction the
crate survey describes, run backwards.

`hwcontext_drm.h` **is** bound by `ffmpeg-sys-next` 6.1, so `AVDRMFrameDescriptor`
and `av_hwframe_map` are reachable from the wrapper that now exists in
`src-tauri/src/modules/export/hwframes.rs`. That is not the blocker.

## What is actually blocking, in order of how much it will cost

### 1. wgpu will not export a texture as a DMA-BUF

This is the real one, and it is the opposite of the problem the crate survey
describes. The survey covers *import* — `wgpu-hal`'s
`texture_from_dmabuf_fd`, added in wgpu 30 — because it was researching
hardware **decode**. Encode needs **export**, and there is no
`texture_to_dmabuf_fd`.

What is needed is `VK_KHR_external_memory_fd`'s `vkGetMemoryFdKHR` on the
`VkDeviceMemory` backing the texture, which requires the image to have been
*created* with `VkExportMemoryAllocateInfo` — a decision made at allocation
time, not afterwards. wgpu's normal allocator does not do that and offers no
way to ask for it.

So the compositor's render target cannot simply be exported. It has to be
allocated as an external-memory image in the first place, through
`wgpu_hal::vulkan::Device::texture_from_raw` with memory we allocated ourselves
via `ash`. That is the same `TextureMemory::External` escape hatch the import
path uses, and it means the export target becomes a texture wgpu did not
allocate, which the compositor has to be taught to render into.

**Consequence for whoever picks this up:** the work is not in `export/`. It is
in `render/`, in how the compositor allocates the frame it renders to. Budget
for that.

### 2. NV12 is not RGBA, and the conversion has to move too

The compositor renders RGBA. The encoder wants NV12. Today swscale does that on
the CPU (step ③ above), and removing the readback does not remove the
conversion — it moves it onto the GPU as a shader pass writing two planes, or
onto the VAAPI VPP engine as a blit.

Two planes is where it gets awkward, and the crate survey's §2b already
documents why the obvious dodge does not work: Y as `R8Unorm` and UV as
`Rg8Unorm` are two views onto **one** memory object, and WebGPU's
copy-compatibility rules object downstream. The survey says this about import;
it is symmetric.

The VPP route is likely better here than it is for decode: we would hand VAAPI
an imported RGBA surface and let the fixed-function scaler produce NV12, which
is a hardware block that is idle during encode anyway. `vpp_vaapi` exists as an
FFmpeg filter; driving it through `ffmpeg-next`'s filter wrapper is unverified.

### 3. Intel tiling, which the survey warns about and which applies here too

A wgpu-allocated `VkImage` on ANV will be tiled — `Y_TILED` or worse on Raptor
Lake — and the `VK_EXT_image_drm_format_modifier` dance is how you find out
which. VAAPI's import will reject a modifier it does not handle. The iroh-live
workaround the survey cites (a VAAPI VPP blit to re-tile) applies unchanged, and
if we are doing a VPP pass for the colour conversion anyway, that is the same
pass. **This is an argument for doing the conversion on the VPP engine rather
than in a wgpu shader.**

Alternatively the export image can be allocated `LINEAR` (`DRM_FORMAT_MOD_LINEAR`),
which every importer accepts and which costs bandwidth on the render side. For a
frame that is written once and read once, linear is very possibly the right
answer, and it is much less code. Try it first.

### 4. Synchronisation

wgpu's submission has to complete before VAAPI reads the surface. The available
tools:

- `wgpu_hal::vulkan::Queue::add_wait_semaphore` (wgpu 30, PR #9461) lets the
  *GPU* wait rather than the CPU, but it is the wrong direction — we need VAAPI
  to wait on wgpu.
- A `VkSemaphore` exported as a sync-file fd, handed to VAAPI. libva has no
  general API for this; the DRM implicit-sync path (the fence attached to the
  DMA-BUF) is what actually gets used in practice and is what GStreamer relies
  on.
- The blunt instrument: `device.poll(Wait)` after submit. Costs a CPU stall per
  frame, but it is a *stall*, not a copy, and against 4 memory round trips it
  will still be faster. **Start here.** Optimising the sync before the copy is
  removed would be measuring the wrong thing.

## What I would do, in this order

1. **Measure the readback in isolation first.** `Compositor::render_frame`'s
   map-and-copy is step ①; instrument it. If it is 4 ms of a 15 ms frame, the
   ceiling on all of this work is 35%, and that number should be known before
   anyone writes `ash` code. It was not measured during this session and it
   should have been.
2. **Try `LINEAR` before anything tiled.** It removes blocker 3 entirely at a
   known, bounded cost.
3. **Do the RGBA→NV12 on the VPP engine, not in a shader.** It is idle, it is
   one `av_hwframe_map` plus a filter graph rather than a second render pass,
   and it subsumes the re-tiling if `LINEAR` turns out not to be enough.
4. **`device.poll(Wait)` for synchronisation** until the copy is gone and the
   sync is measurably the next thing.
5. Only then consider a fenced path.

## The thing that might make all of this unnecessary

`gpu-video` 0.4.0 (Software Mansion, MIT) does **Vulkan Video** encode directly
into and out of `wgpu::Texture`, with `VideoDeviceExt` traits on
`wgpu::Device`. If Vulkan Video encode works on this iGPU, it deletes DMA-BUF
export, DRM modifiers, multi-plane import and the VPP blit from the problem in
one move. Its README claims H.264 encode and H.265 encode.

The catch that the survey names — no H.265 *decode* — does not apply to the
export path, which only encodes. So the argument against it is weaker here than
it was there.

It is pinned to wgpu 29 and we are on 30, which is the real cost.

**`cargo run --example print_hw_capabilities` from that crate is still the
cheapest experiment in this entire area and it still has not been run.** Do it
before writing any `ash`.

## What was verified during this session, and what was not

Verified:

- `av_hwdevice_ctx_create`, `av_hwframe_ctx_alloc/init`, `av_hwframe_get_buffer`
  and `av_hwframe_transfer_data` all work from `ffmpeg-next` 6.1's `ffi`
  re-export, on this machine, against the iHD driver. The wrapper is in
  `export/hwframes.rs`.
- `h264_vaapi` and `hevc_vaapi` encode correct, decodable files from uploaded
  NV12 surfaces.
- `av1_vaapi` is in the FFmpeg build and this chip cannot encode AV1: `vainfo`
  reports `VAProfileAV1Profile0` for `VAEntrypointVLD` only.

Not verified, and stated as unknown rather than assumed:

- Whether `av_hwframe_map` in the *import* direction (DRM_PRIME → VAAPI) works
  on this driver at all. Nothing in this session exercised it.
- Whether the compositor's readback is a large or small share of the frame
  budget. This is the number that sizes the whole project and it is missing.
- Whether `vpp_vaapi` composes through `ffmpeg-next`'s filter wrapper.
- Anything about Vulkan Video on this chip.
