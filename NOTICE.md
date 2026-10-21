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

## Build and release tooling taken from other projects

| Project | Licence | What we took |
|---|---|---|
| [FilmCraft](https://github.com/storytold/filmcraft) and its shared workflows [craft-common-actions](https://github.com/storytold/craft-common-actions), by the ArtCraft Team and the FilmCraft contributors | FilmCraft: MIT OR Apache-2.0, used under Apache-2.0 (text in [`.github/LICENSE-APACHE-2.0.txt`](.github/LICENSE-APACHE-2.0.txt)). craft-common-actions carries no licence file; its workflow was rewritten, not copied | The release pipeline, deliberately and with thanks: `.github/workflows/release.yml`, `.github/actions/resolve-version`, `.github/actions/draft-release`, the AppImage update information in `packaging/appimage/build-appimage.sh`. Each file says what was changed. [Decision 0037](docs/decisions/0037-release-workflow-from-filmcraft.md) lists what was taken and what was left out on purpose (their craft-fonts embedding) |

FilmCraft's NOTICE: "FilmCraft. Copyright (c) 2026 ArtCraft Team and the
FilmCraft contributors. Licensed under the MIT License or the Apache License,
Version 2.0, at your option." The ArtCraft and FilmCraft names and logos are
their trademarks; none of their brand assets are in this repository.

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
