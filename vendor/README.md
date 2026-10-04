# Vendored crates

Two crates of GPUI, patched. Everything else comes from crates.io.

| Directory | Crate | Version | Pinned by |
|---|---|---|---|
| `gpui-pre/` | `gpui-pre` (Zed's `gpui`, snapshot of zed@1a28cff) | 0.3.7 | `gpui-kit 0.7.0` (`=0.3.7`) |
| `gpui-pre-wgpu/` | `gpui-pre-wgpu` (Zed's `gpui_wgpu`) | 0.3.7 | `gpui-pre-linux 0.3.7` |

The workspace `Cargo.toml` points `[patch.crates-io]` at them. They are the
published crates with the `[[example]]`, `[[test]]` and `[[bench]]` tables
removed from the manifests and those directories left out; nothing else was
dropped. Licence: Apache-2.0, as upstream (`LICENSE-APACHE` in each).

## Why

The preview used to read every frame back from the engine's GPU and upload it
again into GPUI's. GPUI on Linux had no way to draw anything but its own atlas.
The patch adds that way, as small as it could be made:
`docs/decisions/0027-preview-frames-shared-with-gpui.md`.

## The patch

Every patched spot is marked `chukcut patch (vendor/README.md)`.

`gpui-pre`:

- `src/scene.rs` — `PaintSurface` gains `external: ExternalBuffer` on Linux
  (the counterpart of macOS's `image_buffer`); new types `ExternalBufferInfo`
  (fd, sizes, memory type, device/driver UUIDs, layout) and `ExternalBuffer`
  (the info, an owner `Arc` that keeps the memory from being rewritten, a
  failure flag).
- `src/window.rs` — `Window::paint_external_buffer(bounds, ExternalBuffer)`,
  the Linux counterpart of `paint_surface`.

`gpui-pre-wgpu`:

- `src/external_buffer.rs` (new) — imports the memory into the renderer's own
  device once per allocation (`VkImportMemoryFdInfoKHR`, opaque fd, dedicated
  allocation), copies each new picture into a `Bgra8Unorm` texture with
  `copy_buffer_to_texture`, reports failure through the `ExternalBuffer`.
  `import_external_buffer` is public for tests and benchmarks.
- `src/wgpu_renderer.rs` — one more `InstanceBinding` (a polychrome sprite per
  surface, `external_surface_sprites`), the copies recorded before the main
  pass, `PrimitiveBatch::Surfaces` drawn with the existing polychrome sprite
  pipeline, the owners held until `on_submitted_work_done`.
- `src/gpui_wgpu.rs` — the module and the one re-export.
- `Cargo.toml` — `ash 0.38` on Linux (the version `wgpu-hal 29` uses).

About 580 lines with their comments; no new shader, no new pipeline.

## Re-applying it on a `gpui-kit` bump

1. Find the `gpui-pre` version the new `gpui-kit` pins (`cargo tree -p
   gpui-kit`).
2. Copy the new published crates in (`~/.cargo/registry/src/*/gpui-pre-X/`:
   `src`, `build.rs`, `Cargo.toml`, `README.md`, `LICENSE-APACHE`; for
   `gpui-pre-wgpu`: `src`, `Cargo.toml`, `LICENSE-APACHE`), and strip the
   target tables from both manifests.
3. Apply the diff of the last patch commit to the new sources:
   `git log -- vendor/` names it; `git show <commit> -- vendor/ | git apply -3`
   usually takes it with fuzz, the marked spots say what to redo by hand.
4. Run the shared-preview tests (`cargo test -p chukcut player::tests`) on the
   real GPU and on lavapipe, and look at the preview.

Upstreaming the element to Zed (a Linux `surface()`) would make the fork
unnecessary.
