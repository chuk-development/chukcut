# 0010 — Open source under GPL-3.0, and what that settles

Status: decided 2026-08-05. Implemented the same day: `LICENSE`, `NOTICE.md`,
the manifests, and the repository made public.

## The decision

**chukcut is GPL-3.0-or-later, developed in the open, and not sold.**

Three questions were on the table at once — how it is licensed, whether it is
sold, and which FFmpeg build it links. They are one question, because the answer
to the first determines the other two.

## Why copyleft rather than MIT or Apache

A permissive licence would let anyone take this engine, close it, and sell it.
The work that makes chukcut worth taking is exactly the work that is expensive
to redo — the zero-copy VAAPI path, the DMA-BUF import into wgpu, the GLSL ES
rewriter that exists because naga's frontend rejects the entire shader corpus.
Copyleft means that work stays available to whoever improves it next.

The counter-argument for permissive licensing is adoption by companies that
cannot use GPL code. That argument applies to libraries. This is an application:
its users run it, they do not link against it.

## What GPL settles about FFmpeg

Decision [0002](0002-ffmpeg-and-hardware-encoding.md) built its whole shape
around a constraint that no longer exists. It said: we link libav* in-process,
Debian and Ubuntu build FFmpeg with `--enable-gpl` for x264, linking GPL code
into a proprietary program makes that program GPL, therefore we must produce our
own LGPL-clean FFmpeg build without x264 and x265.

**That constraint was a consequence of being proprietary.** A GPL program may
link GPL libraries freely. So:

- Linking the distribution's ordinary `--enable-gpl` FFmpeg is fine.
- `libx264` and `libx265` may be used in-process as software encoders, which
  restores a fallback that 0002 was going to have to replace with OpenH264,
  SVT-AV1 or libaom.
- Step 4 of 0002 — "the bundled build itself" — survives, but for the reason it
  should have had all along: **users should not have to install anything**. It
  is a packaging problem now, not a licence problem, and a Flatpak that takes
  its codecs from the freedesktop runtime extension is a legitimate answer to it.

0002's technical direction is untouched and was right: hardware encoding is
faster by an order of magnitude, and that is why we do it.

## What GPL does not settle: patents

Codec patents are a separate legal system from copyright, and no software
licence disposes of them. H.264 and H.265 are covered by pools — Via LA for AVC,
Access Advance for HEVC — that license implementations independently of what
licence the source code carries.

Our position is the ordinary one for free software, and is the same position VLC,
Kdenlive, HandBrake and FFmpeg itself occupy:

- We distribute **source code**, not licensed codec implementations.
- We sell nothing, so there are no units, no royalties, and nothing to report.
- Anyone distributing binaries built from this source, or using them in a
  jurisdiction that recognises those patents, is responsible for their own
  compliance. `NOTICE.md` says so to the user.

Two things worth recording so nobody re-derives them under pressure:

- The claim that European or French law does not recognise software patents is
  **not reliable**. The EPO grants patents on computer-implemented inventions
  with technical character, and video coding is the textbook case. The reason
  VLC is unbothered is that it sells nothing, not that it is French. Germany, in
  particular, hosts the UPC local divisions where HEVC patents are actually
  enforced — InterDigital obtained injunctions covering eleven EU states against
  Disney in 2026.
- **Under the AVC per-unit terms, the first 100,000 units per year are $0.00.**
  Even a commercial fork of this project would owe nothing until it was
  genuinely large. The exposure was never money; it was paperwork and, at scale,
  an injunction.

## What this costs

Someone can take this, build it, and sell binaries. GPL permits that, provided
they pass on the source. That is the deal, and it is accepted: the aim of this
project is that a good editor exists on Linux, not that this repository is the
only place to get one.

The name and logo are not covered by the code licence.

## What would change our minds

Nothing about the licence. Should the project ever be sold — which is explicitly
not the plan — the GPL does not forbid charging for binaries, and the Ardour
model (open source, paid official builds) would be the route, not relicensing.

## Contributions

Contributions are accepted under GPL-3.0-or-later, by the act of contributing —
no CLA. A CLA exists to let a project relicense later, and this project has
given up the option deliberately. `CONTRIBUTING.md` states it in one line so
nobody has to guess.
