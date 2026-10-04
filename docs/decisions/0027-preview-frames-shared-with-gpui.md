# 0027 — Preview frames reach GPUI as exported GPU memory, through a patched GPUI

Date: 2026-10-04. Status: accepted (gputex agent).

## What was decided

The preview no longer reads frames back by default. The engine's player
swizzles each composited frame into a BGRA buffer whose memory is **exported
as an opaque file descriptor** (`render::shared_frame`); a patched GPUI
**imports that memory into its own Vulkan device** once per buffer and copies
each new frame into a texture with one `copy_buffer_to_texture` before it
draws (`vendor/gpui-pre-wgpu/src/external_buffer.rs`). The app paints it with
`Window::paint_external_buffer`, a Linux counterpart of macOS's
`paint_surface` that the patch adds.

- **The patch.** `gpui-pre` and `gpui-pre-wgpu` 0.3.7 are vendored under
  `vendor/` and selected with `[patch.crates-io]`. The diff is about 580
  lines: a field on `PaintSurface`, two types (`ExternalBufferInfo`,
  `ExternalBuffer`), one `Window` method, the import, and drawing
  `PrimitiveBatch::Surfaces` with the existing polychrome sprite pipeline. No
  new shader. `vendor/README.md` lists every spot and how to re-apply it on a
  `gpui-kit` bump.
- **A buffer, not an image.** The exported object is a `VkBuffer` with rows
  padded to 256 bytes. A buffer has no layout and no tiling, so neither side
  needs layout transitions it cannot express through wgpu, and no DRM format
  modifier is negotiated. The one copy into GPUI's texture costs ~0.1 ms at
  4K on the GPU.
- **Opaque fd, not DMA-BUF.** Both devices are in one process on one GPU and
  driver, which is exactly the case opaque fds exist for; NVIDIA's driver and
  Mesa (including lavapipe) export and import them for buffers. Import
  requires matching device and driver UUIDs (`VkPhysicalDeviceIDProperties`),
  the same memory type and size, and an identically created buffer for the
  dedicated allocation — all carried in `ExternalBufferInfo`.
- **Synchronisation by completion and liveness.** The engine hands a frame out
  only after its submission finished (`on_submitted_work_done`). A buffer is
  reused only when the engine's pool holds the last reference: the UI holds
  the frame while it is on screen, GPUI's scene while it is painted, and
  GPUI's renderer until its copy has finished on GPUI's queue. No semaphores.
- **Readback stays, as the fallback.** It is chosen when
  `CHUKCUT_PREVIEW_READBACK=1` is set, when the engine's device cannot export
  (not Vulkan, no `VK_KHR_external_memory_fd`: `Sharing::Unavailable`), and
  automatically when GPUI's renderer cannot import (another GPU — a hybrid
  laptop where GPUI follows the compositor's iGPU and the engine took the
  discrete one —, another driver, not Vulkan): the renderer sets the
  `ExternalBuffer`'s failure flag, the app's next `Player::take` turns sharing
  off, and the player re-renders the current request as bytes.

## The rule "never open a GPU device yourself"

Unchanged. `modules::gpu` still owns the engine's one device, GPUI still owns
its own, and nothing opens a third: the engine exports memory from the device
it has, GPUI imports into the device it has. The exception the rule already
named (GPUI's renderer device) is now also the importer; that is written next
to the rule in `CLAUDE.md`. Tests and the benchmark open a wgpu 29 device to
stand in for GPUI's window device, because the real one exists only with a
window.

## Why

- **The readback was the largest cost left in the preview at 4K**, and it
  cost twice: on the render thread (GPU → CPU) and on the UI thread (a
  frame-sized copy, a new texture and a staging copy per frame). Measured on
  the RTX 3060 at 30 fps playback: the UI thread's work per frame went from
  2.3–2.9 ms (1080p) and 10–15 ms (4K) to 0.05–0.08 ms at either size, and
  the process's CPU from 17–22 % to 5–7 % of a core at 1080p and from
  88–111 % to 14–39 % at 4K. Method and all numbers:
  `docs/research/gpui-shared-texture.md`, "What was built".
- **No wgpu alignment.** GPUI links wgpu 29, the engine wgpu 30. A shared
  device (option (a) of the research note) would have forced the engine, its
  effect runtime and its DMA-BUF code onto GPUI's wgpu and re-done that on
  every `gpui-kit` bump. A file descriptor is version-agnostic.
- **Smallest fork that removes both copies.** Option (c), reusing the atlas
  texture, would have kept the readback and one upload.

## What it costs

- A fork of two GPUI crates, 3.4 MB of vendored source, re-applied on every
  `gpui-kit` bump (`vendor/README.md`). Upstreaming the element to Zed would
  remove it.
- Device memory: the pool keeps up to 12 frame buffers per size (about 8 in
  steady playback: four rendered ahead, one on screen, one in GPUI's queue,
  one being written). At 4K that is ~265 MB of VRAM in steady playback, where
  the readback ring held the same frames in system memory.
- Two Vulkan devices still exist in the process, as before.

## What would change our minds

- Upstream GPUI gains a way to draw a foreign texture on Linux: drop the fork.
- GPUI and the engine end up on one wgpu: share the device instead and drop
  the import (option (a)).
- A driver that tears or shows stale frames with the completion-only
  synchronisation: add an exported semaphore (`VK_KHR_external_semaphore_fd`).
