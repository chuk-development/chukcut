# Notices

## Licence

chukcut is free software under the **GNU General Public License, version 3 or
later**. The full text is in [`LICENSE`](LICENSE). There is no warranty, to the
extent permitted by law.

## Third-party components

chukcut links or bundles the following. Each keeps its own licence; this list
is a map, not a substitute for those texts.

| Component | Licence | Used for |
|---|---|---|
| FFmpeg (`libavcodec`, `libavformat`, `libavutil`, `libswscale`, `libswresample`, `libavfilter`) | LGPL-2.1+, or GPL-2+ when built with `--enable-gpl` | Demux, decode, encode, mux |
| `x264`, `x265` (only in a `--enable-gpl` FFmpeg build) | GPL-2+ | Software H.264 / H.265 encoding |
| libva and the platform media driver | MIT | Hardware decode and encode |
| wgpu, naga | MIT / Apache-2.0 | GPU compositing and shader translation |
| glslang, SPIRV-Tools, shaderc | Apache-2.0 | GLSL → SPIR-V for the effect runtime |
| GPUI (Zed) | Apache-2.0 | User interface |
| gl-transitions (120 transitions, translated to WGSL, in `crates/engine/src/modules/transitions/library/gl/`) | MIT; `InvertedPageCurl` BSD-3-Clause, `StereoViewer` BSD-2-Clause | Library transitions; authors and licence texts in `transitions/library/LICENSE-gl-transitions.md` |

`cargo tree` enumerates the complete set.

## FilmCraft (storytold/filmcraft), Apache-2.0

Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors. FilmCraft is
licensed under the MIT License or the Apache License, Version 2.0, at the
user's option (<https://github.com/storytold/filmcraft>):

> FilmCraft. Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
> Licensed under the MIT License or the Apache License, Version 2.0, at your
> option.

chukcut takes it under the **Apache License, Version 2.0**, which is
compatible with the GPL-3.0; the full text is in
[`.github/LICENSE-APACHE-2.0.txt`](.github/LICENSE-APACHE-2.0.txt). Files
copied or closely adapted from FilmCraft keep its copyright line at their top
and say what was changed. Our changes to them are under the GPL-3.0. The
ArtCraft and FilmCraft names and logos are their trademarks; none of their
brand assets are in this repository. FilmCraft's NOTICE has no further
attribution requirements for these parts.

FilmCraft's shared workflows,
[craft-common-actions](https://github.com/storytold/craft-common-actions),
carry no licence file. Nothing was copied from them; the workflow logic we
needed was rewritten.

What was taken, deliberately and with thanks:

- **Release pipeline**: `.github/workflows/release.yml`,
  `.github/actions/resolve-version`, `.github/actions/draft-release`, and the
  AppImage update information in `packaging/appimage/build-appimage.sh`.
  [Decision 0037](docs/decisions/0037-release-workflow-from-filmcraft.md)
  lists what was taken and what was left out on purpose (their craft-fonts
  embedding).
- **MCP server and control socket design**, from FilmCraft's
  `crates/automation` (`server.rs`, `bridge.rs`, `long_job.rs`),
  `apps/filmcraft/src/control_server.rs`, `docs/agents.md` and
  `docs/control-protocol.md`:
  - the tool set and names: `command_list`, `command_run`, `command_batch`,
    `doc_inspect`, `render_preview`, `project_inspect`, `sequence_inspect`,
    `media_import`, `render_frame`, and the live-app tools `ui_inspect`,
    `ui_click`, `ui_drag`, `ui_key`, `ui_type`, `ui_screenshot`,
    `ui_control`;
  - the two modes, headless and `--bridge` to the running app, and the
    `.mcp.json` that registers both;
  - the conventions: a title and all four annotation hints on every tool,
    unknown arguments as JSON-RPC `-32602` naming the accepted ones,
    `-32700` for a line that is not JSON, the `document` and `commands`
    resources, the MCP 2026-07-28 cache hints, progress and cancellation of
    a long export with no response to a cancelled request;
  - the control channel: one JSON request per line and one reply per line,
    only well-formed requests read, a 4 MiB line cap and 16 connections, the
    `engine.*` / `ui.*` method split;
  - the shape of the tests that check all of the above.

  `crates/cli/src/mcp.rs`, `crates/cli/src/control.rs` and
  `crates/cli/src/bridge.rs` are adapted from FilmCraft. The code was
  rewritten for chukcut (no rmcp or tokio, chukcut's operations, a Unix
  socket); no FilmCraft file is copied verbatim.
- **Audio effects** (`crates/engine/src/modules/audiofx/dsp/repair.rs`,
  `modulation.rs`, `character.rs`, the gate and limiter in `dynamics.rs`):
  adapted from `crates/audio-dsp/src/effects/` (`restoration.rs` DeHum,
  `essential.rs` DeEsser and StereoWidth, `dynamics.rs` Gate and Limiter,
  `modulation.rs` ChorusFlanger and Phaser, `special.rs` Distortion,
  `util.rs` delay line and all-pass), rewritten for offline whole-buffer
  processing.
- **Timeline interchange** (`crates/engine/src/modules/interchange/`): the
  structure of FilmCraft's `crates/interchange` (a format-neutral pass, one
  writer per format, a report of what was left out), and the element sets
  of its OpenTimelineIO, CMX 3600 EDL and FCPXML writers. The writers
  themselves are chukcut's own code over chukcut's document.
- **Marker export** (`crates/engine/src/modules/interchange/markers.rs`):
  the CSV writer (quoting and the formula guard) after
  `crates/engine/src/marker_export.rs`.
- **Sync by sound** (`crates/engine/src/modules/analysis/sync.rs`): the
  GCC-PHAT-β cross-correlation, the valid-lag window and the confidence and
  distinctness figures, adapted from `crates/audio-dsp/src/sync.rs`.
- **Relinking moved media** (`crates/engine/src/modules/media/relink.rs`):
  the folder index, the prefix remap (`derive_remap`, `apply_remap`) and
  the check of a candidate before it replaces the original, adapted from
  `crates/engine/src/relink.rs`.

## Codec patents

**This is separate from the software licence, and no software licence disposes
of it.**

H.264/AVC and H.265/HEVC are covered by patents administered by pools — Via LA
for AVC, Access Advance for HEVC — which license *implementations*, regardless
of the licence the source code carries.

This project distributes **source code** and sells nothing. It pays no
royalties and is in the same position as VLC, Kdenlive, HandBrake and FFmpeg
itself.

If you distribute binaries built from this source, or use them commercially in
a jurisdiction that recognises those patents, **you are responsible for your own
patent compliance**. Nothing here is legal advice.

Where hardware encoders are used (VAAPI, QSV, NVENC, VideoToolbox), the encoder
is part of the GPU and its driver rather than of this software.

## Trademarks and assets

The name *chukcut* and its logo are not licensed under the GPL and remain the
project's own.

No ByteDance-authored assets — effects, fonts, templates or icons — are
contained in this repository or in any build produced from it. The effect
runtime loads packages from a location the user supplies at runtime, and CapCut
project files are neither read nor written.
