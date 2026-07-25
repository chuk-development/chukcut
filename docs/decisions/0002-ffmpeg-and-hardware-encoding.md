# 0002 — Our own FFmpeg build, hardware encoding, and zero system dependencies

Status: decided 2026-07-25. **Step 1 implemented the same day** — VAAPI H.264
and H.265 encode from the export path, on Linux. Steps 2 to 4 are open. What
building step 1 taught us is at the bottom, under "What actually happened",
and it revises one of the numbers above.

## The decision

Three things, and they only make sense together:

1. **We build and ship our own FFmpeg.** Not the distribution's, and not
   Jellyfin's. The user installs nothing.
2. **That build is LGPL-clean** — no x264, no GPL filters — which is what lets
   us keep linking it into our own process.
3. **Encoding is hardware-first**: VAAPI and QSV on Intel, NVENC on NVIDIA,
   VideoToolbox on macOS, with a permissively-licensed software encoder as the
   fallback.

The third point is what makes the second one possible, and that is the part
worth understanding.

## Why not Jellyfin's build

Jellyfin-FFmpeg is a good piece of work: a patched FFmpeg tuned for hardware
transcoding, shipped as a binary package, **bundled with the Intel media
driver**. That last part is its real value, because the encoder does not live in
FFmpeg at all — it lives in the driver (`iHD` on modern Intel). Standard FFmpeg
built with VAAPI does the same job when the driver is present.

So Jellyfin solves "make hardware transcoding work without the user installing
anything", which is exactly our problem. We are not adopting it because we have
to solve the same bundling problem for our own libraries regardless — we link
libav* in-process for decoding — and once we are producing a build, producing
the right build costs no more than producing someone else's.

Building our own also lets us choose the licence, which we cannot do with a
third-party GPL binary.

## The licence, which is the expensive part

Debian and Ubuntu build FFmpeg with `--enable-gpl`, because that is what x264
requires. We currently **link** those libraries into our binary. That makes our
program a derived work, and it would have to be distributed under the GPL.

That is not an abstract concern; it is the kind that becomes a problem precisely
when the project starts making money.

There are two ways out, and the usual one is worse than it looks:

- **Run FFmpeg as a separate process.** A GPL binary invoked over a pipe is
  aggregation, not derivation. This works, and it is what most editors do — but
  it costs us frame-accurate seeking, because a command line cannot express
  "give me the frame at exactly this microsecond, then the one after it, from a
  decoder you keep open". Our whole preview design depends on that.
- **Build FFmpeg without the GPL parts and keep linking it.** libavcodec,
  libavformat, libswscale and libswresample are LGPL. What pulls in the GPL is
  x264, x265, and a handful of filters.

We take the second route. The reason it is available to us at all is that
**hardware encoders are not GPL**: VAAPI, QSV, NVENC and VideoToolbox are
drivers and system frameworks, wrapped by LGPL code in libavcodec. An editor
that encodes on the GPU does not need x264.

For the software fallback — a machine with no usable GPU — the LGPL-compatible
options are OpenH264 (BSD, Cisco), SVT-AV1 (BSD), and libaom (BSD). Slower than
x264, which is an acceptable price on the path nobody takes by choice.

**This has to be verified before shipping**, by someone reading the actual
licence texts of the exact components in the exact build. Nothing above is a
legal opinion. What it is, is the shape of the answer and the reason the shape
matters.

## Why hardware encoding, beyond the licence

Measured on this machine, software export of a 1080×1920 timeline runs at
**22 fps**. Intel QSV on Raptor Lake is expected to reach roughly an order of
magnitude more for H.264 and H.265. An export that takes four minutes instead of
forty is the difference between a tool someone uses and a tool someone avoids.

It is also what every other editor does, and users compare against those.

## What this costs us

The research (`docs/research/rust-crate-survey.md`) established that the path is
shorter than it looks:

- `ffmpeg-sys-next` 6.1 — the version already in our lockfile — **already binds
  `hwcontext.h` and `hwcontext_drm.h`**, and `ffmpeg-next` re-exports them as
  `ffi`. `AVHWFramesContext`, `av_hwframe_*` and `AV_PIX_FMT_DRM_PRIME` are
  callable today. Only a safe wrapper is missing.
- Three permissively licensed reference implementations of the VAAPI→wgpu path
  exist: `AdrianEddy/oxivideo` (MIT/Apache), `jazzfool/ffgpu` (Apache), and
  `ez-ffmpeg`'s `hw_interop.rs`.
- `wgpu-hal` 30.0.0 — our version — has `texture_from_dmabuf_fd`, though it is
  single-plane only and NV12 is not, so a plane-splitting step or a VAAPI VPP
  blit is still ours to write.

Estimated at 6–12 days for the encode path, which is small next to the payoff.

## The order of work

1. A safe wrapper over `AVHWFramesContext`, and a VAAPI H.264/H.265 encoder in
   the export path, selected when the hardware reports itself usable. Software
   stays the default until the hardware path is proven on real files.
2. Zero-copy from the compositor: the composited frame is already a GPU texture,
   so exporting it should not mean reading it back to the CPU and uploading it
   again. This is the DMA-BUF work and is where most of the remaining speed is.
3. Hardware *decode*, which matters more for playback than for export.
4. The bundled build itself: our FFmpeg, the Intel media driver, and whatever
   each platform needs, packaged so that installation is copying one directory.

## What we are explicitly not doing

Shipping a GPL binary and calling it over a pipe. It is the easy answer, it
would work, and it would cost us the decoder control that the preview is built
on.

## What actually happened, 2026-07-25

Step 1 is done. `src-tauri/src/modules/export/hwframes.rs` is the safe wrapper —
it came to about 250 lines including safety comments, which matches the survey's
estimate — and `encoder.rs` takes the VAAPI path when `hwaccel` reports a device
that works. The full numbers are in `docs/STATUS.md`.

Three things the decision above got wrong or left out, recorded here rather than
edited into the text, because a decision record that quietly agrees with itself
afterwards is worth nothing.

**The order-of-magnitude claim was about the wrong thing.** "Intel QSV on Raptor
Lake is expected to reach roughly an order of magnitude more" is true of the
*encoder* — measured 3–5× against x264 medium, and x264 medium is not the
slowest thing we could have compared against — but the export as a whole got
**1.5× faster**, at both aspect ratios. The encoder simply stopped being the
bottleneck. An export that took four minutes now takes two and a half, not
twenty-four seconds.

Nothing in the licence argument depends on that, and the licence argument is the
load-bearing half of this decision. But "an export that takes four minutes
instead of forty" should not be repeated as a promise.

**QSV is not the Intel path on Linux; VAAPI is.** The decision names QSV first
for Intel. On this machine `h264_qsv` is in the FFmpeg build and cannot open a
device at all — no oneVPL runtime — while VAAPI works. QSV on Linux is a layer
over VAAPI in the first place. Keep the QSV branch for Windows and stop treating
it as the Intel default.

**The step ordering should change.** The decision ranks zero-copy second and
hardware *decode* third. The measurements say decode and the CPU-side pixel
shuffling are together most of what remains, so the honest ranking now is:
RGBA→NV12 off the CPU, then hardware decode, then full zero-copy. The reasoning
and the blockers are in `docs/research/zero-copy-encode.md`, which was written
alongside this work.
