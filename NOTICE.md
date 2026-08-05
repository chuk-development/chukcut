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
| Tauri, WebKitGTK | MIT / Apache-2.0, LGPL | Application shell |
| React, Zustand, Radix UI, Tailwind CSS, Lucide | MIT | User interface |

`cargo tree` and `pnpm licenses list` enumerate the complete set.

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
