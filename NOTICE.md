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
licensed under the Apache License, Version 2.0 or the MIT License, at the
user's option (<https://github.com/storytold/filmcraft>). chukcut takes it
under the Apache License 2.0, which is compatible with the GPL-3.0; the
FilmCraft names and logos are not used. Files copied or closely adapted
from FilmCraft keep its copyright line at their top. Our changes to them are
under the GPL-3.0.

What was taken:

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
