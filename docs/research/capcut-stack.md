# What CapCut actually is

Notes from reverse engineering, kept because they justify several architecture
decisions elsewhere in these docs. Sources: `otool -L` on CapCut.app 8.5.0
(macOS, machine since decommissioned), a file survey of CapCut 9.0.0.3858 on
Windows, and the CapCut Web JavaScript bundles.

Nothing here was decompiled. Library inventories, file listings and
already-public JavaScript only.

## Desktop (Windows 9.0.0.3858)

Installed under `%LOCALAPPDATA%\CapCut\Apps\<version>\`.

| Component | Size | Role |
|---|---|---|
| `VECreator.dll` | 236 MB | Video engine creator API |
| `cccreator.dll` | 150 MB | Color correction / caption creator |
| `lens.dll` | 84 MB | Real-time ML video effects |
| `videoeditor.dll` | 63 MB | Timeline and render graph core |
| `libcef.dll` | 218 MB | Chromium Embedded Framework |
| `lynx.dll` + `lynx_core_dev.js` | 24 MB | ByteDance's React-Native equivalent |
| `openvino.dll` (+ NPU plugin) | 17 MB | ML inference (CoreML's role on macOS) |
| `avcodec-61` etc. | 30 MB | Custom FFmpeg 6.1+ build |
| Qt6 Core/Gui/Quick/Qml/Widgets | 25 MB | UI shell |
| `EffectPlatform.dll` | 1.5 MB | Effect/filter engine |
| `clipflow_sdk.dll` | 1.7 MB | Clip flow logic |
| `VEAngle/libGLESv2.dll` | 5.7 MB | **ANGLE** — GLES translated to D3D11 |

### The two conclusions that matter

**1. The UI is a browser.** Qt provides the window and the native widgets;
Chromium renders the panels; Lynx runs JavaScript views. ByteDance did not
build this UI natively, and they have more engineers than we will ever have.
This is the direct justification for
[`overview.md`](../architecture/overview.md)'s process split.

**2. The engine speaks GLES, not D3D.** `VEAngle/libGLESv2.dll` means the
renderer targets OpenGL ES and ANGLE translates to Direct3D 11 on Windows. It
also explains the shader corpus found during effect reverse engineering: GLSL
ES 1.0 almost everywhere, ES 3.0 occasionally. A wgpu-based compositor sits at
the same abstraction level and can consume the same shader dialect after
translation.

## Web

`https://www.capcut.com/editor` loads:

- `vesdk-lvapi.wasm` / `vesdk-lvapi-opt.wasm` — the video engine, compiled to
  WebAssembly
- `libffmpeg.wasm` / `libffmpeg_simd.wasm` — FFmpeg, likewise
- `editor.*.js` (3.3 MB) — the entire editor UI in TypeScript

So the same engine ships three ways: native on desktop, WASM in the browser.
The UI is TypeScript in both cases.

## The project format

`%LOCALAPPDATA%\CapCut\User Data\Projects\com.lveditor.draft\<name>\` contains
plain-JSON `draft_content.json` files, several hundred kilobytes each.

Top level: `canvas_config`, `color_space`, `fps`, `duration`, `materials`,
`tracks`, `keyframes`, `keyframe_graph_list`, `relationships`, `extra_info`.

`materials` groups roughly fifty categories: `videos`, `audios`, `texts`,
`text_templates`, `stickers`, `transitions`, `video_effects`, `plugin_effects`,
`material_animations`, `speeds`, `chromas`, `color_curves`, `hsl`, `hsl_curves`,
`primary_color_wheels`, `log_color_wheels`, `common_mask`, `video_radius`,
`video_shadows`, `video_strokes`, `vocal_separations`, `digital_humans`,
`smart_crops`, and so on.

A representative project had 21 tracks — 14 video, 4 text, 3 audio, 1 sticker —
with 45 segments on the main lane.

That category list doubles as a feature specification: each one is something
the editor can do, named the way its authors named it. Our
[`project format`](../architecture/project-format.md) borrows the structural
decisions (material pool by id, target vs source ranges, microsecond times,
explicit render index) and none of the content.

## Assets

`User Data\Cache\` holds the downloaded material: 312 MB of effects, 231 MB in
`ressdk_db`, 239 MB of images, 771 MB for SmartCrop models, plus music, fonts
and recognition models.

These are ByteDance's. They do not enter this repository or any build we ship.
The effect runtime is designed to load packages from a URL the user provides.
