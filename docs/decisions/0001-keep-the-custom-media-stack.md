# 0001 — Keep the custom media stack

Status: decided 2026-07-25. Full analysis in `docs/research/media-stack-options.md`.

## The decision

Keep building the media layer ourselves. Adopt neither GStreamer/GES nor MLT
wholesale. Borrow narrowly and deliberately.

## Why

The question "should we build on something that already exists" contains a
mistaken premise: that these frameworks solve the parts we have not built. They
solve the parts we have.

What GES and MLT give away is a **timeline model** with clips, ranges and
transitions — which is precisely what `project` + `timeline` already are, tested.
What they do not give away is hardware decode wired into a GPU compositor,
reduced-resolution preview, proxy media, or a render cache. Pitivi builds its own
proxies on top of GES; Kdenlive and Shotcut build their own on top of MLT.

So the trade is: discard ~11,700 lines of working media code to acquire a
timeline we do not need, then rebuild the preview machinery anyway, on an engine
whose internals we no longer control.

Cost of migrating: 95–175 developer-days to arrive back where we are.
Cost of closing our own gaps: 52–98 days, each of which buys a capability.

## The specific blockers, for the record

**GES**: its Rust types are `!Send`/`!Sync`, so a `ges::Timeline` cannot live in
`tauri::State` at all. GPU compositing landed in `main` in April 2026, targets
1.30, and this machine is on 1.24 — until then it composites in software. No free
rotation, no crop, three blend modes. The binding crate had 1,330 downloads in 90
days against 1.19 million for `gstreamer-video` from the same repository.

**MLT**: healthy and well maintained, and its timeline model maps onto ours
almost exactly. But `mlt_position` is a 32-bit frame index and `mlt_frame` is a
CPU byte-plane struct, both structurally incompatible with a microsecond document
and a wgpu compositor. Both reference editors run preview on one render thread.
And the modules we would need — `qtblend` for transform, `qtext` for text — are
GPL, so an LGPL-clean build cannot express per-clip rotation.

## What we do borrow

- **GStreamer as an optional decode backend behind our own trait**, not as the
  engine. This is what Gausian independently arrived at.
- Designs, not code: Olive's `NodeTraverser` emitting *jobs* rather than pixels;
  `lzw5399/video-editor` (MIT) for its preview/export parity test corpus;
  Gausian (Apache-2.0) for its proxy and job tables.
- Cap is **AGPL-3.0** on every crate worth reading. Nothing from it can be
  copied. Its move from hand-written WGSL to Skia is a datapoint worth weighing
  against our Phase 3 plan.

## The pattern worth remembering

Seven dead Rust video editors were examined. None died of Rust, wgpu or FFmpeg.
All died of one person plus growing scope — Olive's author said so himself. Four
never had a working timeline model. We have one.
