# Showing an engine texture in GPUI without a readback — what blocks it

Status: investigated 2026-10-03 by the perf agent; **implemented 2026-10-04**
by the gputex agent as a variant of option (b) — see "What was built" at the
end, and `docs/decisions/0027-preview-frames-shared-with-gpui.md`. The rest of
this file is the investigation as it was written, and still explains why a
patched GPUI was unavoidable. It supersedes the "path (a)" paragraph of
`GPUI_SPIKE.md`, which assumed the snapshot it read was the one we would link.

## What the app links today

| | crate | wgpu |
|---|---|---|
| UI | `gpui-kit 0.7.0` → `gpui-pre 0.3.7` + `gpui-pre-wgpu 0.3.7` + `gpui-pre-linux 0.3.7` (snapshot of zed@1a28cff) | **29.0.4** |
| engine | `chukcut-engine` (`wgpu 30.0.0`, `wgpu-hal 30` + `ash 0.38` for DMA-BUF, `naga 30` for the effect runtime) | **30.0.0** |

Two wgpu versions are in the graph (`grep -n 'name = "wgpu"' Cargo.lock`), so a
`wgpu::Texture` from the engine is not a type GPUI can accept even if it could
reach GPUI's device.

## Where the frame goes in GPUI (measured and read, not guessed)

1. `Window::paint_image` → `sprite_atlas.get_or_insert_with(RenderImageParams)`.
2. `WgpuAtlas::upload_texture` copies the bytes (`swizzle_upload_data` →
   `bytes.to_vec()`, or a CPU BGRA→RGBA swap on an adapter without BGRA atlas
   textures) and queues them.
3. A preview frame is larger than the 1024² default atlas texture, so
   `push_texture` creates **a new texture the size of the frame for every
   frame**; dropping the previous image (`cx.drop_image`) frees the old one.
4. `flush_uploads` → `queue.write_texture`, which copies into wgpu's staging
   buffer once more.

So per frame on the UI thread: one frame-sized allocation and memcpy, a texture
creation, and a staging copy. `examples/player_bench.rs` emulates exactly that
and reports it as "UI upload": 2.7 ms at 1080p and ~10–25 ms at 4K on the RTX
3060 under load. It is not on the render thread, so it costs UI smoothness
rather than preview fps — but at 4K it is the largest single item left.

## Why there is no cheap way in

- **GPUI's device is private.** `gpui-pre-linux` creates the `WgpuContext`
  lazily inside the X11/Wayland client state (`gpu_context:
  Rc<RefCell<Option<WgpuContext>>>`) and never hands it out. `WgpuContext` has
  public `device`/`queue` fields, but nothing public leads to it.
- **There is no primitive for a foreign texture on Linux.** `gpui::surface()`
  is macOS-only (a `CVPixelBuffer`). The scene knows sprites from its own
  atlas, paths, quads, shadows, underlines — nothing that carries an arbitrary
  `TextureView`.
- **The engine cannot simply follow GPUI's wgpu.** Going to 29 means the
  effect runtime's `naga 30` SPIR-V path, `wgpu-hal`/`ash` for DMA-BUF, and the
  API drift between 29 and 30 (`immediate_size`, `Option` bind group layouts)
  — while the effects agent is changing exactly that code. And `gpui-kit`
  pins its own snapshot, so the next `gpui-kit` bump moves the target again.

Every option therefore needs **a patched GPUI**. The question is only how
small the patch is and whether the wgpu versions have to match.

## Options

### (a) One device, shared

Patch three crates (`[patch.crates-io]` to vendored copies under `vendor/`):

1. `gpui-pre-linux`: accept a `WgpuContext` from the app (or expose the one it
   made) — `Application::with_wgpu_context(...)` or a getter on `App`.
2. `gpui-pre` (core): a new paint primitive, `Window::paint_texture(bounds,
   TextureHandle)`, carried through `Scene` like `PaintSurface` is on macOS.
3. `gpui-pre-wgpu`: draw it — one more pipeline that samples a caller-provided
   `TextureView` (the sprite shader with a per-draw bind group).

Then the engine renders into a texture on GPUI's device and the element draws
it. Cost: the engine on wgpu 29 *and* the engine's `gpu::render_context()`
created from GPUI's adapter, so the decode imports (VAAPI DMA-BUF, NVDEC
upload) and the export share it. Blocked on the wgpu alignment above.

### (b) Two devices, DMA-BUF between them — recommended

The same GPUI patches 2 and 3, but instead of patch 1 the element receives a
**DMA-BUF file descriptor** and imports it on GPUI's device with
`wgpu_hal::vulkan::Device::texture_from_raw` over a `VkImage` created with
`VkExternalMemoryImageCreateInfo` + `VkImportMemoryFdInfoKHR`
(`VK_EXT_external_memory_dma_buf`, `VK_KHR_external_memory_fd`). The engine
already exports memory as DMA-BUF (`render::dmabuf`, used by the zero-copy
VAAPI export) — it would export a ring of 3 BGRA images instead of NV12
buffers.

- **No wgpu alignment.** The FD is version-agnostic; GPUI keeps 29, the engine
  keeps 30.
- **Synchronisation:** the engine waits for its submission (it already does on
  the zero-copy paths) before publishing the slot index; GPUI samples it on its
  next frame; a slot is reused only after GPUI has drawn a newer one (the
  player's ring already tracks "taken"). An explicit sync FD
  (`VK_KHR_external_semaphore_fd`) is the refinement, not a requirement.
- **Both vendors:** NVIDIA's proprietary driver and Mesa (Intel, AMD) both
  expose `VK_EXT_external_memory_dma_buf` for images with
  `DRM_FORMAT_MOD_LINEAR`; `render::dmabuf` probes it today.
- **Fallback:** the readback path stays, selected when the import fails, as the
  VAAPI paths do.

### (c) Keep the readback, stop GPUI churning the atlas

Patch only `gpui-pre-wgpu`: reuse the previous frame's atlas texture when the
size matches and `write_texture` into it, skipping the `to_vec`. Removes the
texture creation and one copy per frame; keeps one readback and one upload.
Small, but it is still a fork for a partial win.

## The plan for (b), step by step

1. Vendor `gpui-pre`, `gpui-pre-wgpu` at the exact versions `Cargo.lock` has;
   `[patch.crates-io]` them in the workspace `Cargo.toml`. Build unchanged.
2. `gpui-pre`: `pub struct ExternalTexture { fd: OwnedFd, width, height,
   stride, modifier, generation }`, a `PrimitiveBatch::External` in the scene,
   `Window::paint_external(bounds, Arc<ExternalTexture>)`.
3. `gpui-pre-wgpu`: on first sight of a `(fd identity, generation)`, import it
   (`ash` from `wgpu::hal` on its own device), cache the `wgpu::Texture` per
   ring slot, draw with the sprite pipeline. Drop the cache entry when the
   element stops referencing it.
4. Engine: `render::dmabuf::ExportedImage` — a `Bgra8Unorm` render target with
   exportable memory, a ring of 3; `FramePlayer` gains a mode that composites
   into the ring and publishes `(slot, time)` instead of bytes. The swizzle
   compute pass becomes a blit into the slot (the compositor target is sRGB
   RGBA; the slot is what GPUI samples).
5. App: `player.rs` paints `paint_external` when the player reports the
   zero-copy mode, `RenderImage` otherwise.
6. Measure with `player_bench` (the UI-upload column goes to zero; the
   readback column to zero) and `tests/every_card.rs` on NVIDIA and Intel.

Estimated size: ~400 lines in the GPUI fork, ~300 in the engine. The fork is
the cost to weigh: every `gpui-kit` bump re-applies it. Upstreaming patch 2/3
to zed (a Linux counterpart of `surface()`) is the way to make it free.

## What was built (2026-10-04)

Option (b), with three changes against the plan above, each made because the
plan's version needed something wgpu cannot express:

- **A buffer, not an image.** The engine exports a `VkBuffer` (BGRA rows
  padded to 256 bytes), not a `Bgra8Unorm` render target. An imported image
  would need its layout handed over (`QUEUE_FAMILY_EXTERNAL` release and
  acquire) and, as a DMA-BUF, a DRM modifier; wgpu does neither, and its first
  use of a texture made with `create_texture_from_hal` transitions from
  `UNDEFINED`, which may discard the contents. A buffer has no layout. GPUI
  copies it into a texture of its own with `copy_buffer_to_texture` — one GPU
  copy, ~0.1 ms at 4K.
- **An opaque fd, not a DMA-BUF.** Both devices are in one process on one GPU
  and driver. `VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT` is the handle
  type for exactly that, NVIDIA and Mesa export and import buffers with it,
  and the importer checks `deviceUUID`/`driverUUID` instead of probing
  modifiers. **Lavapipe supports it too** (Mesa 25.2 here), so the shared path
  runs on the software adapter as well; the readback is the fallback only
  where a side cannot export or import.
- **No new pipeline.** The element is drawn as a polychrome sprite (the image
  pipeline) whose atlas tile is the whole copied texture: corner radii,
  opacity and content masks come for free, and there is no new shader.

The engine side: `render::shared_frame` (`SharedFrames`, the `BgraReadback`
shape: submit, `try_collect`, `collect_oldest`), `render::dmabuf`'s
`ExportableBuffer` with an `ExportHandle`, and `FramePlayer::use_shared_frames`
with `FramePixels::{Bgra, Shared}`. The GPUI side: `vendor/README.md`. The app
side: `crates/app/src/player.rs` (`Picture`, the automatic fallback).

Synchronisation is completion plus liveness, no semaphore: the engine hands a
frame out after its submission finished; a buffer is reused only when the
engine's pool holds the last `Arc`, and GPUI's renderer holds one until its
copy finished (`on_submitted_work_done`). One trap found on the way:
`on_submitted_work_done` fires for the queue's *last* submission at
registration, which may be another thread's later one — so it is late, never
early, and `collect_oldest` trusts the submission-index wait instead.

### Measured, RTX 3060, driver 610.57, 2026-10-04

`cargo run --release -p chukcut --example preview_share_bench`: a 30 fps
H.264 clip played in real time for 6 s through `FramePlayer`; a second wgpu 29
device does what GPUI's renderer does with each frame (readback arm: copy the
bytes, create a frame-sized texture, `write_texture`; shared arm: import once
per buffer, one `copy_buffer_to_texture`). Load average 7–8 from other agents'
builds; three runs, ranges:

| | UI thread per frame | UI worst frame | player latency | process CPU |
|---|---:|---:|---:|---:|
| 1080p, readback | 2.3–2.9 ms | 7.5–20 ms | 3.9–5.6 ms | 17–22 % |
| 1080p, shared | 0.05–0.08 ms | 0.2–0.4 ms | 2.1–2.5 ms | 5–7 % |
| 4K, readback | 10.4–14.9 ms | 28–33 ms | 21–27 ms | 88–111 % |
| 4K, shared | 0.05–0.08 ms | 0.3–0.4 ms | 5.8–15.8 ms | 14–39 % |

All arms showed 180 of 180 frames. "Player latency" is the player's smoothed
start-to-ready time per frame (`PlayerStats::latency_ms`); "process CPU" is
user + system time over wall time, 100 % = one core, decoding included. At 4K
the readback arm spent a whole core moving pixels; the shared arm's UI work
per frame no longer depends on the frame size.

The real app was checked on a private Xvfb display on both adapters: GPUI
picks the RTX 3060 (Vulkan) there and the log says `sharing=Shared`; on
lavapipe likewise. Playback in the real window was not driven (it would play
audio on the owner's speakers); the harness above is the playback measurement.
