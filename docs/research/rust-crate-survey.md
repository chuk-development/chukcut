# Rust crate survey

Where the ecosystem would save us work, where it would cost us, and where the
honest answer is that nobody has written this yet.

Surveyed 2026-07-25. Every version number, release date and download figure
below came from the crates.io API or the GitHub API on that day, not from
memory. Where a claim rests on a README rather than on source I have said so,
and where I could not verify something at all there is a list at the end.

The question asked of each subsystem was narrow: *is there a crate that would
save significant work or do a materially better job than what we have or plan?*
Not "what is popular". A crate that replaces two hundred lines we would have
written anyway is not worth a dependency; a crate that replaces two thousand
lines we would have written badly is.

## Summary

| # | Subsystem | In use / planned | Recommended | Verdict | Why in one line |
|---|---|---|---|---|---|
| 1 | Demux, decode, encode | `ffmpeg-next` 6.1 | `ffmpeg-next` (bump to 8.1 when convenient) | **stay** | Nothing else reads the file zoo; the pure-Rust stack is a strictly smaller subset. |
| 2 | Hardware encode (VAAPI) | recorded as blocked | `ffmpeg_next::ffi` hwcontext | **adopt (ours, not blocked)** | The bindings we thought were missing are already in our lockfile; it is ~200 lines of our own `unsafe`. |
| 2b | Zero-copy decode → wgpu | — | raw `ash` + `texture_from_raw`; evaluate `gpu-video` first | **revisit later** | wgpu 30's DMA-BUF importer is single-plane only and NV12 is not, so ~120 lines are still ours. |
| 3 | Audio output and clock | `cpal` 0.18 + own mixer + swresample | same | **stay** | cpal's `playback` timestamp is the only clock that matters; every higher-level crate would take our mixer away. |
| 3b | Resampling | FFmpeg swresample | swresample now, `rubato` 4.0 if speed ramps need it | **revisit later** | swresample is free (already linked); rubato wins only for a ratio that changes per block. |
| 3c | Loudness | — | `ebur128` 0.1.10 | **adopt (small)** | R128 with true peak, correct, tiny; do not hand-roll the K-weighting filters. |
| 4 | Text rendering | — | `parley` 0.11 + `fontique` + `harfrust` + `skrifa` + `kurbo::stroke` | **adopt** | Layout/shaping/outlines are solved; every CapCut-grade effect on top of them is not, and never will be. |
| 5 | Colour management | hand-rolled | `moxcms` 0.9 | **adopt** | It already ships in our binary via `image`, and it has the H.273 CICP tables we would otherwise transcribe by hand. |
| 6 | GPU effect graphs | own multi-pass graph | own multi-pass graph | **stay** | No render-graph crate fits a video pipeline; the closest one, `tweak_shader`, is worth reading and not depending on. |
| 6b | GLSL ES 1.0 ingestion | naga glsl-in (assumed) | our rewriter → `glslang` → `spirv-webgpu-transform` → naga spv-in | **switch (plan, not code)** | naga's GLSL frontend rejects every shader in the corpus, and `sampler2D` breaks both naga frontends regardless. |
| 7 | Waveforms, thumbnails, proxies | ours | ours, plus `fast_image_resize` | **stay** | Nothing exists to reuse; the only borrowable piece is a SIMD scaler. |
| 8 | Timeline data structures | ours | ours | **stay** | No Rust crate models an NLE timeline. The two OTIO crates on crates.io are 2020 placeholders with one commit. |
| 8b | OTIO interchange | — | plain `serde` against the OTIO JSON schema | **revisit later** | Cheap to add, but nothing a consumer editor cares about survives the round trip. |

Two of these are worth reading even if you skip the rest: **§2**, because the
thing we recorded as blocking the VAAPI encode path is not actually blocking,
and **§6b**, because the shader-translation plan in the roadmap does not work
as written and fails for a second reason beyond the obvious one.

---

## 1. Demux, decode, encode

### What is actually on offer

| Crate | Version | Released | Downloads (90d) | Note |
|---|---|---|---|---|
| [`ffmpeg-next`](https://github.com/zmwangx/rust-ffmpeg) | 8.1.0 | 2026-03-18 | 3.2 M | What we use, at 6.1. |
| [`ffmpeg-sys-next`](https://github.com/zmwangx/rust-ffmpeg-sys) | 8.1.0 | 2026-03-18 | 3.3 M | The bindgen layer under it. |
| [`ffmpeg-the-third`](https://github.com/shssoichiro/ffmpeg-the-third) | 5.0.0+ffmpeg-8.1 | 2026-04-03 | 17 k | Fork begun when `ffmpeg-next` looked abandoned. It isn't. |
| [`rsmpeg`](https://github.com/larksuite/rsmpeg) | 0.18.0+ffmpeg.8.0 | 2025-08-24 | 34 k | Thinner, closer to the C API. **Last commit 2025-08-24.** |
| [`video-rs`](https://github.com/oddity-ai/video-rs) | 0.11.0 | 2026-02-24 | 110 k | High-level "read frames as ndarray" wrapper over `ffmpeg-next`. |
| [`ez-ffmpeg`](https://github.com/YeautyYE/ez-ffmpeg) | 0.15.0 | 2026-07-23 | 33 k | Filter-graph-shaped API over `rusty_ffmpeg`. |
| [`ffmpeg-sidecar`](https://github.com/nathanbabcock/ffmpeg-sidecar) | 2.5.2 | 2026-05-30 | 439 k | Drives the `ffmpeg` binary over a pipe. |

`ffmpeg-next` is the right one and remains so. Its README describes the crate
as "in maintenance mode", which reads worse than it is: the repository was
pushed to on 2026-07-21, with a dozen commits that week fixing audio pointers,
non-exhaustive enums and codec parameter setters. Maintenance mode here means
"no redesigns", which for an FFmpeg binding is the desirable state.

The others do not earn a switch. `ffmpeg-the-third` exists because the upstream
crate looked dead in 2022; it now has one two-hundredth of upstream's downloads
and no capability we lack. `rsmpeg` is a genuinely nicer API — it exposes more
of the C surface with less opinion — but it has not been committed to in eleven
months, which for a crate whose entire job is tracking a fast-moving C library
is disqualifying. `video-rs` and `ez-ffmpeg` add abstraction above
`ffmpeg-next`, and abstraction above the decoder is precisely the layer we are
writing ourselves. `ffmpeg-sidecar` is the pragmatic choice for a tool that
shells out; we are not that tool.

### The version pin is looser than we wrote down

`CLAUDE.md` says "ffmpeg-next must match the system FFmpeg." That is not how
the crate works, and the note will mislead the next person.

`ffmpeg-sys-next/build.rs` compiles a probe against the *system* headers and
emits cfg flags from the result. The table it uses, verbatim from master:

```rust
("ffmpeg_6_0", 60, 3),
("ffmpeg_6_1", 60, 31),
("ffmpeg_7_0", 61, 3),
("ffmpeg_7_1", 61, 19),
("ffmpeg_8_0", 62, 8),
("ffmpeg_8_1", 62, 28),
```

Every API added after a given FFmpeg release sits behind the matching cfg. The
crate's own README states the intent plainly: "This crate is currently in
maintenance mode, and aims to be compatible with all of FFmpeg's versions from
3.4 (currently from 3.4 til 8.0)." The crate's major.minor tracks the *newest*
FFmpeg it knows about, not the only one it accepts.

So `ffmpeg-next = "8.1"` against system FFmpeg 6.1 should build, and would get
us eighteen months of upstream fixes. **Should**, because I have not run the
build — that is a ten-minute experiment, not a research result, and it is worth
doing before rewriting the note in `CLAUDE.md`. The failure mode if it goes
wrong is a link error at build time, which is loud and harmless.

### Pure-Rust decoders, and the hybrid question

The honest summary is that the pure-Rust stack is a strict subset of what
FFmpeg gives us, and the subset is smaller than it looks.

**[`symphonia`](https://github.com/pdeljanov/Symphonia) 0.6.0 (2026-05-15)** is
excellent and actively developed — 3.0 M downloads in 90 days, pushed
2026-07-23. Its own support table rates WAV/FLAC/MP3/Vorbis/PCM as "excellent",
ISO-MP4 and AAC-LC as "great", MKV/WebM as "good", and **Opus as "in work"**.
There is no AC-3, E-AC-3, DTS or HE-AAC. For a *music player* that is complete.
For a video editor it means the first AVCHD clip from a camcorder (AC-3), the
first HE-AAC web download and the first Opus-in-WebM file all fail, and we would
be back to FFmpeg for them anyway. Carrying two audio decode paths to avoid
using one we already link is a net loss.

**AV1**: [`rav1d`](https://github.com/memorysafety/rav1d) 1.1.0 reached
functional parity with dav1d in 2024 and is, per Prossimo's own bounty
announcement, still about 5 % slower; the $20,000 parity bounty was still open
as of early 2026. It also exposes only the C API — "A Rust API is planned for
addition in the future". [`dav1d`](https://github.com/rust-av/dav1d-rs) 0.11.1
is a normal binding to the C library. Either way, FFmpeg already links libdav1d
and picks it automatically. There is nothing to gain.

**Demuxers**: [`re_mp4`](https://github.com/rerun-io/re_mp4) 0.5.1 (2026-07-08,
Rerun's fork of `mp4`) and [`mp4parse`](https://github.com/mozilla/mp4parse-rust)
0.17.0 (crate last published 2023-05-29, though the repo is alive) both parse
ISO-BMFF and hand you sample data without decoding.
[`matroska-demuxer`](https://github.com/hasenbanck/matroska-demuxer) 0.8.0 does
the same for MKV.

Would a hybrid — pure-Rust demux, FFmpeg decode — help? No, and it is worth
being explicit about why, because the idea is superficially attractive.
Demuxing is not the part that is hard to get right; it is the part that is hard
to get *complete*. What an editor actually meets is fragmented MP4 from phones,
QuickTime with edit lists, MXF from broadcast cameras, MPEG-TS from capture
cards, AVCHD's `.mts`, and a long tail of files that are technically malformed
and that every player has quietly learned to accept. libavformat's value is
twenty years of that tail. Swapping it for a clean-room MP4 parser trades a
solved problem for an unsolved one and buys a marginally nicer API.

The one place a Rust demuxer earns its place is a wasm build, where FFmpeg is
expensive to ship. That is not on the roadmap.

**Verdict: stay.** Bump to `ffmpeg-next` 8.1 opportunistically after verifying
it links against system 6.1, and correct the pin note in `CLAUDE.md`.

---

## 2. Hardware decode and encode

This section changes a decision, so it is the longest.

### The premise was wrong: the bindings are already here

The brief records that `ffmpeg-next` 6.1 "does not wrap `AVHWFramesContext`,
which currently blocks our VAAPI encode path". The first half is true and the
second half does not follow.

I unpacked the exact crates in our lockfile. `ffmpeg-next` 6.1.1's safe layer
contains no reference to `hw_frames_ctx`, `hw_device_ctx` or `AVHWFramesContext`
— confirmed by grep over the published `.crate`. But one line up the stack:

```rust
pub extern crate ffmpeg_sys_next as sys;
pub use sys as ffi;
```

and in `ffmpeg-sys-next` 6.1.0's `build.rs`, in the list of headers handed to
bindgen:

```rust
.header(search_include(&include_paths, "libavutil/hwcontext.h"))
...
if let Some(hwcontext_drm_header) =
    maybe_search_include(&include_paths, "libavutil/hwcontext_drm.h")
```

So `AVHWDeviceContext`, `AVHWFramesContext`, `av_hwdevice_ctx_create`,
`av_hwframe_ctx_alloc`, `av_hwframe_ctx_init`, `av_hwframe_get_buffer`,
`av_hwframe_transfer_data`, `av_hwframe_map`, `AV_PIX_FMT_VAAPI`,
`AV_PIX_FMT_DRM_PRIME` and `AVDRMFrameDescriptor` are all reachable **today**,
from the version already pinned, as `ffmpeg_next::ffi::*`. And the escape
hatches to attach them exist too — `Context::as_mut_ptr() -> *mut AVCodecContext`
and `Frame::as_mut_ptr() -> *mut AVFrame` are `pub unsafe fn` in 6.1.1.

What is *not* bound is `libavutil/hwcontext_vaapi.h`, so there is no
`AVVAAPIDeviceContext` (the `VADisplay`) and no `AVVAAPIFramesContext` (the
`VASurfaceID` array). We do not need them: going out through `av_hwframe_map` to
`AV_PIX_FMT_DRM_PRIME` is both the portable route and the one that produces the
DMA-BUF descriptor the zero-copy section below wants, and `hwcontext_drm.h`
*is* bound.

What is missing is a *safe* wrapper, which is perhaps two hundred lines of our
own `unsafe` translating `doc/examples/vaapi_encode.c` — allocate a device
context for `/dev/dri/renderD128`, allocate a frames context with format
`AV_PIX_FMT_VAAPI` and sw_format `AV_PIX_FMT_NV12`, init it, set
`codec_ctx->hw_frames_ctx` before `avcodec_open2`, then `av_hwframe_get_buffer`
per frame and upload. That is a day's work, not a blocked path.

There is a shortcut worth trying first: FFmpeg's own CLI does VAAPI encode via
`-vf 'format=nv12,hwupload'`, and `ffmpeg-next` does expose the `filter` module.
The `hwupload` filter builds the frames context internally; you then have to
propagate it to the encoder with `av_buffersink_get_hw_frames_ctx`, which is a
raw call but a single one. I have **not** verified that this composes cleanly
through `ffmpeg-next`'s filter wrapper — flagging it as the first thing to try,
not as a known-good recipe.

Meanwhile the unreleased `ffmpeg-sys-next` master gained a commit on 2026-07-01
titled "Expose `hwcontext` structs", adding a `hwcontext_wrapper.h` that binds
the per-API structs (`AVVAAPIDeviceContext`, `AVVAAPIFramesContext`,
`AVQSVFramesContext`, `AVVulkanFramesContext`, `AVCUDADeviceContext`, the D3D
ones) behind `__has_include` guards with stub SDK headers. That is not in 8.1.0
and there is no release date for it. It would remove the last bit of hand-rolled
struct definition; it is not a prerequisite.

### rsmpeg has the safe wrapper, and cannot use it for what we want

Worth stating because it is the one place another crate genuinely beats
`ffmpeg-next`, and then does not.

`rsmpeg` 0.18.0 has `src/avutil/hwcontext.rs` — 287 lines of real safe wrapper:
`AVHWDeviceContext::{alloc, init, create, create_derived, hwframe_ctx_alloc}`,
`AVHWFramesContext::{init, data, get_buffer}`, plus
`AVCodecContext::{set_hw_device_ctx, set_hw_frames_ctx, hw_config, is_hwaccel}`
and `AVFrame::hwframe_transfer_data`. That is precisely the API surface
`doc/examples/vaapi_encode.c` needs, with no `unsafe` on our side.

But its sys layer, `rusty_ffmpeg` 0.17.0, binds `libavutil/hwcontext.h` and has
**every `hwcontext_*.h` commented out in `build.rs`** — including
`hwcontext_drm.h`. So rsmpeg gives you a clean VAAPI *encode* setup and no
`AVDRMFrameDescriptor`, which means no DMA-BUF export and no zero-copy path
without running bindgen ourselves. Combined with its eleven months of silence
(§1), the trade is: save a day of `unsafe` on encode, lose the decode path we
actually want. Not worth switching for.

### The per-backend crate landscape is thin, and that is fine

| Backend | Crate | Version | Released | 90d | Assessment |
|---|---|---|---|---|---|
| VAAPI | [`cros-libva`](https://github.com/chromeos/cros-libva) | 0.0.13 | 2024-12-06 | 376 k | The only serious one. Has `export_prime()` (i.e. `vaExportSurfaceHandle`) and, contrary to common belief, **encode as well as decode**. But 0.0.x, 34 stars, ~19 months of merged work not yet published, docs.rs build failing, and the download count is AOSP vendoring rather than adoption. |
| VAAPI | `cros-codecs` | 0.0.6 | 2025-06-18 | 766 k | Read-only mirror; `main` frozen since 2025-03-10. VAAPI encode for H.264/VP9/AV1, no HEVC. |
| VAAPI | `libva-sys`, `fev`, `rusty_vainfo` | — | 2021–2024 | ~40 | Dead. `libva`, `vaapi` and `vaapi-sys` do not exist on crates.io at all. |
| NVENC | [`nvidia-video-codec-sdk`](https://github.com/ViliamVadocz/nvidia-video-codec-sdk) | 0.4.0 | 2025-09-15 | 5 k | Encode complete; **decode has no safe wrapper** ("There is no safe wrapper yet", verbatim in `src/lib.rs`). One maintainer, 31 stars. Links at build time, no `dlopen`. |
| CUDA | [`cudarc`](https://github.com/chelsea0x3b/cudarc) | 0.19.8 | 2026-06-19 | 2.7 M | Healthy, and has `import_external_memory` for OPAQUE_FD — but no external *semaphores*, so no fine-grained CUDA↔Vulkan sync, and no DMA_BUF handle type. |
| QSV | `shiguredo_vpl` | 2026.3.0 | 2026-06-23 | 7 k | The only oneVPL binding in existence (`onevpl`, `libvpl`, `mfx` are all 404). Two GitHub stars, five months old, and its contribution policy is "discuss on Discord, Japanese only". |
| VideoToolbox | `objc2-video-toolbox` | 0.3.2 | 2025-10-04 | 21 k | Generated bindings, part of the `objc2` family. The safe default if macOS ever matters. |

The shape is consistent: everybody who does hardware video in Rust does it
through FFmpeg's hwcontext abstraction, because FFmpeg is where the per-vendor
quirks are already absorbed. Adopting `cros-libva` would mean owning the VAAPI
quirks ourselves — including bitstream header generation on the encode side — in
exchange for skipping a layer we already link. Don't.

### Zero-copy decode into a wgpu texture: half solved, and the half that is missing is ours

This is the part that changed under us, and it changed in both directions.

**What landed.** wgpu 30 gained a DMA-BUF importer:

```rust
#[cfg(unix)]
pub unsafe fn texture_from_dmabuf_fd(
    &self,
    fd: OwnedFd,
    desc: &TextureDescriptor<'_>,
    drm_modifier: u64,
    stride: u64,
    offset: u64,
) -> Result<Texture, DeviceError>
```

in `wgpu-hal/src/vulkan/device.rs`, behind the new
`Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF`. It arrived in
[PR #9366](https://github.com/gfx-rs/wgpu/pull/9366), opened 2026-04-03 and
merged 2026-04-09. (The wgpu CHANGELOG credits this to #9412, which is actually
an unrelated `SHADER_I16` PR — verified against the GitHub API. Cite #9366.)

The important consequence: wgpu now advertises the feature only when
`VK_KHR_external_memory_fd`, `VK_EXT_external_memory_dma_buf` and
`VK_EXT_image_drm_format_modifier` are all present, **and pushes them into
`required_device_extensions` automatically**. A plain `request_device()` with
the feature enabled is enough. Until this year the accepted wisdom was that wgpu
could not request `VK_EXT_image_drm_format_modifier` at all and you had to bring
your own `VkDevice`. That blocker is gone.

**What is missing, and it is exactly our case.** The doc says "Currently only
supports single-plane DMA-bufs", and internally it builds
`ImageDrmFormatModifierExplicitCreateInfoEXT` with exactly one
`VkSubresourceLayout`. VAAPI exports NV12 as **one DMA-BUF object with per-plane
offsets and pitches under a single modifier** — which is also what V4L2 and GBM
produce. That cannot be expressed through this signature.

This is tracked as [issue #9801](https://github.com/gfx-rs/wgpu/issues/9801)
(opened 2026-07-03, no maintainer response as of this survey), and its author
has already thought through the alternatives we would otherwise rediscover:

> Integrators doing video interop currently have to hand-roll the import with
> raw `ash` … ~120 lines of unsafe duplicating what `texture_from_dmabuf_fd`
> already does for the single-plane case. The non-raw detours are strictly
> worse: pre-converting on the media engine costs a GPU pass and ~2.7x
> bandwidth per frame, and importing planes as separate R8/RG8 textures runs
> into WebGPU copy-compatibility rules later.

That last clause matters, and it corrects an assumption worth writing down
because it is the obvious idea: importing Y and UV as two independent textures
looks like it dodges the whole problem, and it does not, because they are two
views onto one memory object and wgpu's copy rules will object downstream.

So the honest chain on an Intel iGPU is:

```
 VAAPI decode ──► AVFrame (AV_PIX_FMT_VAAPI)
        │
        │  av_hwframe_map() → AV_PIX_FMT_DRM_PRIME     [ffmpeg-sys-next 6.1 ✔]
        ▼
 AVDRMFrameDescriptor { objects[].fd, .format_modifier,
                        layers[].planes[].offset, .pitch }
        │
        │  ~120 lines of raw `ash`:
        │    vkCreateImage ×2 with explicit plane layouts + external memory info
        │    vkGetMemoryFdPropertiesKHR, VkImportMemoryFdInfoKHR, dedicated alloc
        │    bind both images to the one imported VkDeviceMemory
        ▼
 wgpu_hal::vulkan::Device::texture_from_raw(.., TextureMemory::External)
        │
        │  device.create_texture_from_hal::<Vulkan>(tex, &desc, initial_state)
        ▼
 wgpu::Texture ×2 (R8Unorm, Rg8Unorm) ──► our YCbCr→RGB shader pass
```

`TextureMemory` is new in wgpu 30 (`Allocation | Dedicated(vk::DeviceMemory) |
External`) and is what makes this legal without wgpu trying to free memory it
does not own.

**Three traps that will each cost a day if not known in advance:**

1. **`create_texture_from_hal` gained a fourth parameter in wgpu 30** —
   `initial_state: TextureUses`, from
   [PR #9496](https://github.com/gfx-rs/wgpu/pull/9496). The changelog is
   explicit about why: previously the tracker hard-coded
   `TextureUses::UNINITIALIZED`, and *"this affected zero-copy hardware-decoded
   video imports on the platforms where compressed modifiers are used."* Pass
   `UNINITIALIZED` and Vulkan is entitled to discard the frame you just
   imported. This is a silent-black-frames bug, not a crash.
2. **Intel's tiled modifiers are the usual failure.** The
   [iroh-live](https://github.com/n0-computer/iroh-live) project documents the
   workaround in `rusty-codecs/src/render/dmabuf_import.rs`: when the VAAPI
   decoder produces surfaces with a modifier Vulkan cannot import — *Y_TILED on
   Intel* is the example they name — they run a **VAAPI VPP blit** to re-tile
   the surface to an importable modifier first. On our target hardware this is
   not an edge case, it is the default path, and it should be designed in from
   the start rather than discovered.
3. **No foreign-queue barrier.** wgpu's import hardcodes `SharingMode::EXCLUSIVE`
   and `initial_layout(UNDEFINED)` and emits no
   `VK_QUEUE_FAMILY_FOREIGN_EXT` acquire/release.
   [Issue #2948](https://github.com/gfx-rs/wgpu/issues/2948) has been open since
   2022-08-07 with zero comments. Do it manually via
   `CommandEncoder::transition_resources()`.

**And three things wgpu 30 cannot do, so nobody plans around them:**

- **`VK_KHR_sampler_ycbcr_conversion` is unusable.** wgpu checks for the
  extension's *presence* as a proxy for NV12 format support, but the actual
  enable is commented out in `vulkan/adapter.rs` and it never creates a
  `VkSamplerYcbcrConversion` or sets `pImmutableSamplers`. Architectural, not a
  small patch. Which is fine — we want to do the conversion in our own shader
  anyway, for colour-management reasons (§5).
- **`TextureFormat::NV12` exists and now works on Vulkan** (not DX12-only, as
  older notes say), but it is **sample-only** — no render target, no storage.
- **`Features::EXTERNAL_TEXTURE`**, the WebGPU `GPUExternalTexture` that would
  handle multiplanar YCbCr transparently, is DX12 and Metal only. Vulkan has not
  been started.

There is **no maintainer commitment** to a safe `wgpu::Device::import_dmabuf`.
[Issue #7819](https://github.com/gfx-rs/wgpu/issues/7819), "Plan for external
memory, external semaphores" (2025-06-17), still has zero comments. This stays
at the `wgpu-hal` / `as_hal` level with `unsafe` for the foreseeable future.

One genuinely useful thing that did land: `vulkan::Queue::add_wait_semaphore`
(PR [#9461](https://github.com/gfx-rs/wgpu/pull/9461)), which lets us wait on
the decoder's fence at submit time instead of blocking the CPU.

### The alternative worth an afternoon: Vulkan Video

[`gpu-video`](https://github.com/software-mansion/smelter) 0.4.0 (2026-05-12,
MIT, from the Smelter team — who build a wgpu video compositor, i.e. our exact
problem) skips VAAPI entirely and uses **Vulkan Video**. It adds
`VideoAdapterExt` / `VideoDeviceExt` extension traits directly onto
`wgpu::Adapter` and `wgpu::Device`, decoding straight into a `wgpu::Texture`,
and its README states that decoded frames never leave GPU memory. In-tree
examples include `decode_wgpu.rs`, `encode_wgpu.rs` and, most usefully,
`print_hw_capabilities.rs`.

Support per its README: H.264 decode and encode, H.265 encode only, AV1 in
progress. It is pinned to **wgpu 29** and has 3.9 k downloads — young.

If Vulkan Video works on this iGPU, it removes DRM modifiers, Y_TILED re-tiling
and multi-planar import from the problem entirely. If it does not, we have lost
an afternoon. Running `print_hw_capabilities` is the cheapest experiment in this
whole document and it should happen before anyone writes the `ash` import code.
The catch that would rule it out for us: no H.265 *decode*, and HEVC is
increasingly what phones record.

### Prior art, ranked by how close it is to us

1. **[`ez-ffmpeg`](https://github.com/YeautyYE/ez-ffmpeg)** —
   `src/wgpu_filter/hw_interop.rs`, 539 lines, whose doc header describes
   verbatim the design above: *"The decoder's NV12 surface is exported as a
   dmabuf (`av_hwframe_map` to `AV_PIX_FMT_DRM_PRIME`), its two planes are
   imported as one `R8Unorm` and one `Rg8Unorm` `VkImage` over the same memory
   object."* It is FFmpeg-based like us, has `AVDRMFrameDescriptor` parsing,
   `PhysicalDeviceImageDrmFormatModifierInfoEXT` capability checks,
   `ImportMemoryFdInfoKHR` and `texture_from_raw`, plus thirteen WGSL effects
   alongside. **This is the closest existing thing to chukcut's render path.**
   On wgpu 26, released weekly, bus factor one — read it, do not depend on it.
2. **[`Lightningbeam`](https://github.com/skykooler/Lightningbeam)** — an actual
   Rust video editor. `lightningbeam-ui/gpu-video-encoder/src/dmabuf.rs` handles
   NV12 *and* P010 10-bit, with `vaapi.rs`, `vk_device.rs`, `nv12.rs`,
   `encoder.rs`, `decoder.rs` — so the encode return path too.
3. **[`iroh-live`](https://github.com/n0-computer/iroh-live)** — best-documented
   on the failure modes, including the Intel Y_TILED VPP re-tile above.
4. **[`grafting`](https://github.com/mark-ik/wgpu-graft)** 0.3.0 — the most
   compact reference: ~300 lines plus a `dmabuf_roundtrip` test and semaphore
   interop. 66 downloads; a reading source, not a dependency.
5. **[Gyroflow](https://github.com/gyroflow/gyroflow)** —
   `src/core/gpu/wgpu_interop_{vulkan,cuda,directx,metal}.rs` and
   `src/rendering/ffmpeg_video.rs`. Broadest platform coverage. GPL-3.0, so read
   for shape only. Its author (@AdrianEddy) upstreamed most of wgpu 30's
   cross-API interop surface — #9461, #9463, #9483, #9551 — which is why the
   ecosystem support for our problem appeared this year.
6. **[`jay`](https://github.com/mahkoh/jay)** and
   **[`gst-plugins-rs`](https://github.com/GStreamer/gst-plugins-rs)**
   (`video/gtk4/src/sink/frame.rs`) — not wgpu, but the correct references for
   multi-plane descriptor handling.

Dead ends, so nobody re-checks them: Servo has no dmabuf path at all (its media
stack goes through `glupload` in C), `surfman` has no dmabuf import, libmpv's
render API knows only OpenGL and software, `gst-plugin-wgpu` does not exist, and
`libplacebo-sys` has been dead since 2019.

### If zero-copy proves painful

The fallback is `av_hwframe_transfer_data` into system memory and a normal
`queue.write_texture`. At 1920×1080 NV12 that is 3.11 MB per frame, so 187 MB/s
each way at 60 fps.

The naive estimate understates it, though. `av_hwframe_transfer_data` reads out
of VAAPI surfaces that are typically mapped uncached or write-combining, so the
throughput is nothing like a normal `memcpy` — expect single-digit GB/s rather
than twenty. Worse, the download **serialises the decode pipeline**: the decoder
cannot reuse the surface until the transfer completes, so we lose decoder
parallelism as well as bandwidth. These numbers are reasoned, not measured, and
should be measured before anyone budgets around them.

The practical consequence: at 1080p30 preview this is invisible next to the JPEG
encode we already pay; at 4K60 export it is likely the bottleneck. Which is the
correct shape anyway — build the transfer path first because it always works,
and treat zero-copy as an export optimisation.

`video-rs` 0.11.0 is the copy-paste reference here: `src/ffi_hwaccel.rs` is a
compact, correct implementation of the raw hwaccel setup over `ffmpeg-next`,
including the `get_format` callback, and `src/hwaccel.rs` has the device-type
enumeration. It has **no hardware encode at all** and always transfers to system
memory, so it is a reference rather than a dependency — but it is ~120 lines we
do not have to think through from scratch.

**Verdict: adopt, and unblock.** No new dependency is required for the encode
path, which is not blocked. Zero-copy decode is now possible but needs ~120 lines
of `ash` plus Intel re-tiling; check `gpu-video` first, since it might delete
that work entirely.

---

## 3. Audio

The engine already has `src-tauri/src/modules/audio/` with `device.rs`,
`mixer.rs`, `clock.rs`, `ring.rs` and `decode.rs`, on `cpal` 0.18 with FFmpeg's
swresample. Having read it, the survey's job is to check that choice rather
than propose one.

### cpal is right, and the alternatives would each take something away

| Crate | Version | Released | 90d | What it is |
|---|---|---|---|---|
| [`cpal`](https://github.com/RustAudio/cpal) | 0.18.1 | 2026-06-07 | 4.0 M | Device I/O. Nothing else. |
| [`rodio`](https://github.com/RustAudio/rodio) | 0.22.2 | 2026-03-05 | 1.9 M | Playback library over cpal + symphonia. |
| [`kira`](https://github.com/tesselode/kira) | 0.12.2 | 2026-07-16 | 125 k | Game audio engine over cpal. |
| [`cubeb`](https://github.com/mozilla/cubeb-rs) | 0.36.0 | 2026-07-15 | 151 k | Firefox's device layer. |
| [`tinyaudio`](https://github.com/mrDIMAS/tinyaudio) | 2.0.0 | 2025-11-15 | 13 k | Minimal output. |

All three of cpal, rodio and kira are actively maintained (cpal pushed
2026-07-22, rodio 2026-07-24, kira 2026-07-16). The question is not health, it
is layering.

An NLE's audio path is a *mixer with a document behind it*: per-clip gain
envelopes, fades, speed with pitch handling, sample-exact clip boundaries at
microsecond timeline positions, and — critically — the same mixer must run
offline during export with no device attached and produce bit-identical output.
`rodio` and `kira` both bring their own mixer and their own source graph. Using
either means either fighting their model or using them only as a device wrapper,
which is what cpal already is. `kira` does have a `mock` backend that drives its
`Renderer` without a device, which is the right shape for offline export — but
it is the right shape for *kira's* mixer, not ours.

`cubeb` deserves a mention because it is Firefox's and therefore extremely well
tested across weird devices, but it is a C library with a smaller Rust
community and no advantage for our case.

### The clock: cpal gives us the one number we need

`OutputCallbackInfo::timestamp()` returns an `OutputStreamTimestamp` with two
fields, documented as:

- `callback` — "The instant the stream's data callback was invoked."
- `playback` — "The predicted instant that data written will be delivered to
  the device for playback. E.g. The instant data will be played by a DAC."

`playback - callback` is the output latency, and the existing `device.rs`
already computes exactly that. That is the correct and only sound basis for
"where is the playhead": count frames handed to the device, subtract the
latency, and let video chase it. Nothing in rodio or kira exposes anything
better, because there is nothing better — it is cpal's number either way.

Two cpal features worth knowing about and not currently enabled, both verified
in `Cargo.toml` for 0.18.1:

- **`realtime`** (and `realtime-dbus`) — applies platform real-time scheduling
  to the audio thread via `audio_thread_priority`, going through rtkit over
  D-Bus on Linux desktops. On a machine simultaneously decoding video and
  driving a GPU, this is the difference between occasional xruns and none. It
  needs `rtprio` in `limits.conf` or a working rtkit, so it must degrade
  gracefully.
- **`pipewire`** — a native PipeWire backend rather than going through
  PipeWire's ALSA compatibility shim. Worth measuring; not worth assuming.

### Resampling: swresample now, rubato if speed ramps demand it

[`rubato`](https://github.com/HEnquist/rubato) 4.0.0 (2026-07-09, 3.0 M
downloads/90d, 4 open issues, pushed 2026-07-18) is a healthy, well-documented
crate. It offers a synchronous FFT resampler, and asynchronous sinc and
polynomial resamplers whose **ratio can be changed at any time**.

But `decode.rs` already uses `ffmpeg::software::resampling`, and its own comment
gives the right reason: swresample converts sample format, channel layout *and*
rate in one pass, which is three problems rather than one. rubato does rate
only; adopting it means writing our own format conversion and channel
rematrixing to go with it. That is a downgrade in exchange for removing a
dependency we cannot remove anyway.

The one case that flips it is a **speed ramp** — a clip whose rate varies
continuously across its duration. swresample can be nudged with
`swr_set_compensation`, but rubato's async resamplers are designed for a ratio
that changes per block and will sound better doing it. Phase 2 has "Speed:
constant and curved, with pitch preservation" on it. When that work starts,
compare the two; until then adding rubato is a dependency with no consumer.

Related, for the same Phase 2 item: pitch-preserving time stretch has no
obvious pure-Rust answer.
[`signalsmith-stretch`](https://github.com/colinmarc/signalsmith-stretch-rs)
0.1.3 (2025-09-18, 27 k/90d) binds an excellent and permissively licensed C++
library; [`soundtouch`](https://github.com/Cyanistic/soundtouch) 0.5.4 binds
the classic one (LGPL, which matters for how we link it). FFmpeg's own
`atempo`/`rubberband` filters are reachable through `ffmpeg-next`'s `filter`
module at zero new dependency cost, and that is where I would start.

### Loudness and limiting

[`ebur128`](https://github.com/sdroege/ebur128) 0.1.10 is the only credible
loudness crate: momentary, short-term and integrated LUFS, loudness range, and
true-peak scanning, by Sebastian Dröge (a GStreamer maintainer). Flag honestly:
**last release 2024-10-26, last commit 2025-02-24, 7 open issues.** That looks
stale and mostly is not — it is a port of libebur128 implementing a frozen
standard, so "no commits" and "finished" are hard to distinguish. The K-weighting
filter coefficients and the gating algorithm are fiddly enough that transcribing
them ourselves would be strictly worse. Adopt it, and accept that we may end up
maintaining a fork if something does break.

**There is no usable limiter crate.** I searched; the results were noise.
`fundsp` 0.23.0 has a limiter, but pulling a full synthesis DSL in for one
processor is the wrong trade. A look-ahead limiter for the master bus is about
150 lines and we should write it.

**Verdict: stay** on cpal + our mixer + swresample; **adopt** `ebur128` when
loudness normalisation appears on the roadmap; **revisit** rubato when speed
ramps do.

---

## 4. Text rendering

The requirement — user fonts, multiple outlines, shadows, per-line background
boxes, per-character animation, RTL, CJK, emoji — has no crate-shaped answer.
What the ecosystem provides is the bottom two thirds: font enumeration,
shaping, line breaking, and glyph outlines. Everything above that is ours. The
job is picking the bottom two thirds well, because that is the part that is
genuinely hard to write and genuinely easy to get wrong for Arabic.

### The landscape moved in the last year, twice

Two shifts that any older advice will miss:

**`rustybuzz` is finished, `harfrust` replaced it.**
[`rustybuzz`](https://github.com/harfbuzz/rustybuzz) 0.20.1 was released
2024-11-12 and the repository's last commit is 2025-06-09 — nothing since. Its
successor, [`harfrust`](https://github.com/harfbuzz/harfrust) 0.12.0
(2026-07-03, 2.7 M downloads/90d, pushed 2026-07-14), is the same port migrated
from `ttf-parser` to `read-fonts`, developed in the HarfBuzz organisation with
Behdad Esfahbod among its crates.io owners. Its README claims parity tracking
with HarfBuzz v13.0.0 and "less than 25 % slower than HarfBuzz on most common
fonts". Neither README carries a deprecation notice, which is the only reason
`rustybuzz` still looks alive.

**`swash` is dormant.** Its issue
[#107](https://github.com/dfrg/swash/issues/107) quotes Google's HarfRust
document describing swash as "dormant, with developer Chad Brokaw's time spent
on Google's fontations and HarfRust these days". The 0.2.7–0.2.10 releases
through 2026 are largely skrifa version bumps by other people. It still has no
COLRv1 support — issues [#21](https://github.com/dfrg/swash/issues/21) and #22
have been open since 2021-09-20 — which matters because modern colour emoji
fonts ship COLRv1.

### The candidates, measured

| Crate | Version | Released | 90d | Bus factor | Verdict |
|---|---|---|---|---|---|
| [`parley`](https://github.com/linebender/parley) | 0.11.0 | 2026-06-26 | 1.1 M | Good — 6+ regulars, Linebender | **take** |
| [`fontique`](https://github.com/linebender/parley) | 0.11.0 | 2026-06-26 | 1.1 M | same repo | **take** |
| [`harfrust`](https://github.com/harfbuzz/harfrust) | 0.12.0 | 2026-07-03 | 2.7 M | Good — HarfBuzz org | **take** (via parley) |
| [`skrifa`](https://github.com/googlefonts/fontations) | 0.45.1 | 2026-07-23 | 8.3 M | Very good — Google Fonts | **take** |
| [`kurbo`](https://github.com/linebender/kurbo) | 0.13.1 | 2026-05-13 | 13.3 M | Good — Linebender | **take** (stroking) |
| [`cosmic-text`](https://github.com/pop-os/cosmic-text) | 0.19.0 | 2026-04-22 | 2.3 M | **Poor** — 474 commits by one person vs 61 by the next | viable alternative |
| [`zeno`](https://github.com/dfrg/zeno) | 0.3.3 | 2025-05-08 | 3.2 M | Dormant since 2025-06 | usable, feature-complete |
| [`glyphon`](https://github.com/grovesNL/glyphon) | 0.12.0 | 2026-07-09 | 382 k | Weak — ~1 maintainer | **no** |
| [`vello`](https://github.com/linebender/vello) | 0.9.0 | 2026-05-15 | 348 k | Good | **not yet** |
| [`swash`](https://github.com/dfrg/swash) | 0.2.10 | 2026-07-17 | 3.5 M | Dormant author | rasteriser only |
| [`rustybuzz`](https://github.com/harfbuzz/rustybuzz) | 0.20.1 | 2024-11-12 | 8.8 M | **Frozen** | **no** |
| [`harfbuzz_rs`](https://github.com/harfbuzz/harfbuzz_rs) | 2.0.1 | **2021-08-28** | 9.7 k | **Dead as a crate** | **no** |
| [`fontdue`](https://github.com/mooman219/fontdue) | 0.9.3 | 2025-02-12 | 3.3 M | Dormant since 2025-05 | **no** |
| [`ab_glyph`](https://github.com/alexheretic/ab-glyph) | 0.2.32 | 2025-09-28 | 8.8 M | 1 maintainer, responsive | no shaping → **no** |

### Why glyphon is out, despite being the obvious fit

`glyphon` is a wgpu text renderer over cosmic-text and it tracks wgpu versions
impressively fast — 0.12.0 landed on 2026-07-09, eight days after wgpu 30. It
is also structurally incapable of what we need:

- **No transforms.** Issue [#4](https://github.com/grovesNL/glyphon/issues/4),
  "Consider handling transformations", has been open since **2022-05-10**. A
  `TextArea` has `left`, `top`, `scale` and `bounds`. Per-character rotation is
  not expressible.
- **No stroke, no shadow.** Issue
  [#153](https://github.com/grovesNL/glyphon/issues/153), open since 2025-08-10.
- It hands you an atlas quad, never a path, so we cannot stroke or expand
  outlines ourselves either.

Per-character animation *is* the CapCut title feature. A renderer that cannot
rotate a glyph is not a candidate. It remains a fine choice for plain UI text if
we ever want native-rendered text in the engine, which we do not — that is the
webview's job.

### Why vello is "not yet" rather than "no"

`vello` would otherwise be attractive: `DrawGlyphs::draw` takes a `Fill` *or* a
`Stroke`, there is a per-glyph `glyph_transform`, and it handles COLRv1 and
bitmap emoji through skrifa. But it is pinned to **wgpu 29.0.3**; the upgrade PR
[#1754](https://github.com/linebender/vello/pull/1754) was still a draft on
2026-07-19, blocked on a wgpu-profiler release. Linebender's own Q1 2026 post
calls it "roughly beta quality". For a compositor on wgpu 30 that means
downgrading wgpu or vendoring vello, both of which cost more than the text
rasteriser is worth. Revisit when #1754 lands.

`vello_cpu` 0.0.9 is a different proposition — SIMD, multithreaded, no wgpu
dependency at all — and is a plausible CPU rasteriser for the "render the text
layer once, then animate it as a texture" design below. Version 0.0.x means the
API will move under us.

### The recommendation

```
fontique 0.11   → font enumeration, user font loading, fallback chains
parley 0.11     → line breaking, bidi, letter/word spacing, inline boxes
  └ harfrust    → shaping (pulled in by parley_engine; nothing to wire)
skrifa 0.45     → per-glyph outlines into kurbo::BezPath; COLRv1 + CBDT/sbix
kurbo::stroke   → outline expansion for strokes
our wgpu code   → raster, atlas, per-glyph quads, blur, boxes, animation
```

`parley` gives `Glyph { id, style_index, x, y, advance }` per `GlyphRun`, which
is exactly the granularity per-character animation needs, and its
`StyleProperty` enum already has `LetterSpacing`, `WordSpacing`, `LineHeight`
and `FontVariations`. Its `complex-scripts` feature adds dictionary-based line
breaking for CJK, Thai, Khmer, Lao and Burmese.

The alternative, `cosmic-text`, is more convenient — it hands you outlines
directly via `SwashCache::get_outline_commands` and comes with a working editor
and cursor model. Two things push me to parley anyway: `cosmic-text` renders
through the dormant `swash` and therefore **cannot display COLRv1 colour
emoji**, and its bus factor is one person by a factor of eight (474 commits to
the next contributor's 61, with five commits in the last three months). parley's
cost is real and should be stated: 0.8 through 0.11 all shipped in the last four
months, all with breaking changes. We will be doing upgrade work.

### What we build regardless of choice

Nothing in any Rust crate does any of this:

1. **Per-character animation** — timeline, stagger, easing, per-glyph transform
   and colour interpolation.
2. **Multiple stacked outlines** — repeated `kurbo::stroke` at increasing widths
   painted back to front, with nonzero fill to survive self-intersection.
3. **Shadows and glow** — no Rust text crate does blur. This is a separable
   Gaussian pass in our compositor, which we want anyway.
4. **Background boxes** — derive line rectangles from the layout, pad, merge,
   round the corners. Pure geometry, entirely ours.
5. **Vertical CJK layout** — see below.
6. **COLRv1 compositing** — skrifa gives paint callbacks with gradients, clips
   and composite modes; executing them is our renderer's job.
7. **Glyph atlas and its invalidation** across scale and effect parameters.

The design that follows from this is the one CapCut itself uses: rasterise each
glyph *with all its effect layers baked in* once into an atlas tile, then
animate a per-glyph quad with its own instance transform, re-rasterising only
when the scale changes materially. Cheap, deterministic, and it makes
per-character animation a vertex-buffer problem.

### Vertical CJK: nobody has this

Stated plainly because it will otherwise be discovered late. **No Rust text
stack supports `writing-mode: vertical-rl`.** cosmic-text issue
[#11 "Vertical text"](https://github.com/pop-os/cosmic-text/issues/11) has been
open since 2022-10-24. parley has nothing.

The shaping layer can do it — `harfrust::Direction` has `TopToBottom`, which
gets you `y_advance`/`y_offset`, `vmtx`/`VORG` metrics and the `vert`/`vrt2`
features. It is the *layout* layer that is missing, and it is ours to write:
columns as the main axis advancing leftward, full-width punctuation recentering,
tate-chu-yoko, 90° rotation of Latin runs, vertical kinsoku. The Rust project
[koharu](https://koharu.rs/explanation/text-rendering-and-vertical-cjk-layout/)
has done exactly this on harfrust + ICU4X + skrifa and written up the approach;
it is the best available reference.

Whether this matters depends on a decision not yet made: whether chukcut targets
CJK markets at launch. If not, it is Phase 4 or later.

**Verdict: adopt** parley + fontique + harfrust + skrifa + kurbo. Budget for the
effect layer being entirely ours, because it is.

---

## 5. Colour management

### What correct actually requires

Every decoded frame carries four independent tags, and `ffmpeg-next` already
exposes all of them — `util::color` has `primaries.rs`, `space.rs`,
`transfer_characteristic.rs` and `range.rs`, present in our pinned 6.1.1:

1. **Matrix coefficients** — how YCbCr becomes R'G'B'. BT.601 for SD and most
   phone video that lies about it, BT.709 for HD, BT.2020 for UHD.
2. **Range** — limited (16–235) or full (0–255). Getting this wrong is the
   classic "washed out / crushed blacks" bug and it is off by only 7 %, which is
   exactly enough to look like a deliberate grade rather than a bug.
3. **Transfer characteristics** — the EOTF. BT.709's OETF, sRGB's (they differ),
   PQ, HLG.
4. **Primaries** — which actual chromaticities R, G and B mean.

Composite correctly and you convert to linear light, blend, then re-encode.
Composite in gamma space and cross-fades go muddy in the middle and 50 % opacity
white over black is not 50 % grey. Consumer editors mostly get this wrong, and
users have partly calibrated to the wrong answer, so the decision of *where* to
blend is a product decision as much as a correctness one — but it should be a
decision, not an accident, and it needs the tags either way.

Then there is the boundary problem: text and UI overlays are authored in sRGB
and must land in the video's space, and the preview is JPEG in sRGB while the
export is BT.709 limited-range YUV.

### moxcms, and the fact that we already ship it

| Crate | Version | Released | 90d | Assessment |
|---|---|---|---|---|
| [`moxcms`](https://github.com/awxkee/moxcms) | 0.9.0 | 2026-07-22 | **27.3 M** | **Adopt.** Already in `Cargo.lock` via `image`. |
| [`yuv`](https://github.com/awxkee/yuvutils-rs) | 0.8.16 | 2026-06-14 | 1.2 M | Adopt if CPU-side YUV conversion is ever needed. |
| [`palette`](https://github.com/Ogeon/palette) | 0.7.6 | 2024-04-28 | 4.6 M | Type-safe colour algebra. No CICP, no video. |
| [`lcms2`](https://github.com/kornelski/rust-lcms2) | 6.1.1 | 2025-07-13 | 793 k | Little-CMS binding. Heavy, C, ICC-centric. |
| [`qcms`](https://github.com/FirefoxGraphics/qcms) | 0.3.0 | 2024-01-09 | 1.1 M | Firefox's CMS. Display-profile shaped. |
| [`colorutils-rs`](https://github.com/awxkee/colorutils-rs) | 0.8.0 | 2026-04-12 | 33 k | Same author, older, narrower. Superseded in practice. |
| [`kolor`](https://github.com/kabergstrom/kolor) | 0.1.9 | 2023-04-25 | **521** | Dead. |
| [`colstodian`](https://github.com/termhn/colstodian) | 0.1.0-rc.3 | **2021-07-15** | **144** | Dead. |

`moxcms` is the answer and the reason is specific: it models **CICP**, the
ITU-T H.273 / ISO 23091-4 code points that video actually uses, rather than
generic colour theory. Verified on docs.rs 0.9.0, it has `CicpColorPrimaries`,
`TransferCharacteristics` and `MatrixCoefficients` enums, with
`TransferCharacteristics` carrying `Bt709` (1), `Bt601` (6), `Linear` (8),
`Srgb` (13), `Smpte2084` (16, PQ) and `Hlg` (18), and methods
`linearize(f64) -> f64` and `gamma(f64) -> f64` on the enum itself. It also has
primaries → colorant matrices (`bt2020_colorants()`, `bt2020_matrix()`, and the
equivalents for sRGB, Display P3, DCI-P3, Adobe RGB, ProPhoto, ACES).

That is precisely the set of tables and reference implementations we would
otherwise transcribe from specifications by hand, which is a task where a
transposed matrix produces a subtly wrong image that survives review.

The decisive practical point: **`moxcms` is already in our lockfile**, pulled
in by `image` 0.25. Using it directly adds nothing to the binary and no new
supply-chain surface. That flips the calculus completely — a crate we already
carry is free.

The caution, and it is real: `moxcms` has **47 GitHub stars and one maintainer**
(Radzivon Bartoshyk, "awxkee"). The 27 M downloads per quarter are almost
entirely transitive through `image`, not a sign of a broad contributor base. The
mitigation is that we would use it as a source of colour *mathematics* — pure
functions over f32 — not as an architectural dependency. If it were abandoned we
could vendor the parts we use in an afternoon. That is an acceptable exposure
for tables; it would not be for, say, a decoder.

`palette` is the crate most people reach for and it is the wrong shape here: it
is excellent for typed colour algebra in an sRGB/Lab/Oklab world and has no
concept of CICP tags, matrix coefficients or limited range. `lcms2` and `qcms`
solve ICC profile transforms, which is a display-calibration problem, not a
video-primaries problem. `kolor` and `colstodian` are dead — 521 and 144
downloads in 90 days respectively, last released 2023 and 2021.

### Where the conversion runs

Almost all of it belongs in the shader: the compositor samples Y and UV planes,
applies the matrix and range, linearises, blends, re-encodes. `moxcms` supplies
the constants and a CPU reference implementation to test the shader against,
which is the more valuable of the two — a wrong colour matrix is invisible in
review and obvious in the exported file.

`yuv` 0.8.16 (same author, SIMD, BT.601/709/2020, limited and full range, 8 to
16 bit, NV12/P010/planar) is the crate to reach for if a CPU path is ever needed
— thumbnails from a decoded frame, or a fallback when there is no GPU. Not
needed today.

HDR is a Phase 4-or-later question and depends on a decision we have not made
(does the compositor render to `Rgba16Float` always, or only for HDR
projects?). `moxcms` covers the PQ and HLG transfer functions either way; tone
mapping for SDR export is ours regardless.

**Verdict: adopt `moxcms`** for CICP tables, transfer functions and primaries
matrices. Keep the conversions in our shaders.

---

## 6. GPU effect graphs

Two separate questions here, and they have different answers.

### 6a. Is there a render-graph crate worth adopting?

No.

| Crate | Version | Released | 90d | State |
|---|---|---|---|---|
| [`rend3`](https://github.com/BVE-Reborn/rend3) | 0.3.0 | **2022-02-12** | **501** | **Dead.** Repository **archived 2025-06-07**. |
| [`blade-graphics`](https://github.com/kvark/blade) | 0.8.4 | 2026-04-18 | 110 k | Alive and healthy, but its own Vulkan/Metal/GLES backends — a wgpu *competitor*, not a layer over it. |
| [`tweak_shader`](https://crates.io/crates/tweak_shader) | 0.6.1 | 2025-12-16 | 175 | Closest thing to our problem that exists. See below. |
| [`naga_oil`](https://github.com/bevyengine/naga_oil) | 0.23.0 | 2026-07-14 | 1.3 M | Shader *composition* (`#import`, `#ifdef`) over naga. Already on naga 30 / wgpu 30. |
| [`vello`](https://github.com/linebender/vello) | 0.9.0 | 2026-05-15 | 348 k | 2D vector renderer. Not an effect graph. Pinned to wgpu 29. |
| `render-graph` 0.0.1 | 2018 | — | — | Name squatting. Description: "Reserved for planned future use." |
| `frame_graph` 0.1.0 | 2026-06-02 | 17 | — | Toy. |

Bevy's render graph is the only mature one and it is not extractable — it is
bound to Bevy's render world, sub-apps and extract schedule, and Bevy's own
maintainers describe the supported direction as omitting `bevy_render` and
writing your own renderer, not the reverse. The generic crates in this space are
built for 3D scene rendering, where the graph nodes are shadow passes and
G-buffers. A video effect graph is a different shape: a DAG of full-screen
passes over ping-pong render targets, with the interesting complexity in
resource lifetime, format selection and cache invalidation across timeline
positions, not in scheduling.

That is a few hundred lines and it will be better than anything generic because
it knows what a frame is. Both Gyroflow and Gausian — the two other serious
Rust video projects on wgpu — roll their own for the same reason.

The one crate worth reading before writing ours is
**[`tweak_shader`](https://crates.io/crates/tweak_shader) 0.6.1** (2025-12-16).
It is exactly our problem class: a wgpu render context for ISF/Shadertoy-style
GLSL screen shaders, with multi-pass support, custom uniforms, input textures,
and a `#pragma input(float, name=..., min=..., max=...)` convention for
declaring a parameter UI from the shader source — which is precisely what our
effect manifests do. It is also on **wgpu 27**, has 3 GitHub stars, one
maintainer and 175 downloads in 90 days, and it consumes `ShaderSource::Glsl`,
meaning it inherits every problem in §6b below. **Read it for the design, do not
depend on it.**

`naga_oil` is a separate and more likely adoption: if our own compositor shaders
start wanting `#import` and `#ifdef` — and a multi-pass effect graph with shared
helper libraries will — it is the maintained answer, and it already tracks naga
30 and wgpu 30. That is a convenience for shaders *we* write, not for the CapCut
corpus. Same for `encase` (UBO layout) and `wgpu-profiler` (per-pass timer
queries), both maintained, both worth having when the graph exists.

The Lua half is settled: [`mlua`](https://github.com/mlua-rs/mlua) 0.12.0
(2026-07-05, 1.2 M/90d, 2804 stars) is the only serious option.
[`piccolo`](https://github.com/kyren/piccolo) 0.3.3 is a pure-Rust stackless Lua
that is genuinely interesting and genuinely incomplete — last release 2024-06-16,
repo last pushed 2025-07-10. `rlua` is now a compatibility shim over mlua.
`hematita` is abandoned. Stay with mlua.

### 6b. The shader path in the roadmap does not work, for two reasons

Phase 3 says "GLSL ES 1.0/3.0 → WGSL translation". The implicit plan is
`naga::front::glsl`. That plan fails twice, and the second failure is the one
that would have been discovered late and painfully.

**Failure one: naga's GLSL frontend accepts no ES profile at all.**

`naga/src/front/glsl/mod.rs` documents its supported versions as Vulkan 440
(partial), 450 and 460. `parser.rs` enforces it — the `#version` handler,
verbatim:

```rust
440 | 450 | 460 => self.meta.version = int.value as u16,
_ => self.errors.push(Error {
    kind: ErrorKind::InvalidVersion(int.value),
    meta: location.into(),
}),
```

and the profile handler accepts only `core`, erroring on `es`. Confirmed by
running `naga-cli` 30.0.0 against real input: `#version 100` gives
`error: Invalid version: 100` followed by `Expected end of file, found
Identifier("varying")`; `#version 300 es` gives `Invalid version: 300` and
`Invalid profile: es`. The tracking issue
[gfx-rs/wgpu#6335](https://github.com/gfx-rs/wgpu/issues/6335), "Naga frontend
supports gles" (2024-09-27), was **closed as not planned**.

Against our corpus, from [`effect-package-format.md`](effect-package-format.md):
of 228 sampled fragment shaders, **224 have no `#version` directive at all** and
open with `precision highp float;`; four use `#version 300 es`. 222 call
`texture2D(...)`, 152 write `gl_FragColor`. So naga glsl-in rejects 100 % of
them.

The naga GLSL frontend is maintained but explicitly second-class: wgpu's own
support matrix marks `wgsl-in` and `spv-in` as first class and `glsl-in` as
best-effort, and there are **64 open issues labelled `lang: GLSL`**, the oldest
from 2021-02-17. The direction of travel is away from runtime shader
translation, not toward it — the "Precompiled Shaders" tracking issue
([#9052](https://github.com/gfx-rs/wgpu/issues/9052), 2026-02-13) has making
naga an *optional* dependency as a goal.

**Failure two: `sampler2D` breaks both naga frontends, whatever you do.**

WebGPU has no combined image samplers ([gpuweb#770](https://github.com/gpuweb/gpuweb/issues/770)),
and every CapCut shader uses `uniform sampler2D`. Verified by running both
frontends:

- naga **glsl-in** on `layout(binding=0) uniform sampler2D uTex;` →
  `error: Not implemented: variable qualifier`, pointing at `sampler2D`.
- naga **spv-in** on glslang-produced SPIR-V containing a combined sampler →
  `invalid id %59`.
- Split into `uniform texture2D` + `uniform sampler` with
  `texture(sampler2D(t, s), uv)` → both work.

The glsl-in issue,
[#4342](https://github.com/gfx-rs/wgpu/issues/4342) "Consider supporting
combined image/samplers", has been open since **2021-06-22**; its author
describes converting GLSL ES to WGSL at runtime, which is exactly our use case.
PR [#9558](https://github.com/gfx-rs/wgpu/pull/9558) (2026-05-16) is still open
with reported bugs on `sampler2D` as a function parameter. Do not wait for it.

And glslang is no escape on its own — verified against glslang 15.1.0:

```
$ glslangValidator -V es100.frag
ERROR: #version: ES shaders for SPIR-V require version 310 or higher
ERROR: es100.frag:3: 'varying' : no longer supported in es profile
```

The commonly suggested `-std=450core` override makes it worse, not better:
`varying`, `gl_FragColor` and `texture2D` then become illegal for a different
reason.

### The pipeline that does work

Run end to end with the real toolchains — glslang 15.1.0, `naga-cli` 30.0.0,
`spirv-webgpu-transform` 0.1.6 — against a *representative* CapCut-shaped shader
(two combined samplers, a helper function, `mod`/`mix`, `highp`). Representative,
not one of ours: the corpus itself has not been run through this yet, which is
the conformance run recommended at the end of this section.

```
 GLSL ES 1.0 source (no #version, precision highp float;)
        │
        │  [1] our rewriter — mechanical, ours to write
        │      #version 450 core · varying → in · attribute → in
        │      gl_FragColor → layout(location=0) out vec4
        │      texture2D → texture · textureCube → texture
        │      texture2DProj → textureProj
        │      loose uniforms → a uniform block
        ▼
 Vulkan-flavoured GLSL 450
        │
        │  [2] glslangValidator -V --auto-map-bindings --auto-map-locations
        ▼
 SPIR-V (still with combined image samplers)
        │
        │  [3] spirv-webgpu-transform: split combined samplers
        ▼
 SPIR-V (separate texture + sampler) + CorrectionMap
        │
        │  [4] naga spv-in  ── first-class frontend
        ▼
 naga IR / WGSL  ──►  wgpu pipeline
```

Step [3] is [`spirv-webgpu-transform`](https://crates.io/crates/spirv-webgpu-transform)
0.1.6 (2026-05-29), by davnotdev, written for **Godot's WebGPU driver**. It
splits combined image samplers in the SPIR-V bytecode, including through
function parameters and nesting. Bus factor is one and it has 4 GitHub stars —
but it has **no runtime dependencies at all** (naga and spirv-tools are
dev-dependencies only), so vendoring or forking it is a contained risk. Without
step [3], step [4] fails with `invalid id %59`; with it, the output is clean
WGSL with correctly lowered `mod()`/`mix()` and the helper function preserved.

Note that this pipeline splits the samplers on the *SPIR-V* level rather than in
the GLSL source, which is materially easier and less error-prone than doing it
textually — and it is the reason to prefer spv-in over glsl-in even after the
rewrite makes glsl-in theoretically viable.

**Gotchas, all verified by running the tools:**

1. **Loose uniforms are illegal in Vulkan GLSL.** `error: 'non-opaque uniforms
   outside a block' : not allowed when using GLSL for Vulkan`. Every scalar and
   vector uniform must be wrapped in a block — which is nearly every CapCut
   shader, since the Lua side sets them individually via `material:setFloat(...)`.
   The rewriter has to synthesise the block and the Lua binding has to write
   into it by offset. This is the single largest piece of work in step [1].
2. **Missing bindings and locations** are solved by
   `--auto-map-bindings --auto-map-locations`.
3. **Precision qualifiers** survive: glslang accepts `precision highp float;` in
   450 core and emits `RelaxedPrecision` decorations, which naga logs as unknown
   and ignores. Functionally correct; no `mediump` performance benefit.
4. **Bindings get renumbered** by the sampler split. In the test, source
   bindings 0/1/2 became texture@0, sampler@1, texture@2, sampler@3, ubo@4. The
   `CorrectionMap` must be read to build bind-group layouts — and its CLI summary
   under-reported the splits, so use the library API
   (`combimgsampsplitter(&mut Vec<u32>, &mut CorrectionMap)`) and not the CLI
   output.
5. **`sampler2D[N]` and `sampler2DArray[N]` are unsupported** by
   `spirv-webgpu-transform`, per its README. Unknown how many corpus shaders use
   arrays of samplers; worth counting before committing.

**Toolchain choice for step [2]**: [`shaderc`](https://github.com/google/shaderc-rs)
0.10.1 (2025-09-06, 509 k/90d, 286 stars) is the well-known binding;
[`glslang`](https://github.com/SnowflakePowered/glslang-rs) 0.8.1 (2026-05-11,
20 k/90d) is a lighter one from the same author as `spirv-cross2`. Both drag a
C++ build in. Whether either finds prebuilt libraries on our targets, or forces
a CMake-and-Python build, is **not verified** and is the main practical risk in
this plan — it lands on the Windows and macOS builds, not on Linux.

`spirv_cross` 0.23.1 (last released **2021-03-03**) is dead; `spirv-cross2`
0.7.1 (2026-05-21) is alive but tiny. Neither is needed on this path.

For step [1], [`glsl-lang`](https://github.com/alixinne/glsl-lang) 0.8.1
(2025-09-15, 131 k/90d, 25 stars, one maintainer) is the only real Rust GLSL
parser, and it is declared as a *GLSL 4.6* parser, where `attribute` and
`varying` are reserved words. Whether it parses ES 1.0 at all is **not verified**
and should be tested against ten real shaders before anyone plans around it.
Given how stylistically uniform the corpus is — machine output from ByteDance's
`.ausl` compiler, mangled identifiers like `_f0`/`_p0` — a token-level pre-pass
is likely sufficient and the failures are loud (a compile error, never a wrong
pixel).

The option worth naming and rejecting: ANGLE's shader translator is literally
built to consume ES 1.0 and emit SPIR-V, and it is the component CapCut itself
ships (`VEAngle/libGLESv2.dll`). Binding it means vendoring a large C++ build
into a Tauri app for one translation step. Not worth it unless the pipeline
above fails on real content.

**Verdict: stay** on our own multi-pass graph; **switch** the shader plan from
"naga glsl-in" to the four-step pipeline above. The corpus is fixed and failures
are compile-time, so a one-off conformance run over every shader we intend to
support answers the remaining unknowns in an afternoon — and that run should
happen before Phase 3 starts, not during it.

---

## 7. Waveforms, thumbnails, proxies

Correctly ours. This is the shortest section because the search was
comprehensive and empty: crates.io has nothing for audio peak extraction beyond
`audio-viz` (45 downloads in 90 days), nothing for video thumbnail strips, and
nothing for proxy generation.

That is not surprising. Each of these is a hundred lines of glue over decoders
we already have, and all the difficulty is in *policy* rather than algorithm:

- **Waveforms** — decode, take min/max per bucket at several zoom levels, cache
  to disk keyed by content hash, invalidate on trim. The peak-picking is
  trivial; the multi-resolution cache and its invalidation is the work.
- **Thumbnails** — seek to keyframes, decode, scale, pack into a strip. The work
  is deciding *which* frames (keyframe-aligned, or accurate and slow) and how to
  present the strip at arbitrary timeline zoom without re-extracting.
- **Proxies** — transcode to a fast intermediate, then substitute at render time
  when a flag is set. The work is entirely in the substitution logic and in not
  letting a stale proxy paint a wrong frame.

Two crates are worth borrowing at the edges:

- [`fast_image_resize`](https://github.com/cykooz/fast_image_resize) 6.1.0
  (2026-07-21, 2.6 M/90d) — SIMD image scaling, substantially faster than
  `image::imageops`. Directly useful for thumbnail strips and any CPU-side
  downscale. Cheap, well maintained, easy to adopt.
- [`yuv`](https://github.com/awxkee/yuvutils-rs) 0.8.16 — as in §5, if a
  thumbnail path ever needs NV12 → RGB on the CPU rather than through swscale.

On the adjacent preview encode: `jpeg-encoder` 0.7.0 at ~8 ms per proxy frame is
fine and pure Rust. If that ever becomes the bottleneck,
[`turbojpeg`](https://github.com/honzasp/rust-turbojpeg) 1.5.1 (updated
2026-07-25, 761 k/90d) wraps libjpeg-turbo and would be roughly three times
faster, at the cost of a C dependency in the Windows and macOS builds. Not a
trade worth making before it is measured. `zune-jpeg` is decode-only, so it is
not an alternative here — though it is already in our tree via `image`.

**Verdict: stay.** Adopt `fast_image_resize` when the thumbnail path is written.

---

## 8. Timeline data structures and OTIO

### No crate models an editing timeline

Searched crates.io for `opentimelineio`, `otio`, `edl`, `timeline`, `nle`,
"video editing timeline", "edit decision list". The results:

| Crate | Version | Released | 90d | State |
|---|---|---|---|---|
| `opentimelineio` | 0.1.0 | **2020-10-20** | **29** | Description reads "Rust bindings for OpenTimelineIO (**placeholder**)". The repository ([scott-wilson/OpenTimelineIO-rs](https://github.com/scott-wilson/OpenTimelineIO-rs)) has **one commit, "Initial commit", from 2020-10-20**. |
| `opentimelineio-sys` | 0.0.0 | 2020-11-03 | 7 | Same, also a placeholder. |
| `edl` | 1.1.2 | 2022-10-06 | 24 | CMX 3600 parsing. Abandoned, and CMX 3600 is not our interchange problem. |
| `oxideav-scene` | 0.1.4 | 2026-05-31 | 985 | New, tiny, unknown provenance. Not a foundation. |

A caution on that last row and its neighbours. `oxideav-scene`, `oximedia-*`
(which advertises itself as a "pure Rust reconstruction of both FFmpeg and
OpenCV" — a claim that should be read as a warning label) and
`kael_media_engines` all appeared recently, have negligible usage, and have the
texture of machine-generated crates. Nobody read their source for this survey.
Treat anything in this corner of crates.io as unverified until someone does.

The one genuinely useful thing nearby is time primitives:
[`mediatime`](https://crates.io/crates/mediatime) 0.1.9 (2026-07-03) models
exact rational time with FFmpeg-style `Timebase`/`Timestamp`/`TimeRange`,
`no_std`, zero dependencies, `const fn`. It is technically well made and four
months old with two stars. We use `i64` microseconds by decision
([`project-format.md`](../architecture/project-format.md)), so this is only
interesting if that decision is ever revisited — and if it is, the crate is
small enough to vendor.

This is the expected result and it should not be read as a gap in the ecosystem.
An NLE timeline model is where every editor encodes its *entire* semantics —
what a trim does at a transition boundary, whether ripple propagates across
tracks, how nested sequences resolve, what a speed ramp does to downstream
timing. Two editors with the same struct definitions still behave differently.
There is nothing generic to share, which is why nobody has shared it.

Our model is already documented in
[`project-format.md`](../architecture/project-format.md) and
[`timeline-editing.md`](../architecture/timeline-editing.md), with i64
microseconds, source-vs-target ranges, a material pool by id, and all mutation
through `EditCommand`. Those are the right decisions. Keep them.

### OTIO: cheap to add, thin in what it carries

[OpenTimelineIO](https://github.com/AcademySoftwareFoundation/OpenTimelineIO) is
the ASWF interchange format for editorial timelines. Current release **v0.18.1,
tagged 2025-11-08** (also on PyPI, 2025-11-09) — still pre-1.0 after roughly a
decade, which is itself informative about the pace. The repository is 767 k
lines of C++ and 665 k of Python; there is **no Rust binding worth the name**,
only the two 2020 placeholder crates above.

That does not matter, because OTIO's serialisation is plain JSON with an
`OTIO_SCHEMA` discriminator string on every object:

```json
{ "OTIO_SCHEMA": "Timeline.1", "name": "transition_test",
  "tracks": { "OTIO_SCHEMA": "Stack.1", "children": [
    { "OTIO_SCHEMA": "Track.1", "children": [
      { "OTIO_SCHEMA": "Transition.1", "transition_type": "SMPTE_Dissolve",
        "parameters": {},
        "in_offset":  { "OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 10 },
        "out_offset": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 10 } },
      { "OTIO_SCHEMA": "Clip.1", "name": "A", "enabled": true,
        "effects": [], "markers": [], "media_reference": null,
        "source_range": { "OTIO_SCHEMA": "TimeRange.1", "start_time": {}, "duration": {} } }
    ] } ] } }
```

`#[serde(tag = "OTIO_SCHEMA")]` on an internally-tagged enum with
`#[serde(rename = "Clip.2")]` maps this one-to-one. Perhaps 300 to 600 lines, a
weekend, no C++ binding. The gotchas are ordinary: schema versions are per-type
and both `Clip.1` and `Clip.2` appear in OTIO's own sample data (the
upgrade/downgrade machinery lives only in the C++ library, so we must accept
several version tags per type); `media_reference` is polymorphic across
`null`/`ExternalReference.1`/`MissingReference.1`/`ImageSequenceReference.1`/
`GeneratorReference.1`; `.otioz` and `.otiod` are ZIP and directory bundles
around a `content.otio`; `rate` and `value` are floats, not integers.

Adoption is real and growing: **DaVinci Resolve** since 18.5 (2023), and
**Adobe Premiere 26.0** shipped OTIO import/export in January 2026 after a
beta from late 2024. Nuke Studio and Hiero do a full round trip; Avid goes
through the AAF adapter; Kdenlive and Final Cut via community adapters. (I could
not verify Autodesk Flame.) No consumer editor supports it — CapCut certainly
does not.

The question is whether it is worth the weekend, and the answer is *not yet*,
for a reason that has nothing to do with implementation cost.

OTIO models **editorial decisions**: what clip, from what source, at what time,
on what track. What it does not model is anything with parameters. The `Effect.1`
schema (`src/opentimelineio/effect.h`) has exactly four fields — `name`,
`effect_name`, `enabled`, and an opaque `metadata` dictionary. **There are no
parameters.** The only standardised effects in the whole format are
`LinearTimeWarp.1`, whose single field is `time_scalar: double`, and
`FreezeFrame`. `Transition.1` has a `parameters` dict that is empty in practice.

OTIO's own test data proves the consequence. `tests/sample_data/effects.otio` is
a Resolve export, and a Transform effect in it looks like this:

```json
"effects": [ { "OTIO_SCHEMA": "Effect.1",
  "metadata": { "Resolve_OTIO": {
    "Effect Name": "Transform",
    "Parameters": [ { "Parameter ID": "transformationZoomX",
      "Parameter Value": 1.1899998188018799, "Variant Type": "Double",
      "Key Frames": { "3": { "Value": 2.1103820 },
                      "424": { "Value": 1.5003825,
                               "InBez": { "InBez": [-49.75, 0.0] } } } } ] } } } ]
```

Every keyframe, every Bézier handle, every range is inside a Resolve-private
blob that no other host reads. This is acknowledged upstream: in
[Discussion #921](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/discussions/921),
the maintainers describe AAF effect parameters as living entirely in
AAF-specific metadata and suggest starting with "a small set of simple effects,
without any animated keyframes"; an OpenFX-inspired standardisation proposal
from May 2025 is still a proposal, and keyframe animation is blocked on a
separate issue. They also note that different hosts apply transforms in
different orders, which is the deeper reason this is hard.

So an OTIO export from chukcut would faithfully communicate the cuts, the
timing, markers, speed ramps and dissolve durations — and lose text, titles,
stickers, filters, colour grades, keyframe animation and audio levels. That is
to say: it would lose the reason someone used chukcut. Which is correct
behaviour for an interchange format, and exactly why it is a feature for a user
who edits here and finishes in Resolve, not for the user Phases 1 through 3 are
built for.

**Verdict: stay** on our own model. **Revisit** OTIO *export* — import is harder
and gains us nothing — if a professional-workflow user ever asks. It has
marketing value out of proportion to its cost. When it comes, write it with
`serde` and skip the bindings entirely.

---

## Crates flagged as unmaintained or risky

Adopting a dead crate is worse than writing the code, so these are called out
even where they appear in recommendations.

| Crate | Last release | Last commit | Signal | Our exposure |
|---|---|---|---|---|
| `harfbuzz_rs` 2.0.1 | **2021-08-28** | 2025-08-11 | 9.7 k downloads/90d. Repo moved into the harfbuzz org and gets occasional commits, but **no release in five years**. | None — do not adopt. |
| `rustybuzz` 0.20.1 | 2024-11-12 | **2025-06-09** | Superseded by `harfrust`. No deprecation notice, which is the trap. | None — parley uses harfrust. |
| `harfbuzz_rs_now` 2.3.2 | 2025-05-01 | — | 411 downloads/90d, history full of yanked versions. | None. |
| `swash` 0.2.10 | 2026-07-17 | 2026-07-17 | Author publicly "dormant"; releases are third-party skrifa bumps; no COLRv1 since 2021 (#21, #22). | Indirect if we chose cosmic-text. We are not. |
| `zeno` 0.3.3 | 2025-05-08 | ~2025-06 | Feature-complete and widely depended on, but nobody is fixing anything. | Optional — `kurbo::stroke` is the live alternative. |
| `fontdue` 0.9.3 | 2025-02-12 | 2025-05-25 | 38 open issues, dormant. | None. |
| `glyphon` 0.12.0 | 2026-07-09 | 2026-07-09 | Tracks wgpu fast, but ~1 maintainer and 12 commits in all of 2026. | None — rejected on capability. |
| `rsmpeg` 0.18.0 | 2025-08-24 | **2025-08-24** | Eleven months silent on a crate whose job is tracking FFmpeg. | None — staying on ffmpeg-next. |
| `ebur128` 0.1.10 | 2024-10-26 | 2025-02-24 | Looks stale; implements a frozen standard, so probably just finished. Sole maintainer. | **Accepted** — we would adopt this. Fork risk is low and cheap. |
| `moxcms` 0.9.0 | 2026-07-22 | 2026-07-24 | Very active but **47 stars, one maintainer**; the 27 M downloads are transitive through `image`. | **Accepted** — already in our lockfile, used for pure functions, vendorable. |
| `mp4parse` 0.17.0 | 2023-05-29 | 2026-07-17 | Repo alive (Mozilla), crate release three years old. | None. |
| `opentimelineio` 0.1.0 | 2020-10-20 | 2020-10-20 | Literally one commit. Self-described placeholder. | None. |
| `kolor` 0.1.9 / `colstodian` 0.1.0-rc.3 | 2023-04-25 / **2021-07-15** | — | 521 and 144 downloads/90d. | None. |
| `rend3` 0.3.0 | **2022-02-12** | 2024-07-08 | **Repository archived 2025-06-07.** 501 downloads/90d. | None. |
| `spirv_cross` 0.23.1 | **2021-03-03** | — | Dead. Its recent downloads are legacy gfx-rs. `spirv-cross2` 0.7.1 is alive but tiny; neither is on our path. | None. |
| `spirv-webgpu-transform` 0.1.6 | 2026-05-29 | 2026-05 | 4 stars, one maintainer, 1.2 k downloads. **But zero runtime dependencies**, so vendoring is trivial. Used by Godot's WebGPU driver. | **Accepted** — §6b depends on it. Plan to vendor. |
| `tweak_shader` 0.6.1 | 2025-12-16 | — | 3 stars, one maintainer, 175 downloads/90d, on wgpu 27. | None — read only. |
| `cros-libva` 0.0.13 | 2024-12-06 | 2026-03-30 | ~19 months of merged work unpublished; docs.rs build failing. | None. |
| `nvidia-video-codec-sdk` 0.4.0 | 2025-09-15 | 2026-03-31 | One maintainer, decode has no safe wrapper, pinned to an old `cudarc`. | None yet — NVENC is Phase 4. |
| `shiguredo_vpl` 2026.3.0 | 2026-06-23 | — | 2 stars, 5 months old, contribution policy is Japanese-only Discord. | None. |
| `gpu-video` 0.4.0 | 2026-05-12 | 2026-07-24 | 3.9 k downloads, young, pinned to wgpu 29. Backed by a funded team (Smelter). | Evaluation only, for now. |
| `mediatime` 0.1.9 | 2026-07-03 | — | Four months old, 2 stars. Small enough to vendor. | None. |
| `oximedia-*`, `oxideav-scene`, `kael_media_engines` | 2026 | — | Negligible usage, extraordinary claims, source unread. | None — avoid. |
| `dasp` 0.11.0 | **2020-05-29** | — | Still 893 k downloads/90d as a transitive dep (cpal uses `dasp_sample`). Fine as a leaf. | Indirect, harmless. |
| `unicode-bidi` 0.3.18 | 2024-12-16 | 2026-06-09 | Slow but alive. cosmic-text depends on it; parley uses ICU4X instead. | None if we take parley. |

## Where recommendations depend on decisions we have not made

- **§2, zero-copy vs transfer.** Whether the zero-copy path is worth building
  depends on whether preview stays at proxy resolution (the current decision in
  [`preview-pipeline.md`](../architecture/preview-pipeline.md)) or moves to
  hole-punched full resolution. At proxy resolution the transfer copy is free.
- **§2b, VAAPI vs Vulkan Video.** These are two different decode stacks with
  different codec coverage, and choosing between them is a Phase 4 decision that
  should be informed by running `gpu-video`'s capability probe on target
  hardware, not by reading. If HEVC decode matters — and phone footage says it
  does — VAAPI wins by default and the `ash` import work is unavoidable.
- **§3, rubato.** Only pays off if speed ramps are implemented as continuous
  varispeed rather than as resampled segments.
- **§4, vertical CJK.** A large piece of bespoke work that is only justified if
  CJK markets are a launch target.
- **§5, HDR.** Whether the compositor renders `Rgba16Float` unconditionally
  changes how much of `moxcms` we need. The CICP tables are useful either way.
- **§6b, translator ambition.** A token-level rewriter is a day or two; a real
  GLSL AST rewriter is a week. Which is right depends on how much of the effect
  corpus we intend to support, which nobody has decided — and the uniform-block
  rewrite means the Lua binding layer has to change with it, so this is not
  purely a shader-side decision.
- **§6b, C++ in the build.** Step [2] needs glslang, which means a C++ toolchain
  in CI for Windows and macOS. If that is unacceptable, the alternative is to
  precompile the corpus to SPIR-V offline and ship only the SPIR-V — which
  conflicts with the legal boundary in `CLAUDE.md`, since the packages are meant
  to be fetched at runtime from a URL the user supplies. Worth resolving before
  Phase 3, not during.
- **§8, OTIO.** Depends entirely on whether a "finish elsewhere" user is a user
  we want.

## What I could not verify

Stated rather than guessed:

- **That `ffmpeg-next` 8.1 links against system FFmpeg 6.1.** The `build.rs`
  version-detection table makes it very likely; I did not run the build.
- **That `texture_from_dmabuf_fd` works on Mesa/ANV at all.** The strongest
  caveat in this document. Grepping the wgpu 30.0.0 tarballs, the function
  appears **only in its own definition** — no test, no example, no CI coverage.
  PR #9366's test plan was `cargo fmt`, `clippy` and `check`. It is
  compile-verified, not runtime-verified, by its own authors.
- **That the VAAPI → DRM_PRIME → Vulkan chain works on this iGPU.** Every link
  is documented; the composition is not tested, and Intel's Y_TILED modifier is
  the documented failure point.
- **That `hwupload` composes through `ffmpeg-next`'s filter wrapper** well enough
  to avoid hand-building `AVHWFramesContext`. Plausible; untested.
- **The fallback bandwidth and latency figures in §2.** Reasoned from frame
  sizes and the write-combining behaviour of mapped VAAPI surfaces. Not measured.
- **`gpu-video`'s capability matrix on this hardware.** Taken from its README;
  the Vulkan Video capability queries were not run against our Mesa version.
- **`shiguredo_vpl`'s codec list** — README claim, not checked against source.
- **`cros-libva`'s API details** — read from the crate tarball, because its
  docs.rs build fails.
- **Whether `glsl-lang` parses `attribute`/`varying`.** It is declared as a GLSL
  4.6 parser, where both are reserved words. Untested.
- **Whether `naga`'s spv-in handles the whole rewritten corpus cleanly**, and how
  many corpus shaders use `sampler2D[N]` arrays, which
  `spirv-webgpu-transform` does not support. Only a conformance run answers both.
- **Whether `spirv-webgpu-transform`'s `CorrectionMap` is complete** — its CLI
  reported one split where two demonstrably occurred, so use the library API and
  verify.
- **Whether `shaderc` or `glslang-rs` find prebuilt libraries on Windows and
  macOS** or force a CMake-and-Python build. This is the main practical risk in
  the §6b plan and it lands on the platforms we are not developing on.
- **`harfrust` and `rustybuzz` performance figures** — taken from project
  READMEs, not benchmarked.
- **`moxcms` accuracy against a reference CMS.** It has the right code points
  and the right function names; I did not compare output against Little-CMS.
- **`kira`'s mock backend as an offline export path.** Documented as "useful for
  testing and benchmarking"; I did not read its `Backend` implementation. Moot,
  since we are not adopting kira.
- **OTIO support in Autodesk Flame**, and OTIO's current ASWF maturity stage.
