# GPUI wgpu-texture spike — findings

De-risking spike for the native rewrite (Tauri+React → GPUI). Question: can GPUI
display a wgpu texture the engine rendered, ideally in one shared GPU device, so
the video preview needs no JPEG/IPC/readback? Spike code lives in
`_scratch/gpui_spike/` (throwaway). Result: `cargo +1.97.1 check` EXIT=0, clean,
the full gpui + gpui_platform + wgpu tree builds and our code type-checks against
the real current API (zed commit b1a7ef0, gpui 0.2.2).

## Headline — the premise changed

**Upstream GPUI no longer uses `blade`; it uses `wgpu`.** Zed merged PR #46758
"gpui: Remove blade, reimplement linux renderer with wgpu" (2026-02-13). Proven
from the resolved graph, not just the PR: in the spike `Cargo.lock` `blade`
appears 0 times, while `wgpu`, `wgpu-core`, `wgpu-hal`, `naga` are direct gpui
deps. **gpui pulls wgpu-hal/naga v29.** Any docs snapshot still showing
`platform/blade/blade_renderer.rs` is stale.

Consequence: we do NOT need a from-scratch wgpu backend for gpui (the thing that
made Kiru's `gpui_wgpu` special is now largely upstream). True zero-copy is
realistic. The only missing upstream piece on Linux is a public "display my own
wgpu texture" element.

## Real GPUI API today (commit b1a7ef0 / gpui 0.2.2)

- Renderer: wgpu on Linux (x11/wayland). Backend is a direct platform dep, not a
  cargo feature — not toggleable. Default features `["font-kit","wayland","x11","windows-manifest"]`.
- App entry moved crates: `gpui_platform::application() -> gpui::Application`.
  Must depend on `gpui_platform` too.
- `gpui::canvas(prepaint, paint)` — paint may only call `Window::paint_*`
  primitives, **no GPU device access**.
- `gpui::surface(source) -> Surface` is **macOS-only** (`#[cfg(target_os="macos")]`,
  wraps a CoreVideo `CVPixelBuffer`). **No Linux bring-your-own-texture element upstream.**
- CPU path that works today: `img(src)` + `RenderImage::new(SmallVec<[image::Frame;1]>)`
  (stores BGRA) → `Window::paint_image(bounds, corner_radii, Arc<RenderImage>, frame_index, grayscale)`.

## Can we depend on a wgpu external-texture element?

- Upstream renderer is wgpu, but no public external-surface element on Linux.
- **Kiru's stack is private** — crates `wgpui` / `wgpui-component` (forks of gpui
  and longbridge/gpui-component), with `gpui_wgpu`, `WgpuExternalSurface`, and
  zero-copy `import_dmabuf_to_wgpu_texture` via Vulkan `external_memory_fd` +
  `create_texture_from_hal` + `zwp_linux_dmabuf`. Not published ("contact us for source").
- **Public community forks exist:** `mdeand/gpui-wgpu` (Apache) adds a
  `WgpuSurface` element + `wgpu_surface(handle)` with a triple-buffered registry
  and `present_synced(SubmissionIndex)` — an external thread renders into
  gpui-managed wgpu textures, zero readback (no dmabuf import though). Also
  `ahkohd/ggpui`. Both track gpui-ce and may lag.

## Integration paths, ranked (scrubbing perf)

**(c) CPU readback → `RenderImage`/`img` — START HERE.** Works on upstream gpui
today, zero fork; this is what the spike builds end to end. Cost per displayed
frame: one `copy_texture_to_buffer` + `map_async` (a full GPU→CPU sync stall)
≈ 8 MB @1080p / 33 MB @4K, then a CPU→GPU re-upload into gpui's atlas. ~16 MB
round-trip + a pipeline stall per frame. Throws away the engine's on-GPU
locality, but has **no JPEG, no IPC, no webview** — already far better than the
current Tauri path. Fine for a 1080p preview MVP and as an A/B correctness ref.
The map-stall is what hurts fast 4K scrubbing.

**(a) Shared wgpu device — engine renders into a gpui-owned wgpu texture,
composited by a custom `Element` — THE TARGET.** Now realistic without a backend
rewrite because gpui is already wgpu. Either patch gpui to expose its
`wgpu::Device`/`Queue` + add an `ExternalSurface`-style Element (small diff), or
adopt `mdeand/gpui-wgpu`'s `WgpuSurface`. Kills the ~16 MB/frame + the map stall
→ smooth 4K scrubbing. **Caveat: gpui is on wgpu 29; chukcut's engine is on
wgpu 30.** For a shared-device handoff `wgpu::Texture` must be one type across the
boundary, so align the engine to wgpu 29.

**(b) Separate devices, dmabuf FD import (kiru-style) — cross-device fallback.**
wgpu `vulkan::Device::texture_from_dmabuf_fd()` (feature `VULKAN_EXTERNAL_MEMORY_FD`/
`_DMA_BUF`); the engine already has dmabuf plumbing (`render::dmabuf::import_plane`,
`preview/zerocopy.rs`, `preview/vasurface.rs`). Version-agnostic across the FD, so
it sidesteps the wgpu-29-vs-30 mismatch — at the cost of the export/import dance.

## Recommendation

Ship **(c)** now for a working preview with zero fork. Move to **(a)** for
production scrubbing by aligning chukcut on **wgpu 29** and adding a shared-device
external-surface Element (upstream patch or the mdeand fork). Keep **(b)** as the
cross-device fallback. Do NOT build a wgpu backend from scratch — that work is
already upstream.

## Build notes for the real port

- Needs **Rust 1.97.1** (zed `rust-toolchain.toml` pin). Default 1.91 fails on
  `gpui_util`'s unstable `slice_as_array`. Install with `rustup toolchain install 1.97.1`.
- API drift seen: app entry is `gpui_platform::application()` (not `Application::new()`);
  entities via `cx.new(...)` with `gpui::prelude::*` in scope; wgpu 22 `entry_point`
  is a plain `&str`, not `Option<&str>`.
- Debug link of gpui is heavy — this box (~2 GB free, memguard active) can build
  but a full windowed `cargo run` risks OOM. Real dev needs more headroom or a
  release/thin-LTO profile tuned for RAM.
