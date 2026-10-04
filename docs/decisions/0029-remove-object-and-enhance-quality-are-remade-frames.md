# 0029 — "Remove object" and "Enhance quality" remake a clip's frames into the cache, drawn in place of the decoded ones

Date: 2026-10-04. Status: accepted (agent/ml4).

## What was decided

**A clip's frames can be remade by models and drawn instead of the
decoded frames.** Two features use it, alone or together, in this order:

- **Remove object** paints over an object in every frame with LaMa
  (Suvorov et al., big-lama, Apache-2.0; Carve's fp32 ONNX export, 208 MB).
- **Enhance quality** makes the frame 2x or 4x larger and cleaner with
  Real-ESRGAN general x4v3 (BSD-3-Clause; CoderViking's re-export of the
  official weights, 5 MB), at most 3840 px on the long side, for sources up
  to 1920 px.

- **The setting** is an extras block per feature on the clip
  (`object_removal`, `upscale`), stored and edited like frame blending
  (decision 0026): a remove + insert of the segment, no new `EditCommand`.
  The document records the models, their versions and the mask's
  definition, never pixels.
- **The mask** of a removal is any mix of a selected object (clicks on one
  frame; MobileSAM + VitTrack carry it over the clip — the "Select object"
  mattes of decision 0025's amendment, shared), painted strokes and boxes
  (both in source fractions, the same place in every frame), grown by a
  fraction of the shorter side.
- **The frames** are JPEG (quality 95, 4:4:4), one per source frame, named
  by presentation time like a matte, under
  `~/.cache/chukcut/enhance/<digest>-<ops>-<signature>-<provider>-<long side>/`.
  The signature hashes everything that decides the pixels (models,
  versions, the mask, the growth, the scale, a pipeline revision). Counted
  in the cache limit, deleted with the cache, kept for the open project by
  a trim.
- **The renderer** draws a made frame in place of the decoded one, placed
  by the source's display size, so a 4x frame fills exactly the clip's
  place and every clip feature (grade, masks, matte, effects, blend)
  applies. libjpeg-turbo decodes it at the smallest of full, half, quarter
  or eighth size that still covers the render. A frame not made yet is the
  decoded frame. With frame blending the two made neighbours are blended;
  optical flow falls back to that blend on a remade clip (RIFE's frames
  are made from decoded frames, which still show the object).
- **Baking** runs in the ML worker (protocol 5: `inpaint`, a crop and its
  mask; `upscale`, a frame and the size wanted). A removal walks the
  frames in order from half a second before the first missing one; the
  engine owns the crop, the blend and the state from frame to frame, so a
  restarted worker loses nothing. After any edit the app asks for missing
  frames (`enhance_queue_missing`); an export bakes what it needs first and
  fails in words when it cannot.
- **The CPU is allowed** for both, with the time it will take said before
  the bake (Inspector, CLI `estimate`) and while it runs.

**How a removal is filled**, cheapest first:

1. **Background memory**: in a still shot, a pixel the object covers now
   was seen in an earlier frame; that value is used.
2. **Clean plate**: for a selected (moving) object, a first pass over the
   clip keeps, per still stretch, the last background seen at each pixel,
   so pixels covered at the start of a clip are known from later frames.
3. **LaMa** on a square crop around what is left (as much picture again as
   the object on every side, at least 512 px), stretched to its fixed 512²
   input and back, blended in by a soft edge.
4. **Smoothing**: in a still shot LaMa's fill is mixed half and half with
   the previous frame's, so its texture settles.

"Still" means the picture outside both masks changed by less than 4 code
values on average from the previous frame.

## Why

- **Cache, not document**, for the reasons of 0025 and 0028: pixels are
  derived, the project stays small, a newer model re-makes them without a
  migration.
- **One substitution point for both features and their combination**,
  inside the compositor's quad: transitions, nested compound clips, the
  preview and the export all get it without a second path. Optical flow
  showed the pattern works (0028).
- **LaMa per frame with temporal help, not a video inpainter**: the video
  inpainters with ONNX-able models are not usable under our licence rule
  (`registry.rs`'s licence gate): ProPainter is S-Lab (non-commercial),
  E2FGVI CC BY-NC 4.0. LaMa alone flickers; the memory and the plate
  replace invention by the real background wherever the background is ever
  seen, which on locked-off shots (the common case for removing a passer-by
  or a sign) is nearly everywhere, and the smoothing steadies the rest.
- **Carve's fp32 LaMa over OpenCV Zoo's 93 MB file**: OpenCV's has
  block-quantized weights whose `DequantizeLinear` nodes ORT 1.28's CUDA
  provider refuses, which would have pinned removal to the CPU.
- **realesr-general-x4v3 over RealESRGAN_x4plus**: 5 MB against 64 MB and
  several times faster, trained on the same real-world degradations; a
  video gets a frame a second at 1080p on a 3060 instead of a few seconds.
  2x is the 4x picture reduced by area averaging, what the project's own
  `--outscale` does.
- **Equal tiles** for Real-ESRGAN: the fewest equal parts of at most 768 px
  with 16 px of context, one tensor shape per clip; fixed 512 px tiles read
  70 % more pixels at 1080p.
- **JPEG** for the reason of 0028: a 4K PNG is ~20 MB and slow to decode on
  the render thread; DCT-scaled JPEG decode makes a 4K frame cheap in a
  small preview.

## What it costs

- **Time.** The numbers measured on the RTX 3060 are in `docs/STATUS.md`,
  "Remove object and enhance quality". LaMa is ~160 ms per crop on CUDA
  (fp32, the CUDA provider; TensorRT or fp16 are the open speed-ups),
  ~1.9 s on four CPU threads; Real-ESRGAN ~0.6 s per input megapixel on
  CUDA, ~7 s on the CPU.
- **Disk.** One JPEG per frame at the output size: a 4K frame is 1–3 MB, so
  ten seconds of 4x footage is about a gigabyte of cache.
- **Edges.** A static mask (strokes, boxes) never sees behind itself, so a
  logo is always invented by LaMa; the smoothing keeps it steady in a still
  shot and lets it shimmer in a moving one. A moving camera gets no memory,
  plate or smoothing: per-frame LaMa. Real-ESRGAN is per frame too; its
  compact model flickers little but can sharpen noise into texture.
- **Combinations.** A remade clip with optical flow on blends instead of
  flowing; matte masks and "Remove background" use the decoded frames'
  mattes, which line up because the made frame has the same geometry.

## What would change our minds

- A permissively licensed video inpainter with an ONNX export (a flow-
  guided one): it would replace steps 1–4.
- An fp16 or TensorRT path that makes Real-ESRGAN real-time: the preview
  could enhance live and the cache would become optional.
- Users wanting a mask that moves without a selection (a keyframed box):
  the painted part would gain keyframes, as shape masks have.

## Amendment, 2026-10-04 (agent/polish3): baked on open

The four kinds of derived frames and sound — mattes (0025), optical-flow
frames (0028), remade frames (this decision) and compound mix-downs (0024)
— were asked for after every edit and before an export, never when a
project opened. A project opened after the cache was cleared, or on another
machine, showed its matted clips uncut and its slow motion blended until
the first edit. `modules::prepare` is the open path: after 1.5 s of grace
it reads the cache off the UI thread, counts what every timeline and every
compound clip lacks, and bakes it **one clip at a time**, kind after kind
(sound first, the cheapest), through each module's own job, so an edit
meanwhile joins the running bake instead of starting a second. The app
shows one "Preparing N frames" chip with Stop, and runs the face-landmark
and voice-isolation queues (decision 0030) when the run ends. One at a time rather than all
at once because the bakes share one ML worker and each decodes its own
file: in parallel they would contend for the decoder and the GPU and finish
no sooner, while the preview of the just-opened project waits behind them.
What would change our minds: a second worker per GPU (0025), or a project
whose missing frames take long enough that the user wants the clip under the
playhead first — then order the queue by distance from the playhead.
