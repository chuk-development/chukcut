# 0028 — Optical-flow frames are baked cache drawn in place of the blend; a clip's matte steers its own grade and effects

Date: 2026-10-04. Status: accepted (agent/ml3).

## What was decided

**"Optical flow (AI)" is the third frame-blending mode, and its frames are
cache.** A slowed clip (a constant speed below 1x or a speed curve) with
the mode on shows, at an instant between source frames `k` and `k + 1`, a
frame RIFE made for that pair and phase, instead of the mix the plain
"Frame blend" draws (decision 0026).

- **The setting** is the frame-blend `extras` block with `mode: "flow"`
  (`speed::blend::FrameBlend::Flow`). No new `EditCommand`, no new field.
  "Smooth slow-mo" is one `Composite` of the speed edit and the blend edit.
- **The model** is RIFE v4 (Practical-RIFE, MIT, weights included), the ONNX
  export `walterlow/RIFE_fp32_timestep` (MIT), whose timestep is an input,
  so one session makes any phase. Pinned by Hugging Face commit and SHA-256
  in the worker's registry, 22 MB, downloaded on first use.
- **The frames** are JPEG (quality 95, 4:4:4) under
  `~/.cache/chukcut/flow/<media digest>-rife-<version>-<provider>-<long side>/`,
  one file per `(source frame, phase in 64ths)`. The provider is in the key
  for the reason it is in the matte key: CUDA and CPU frames differ in the
  last bits, and a clip draws from one directory, never a mix. They count
  towards the cache limit, are deleted with the cache, and the open
  project's are kept by a trim.
- **The renderer** draws the baked frame in place of the decoded one, after
  the decoded frame placed the clip, so the grade, masks, matte, effects and
  blend mode apply as they do to any frame. A frame not baked yet is drawn
  as the plain blend: the preview never waits and never shows a hole.
- **Baking** runs in the ML worker (protocol 4: `interpolate`, two frames in
  and one frame per phase out). Each pair of source frames is decoded once
  and asked for all its phases in one request. After any edit the app asks
  for missing frames (`speed_flow_queue_missing`) and bakes them in the
  background; an export bakes the frames its own frame grid lands on before
  the first frame renders, and fails in words when they cannot be made.
- **Size.** Frames are made at the source's size, at most 1920 on the long
  side: a 4K clip's new frames are made at 1080p and drawn scaled.
- **The CPU** is allowed (RIFE fits in memory there), with a warning that
  says how many frames are left and how long they will take at the pace so
  far, and that a GPU bundle is 10–30 times faster.

**A clip's matte is a mask for its own grade and effects.** The matte that
"Remove background" bakes (people, objects, a selected object) can, without
cutting anything, limit the clip's grade (Adjust tab) or its effects to the
subject or to the rest: "grade only the person", "blur only the
background", on one clip, no copy on the lane above.

- **The setting** is three fields on the clip's `BackgroundRemoval`:
  `cut` (default true: what old files mean), `grade` and `effects`
  (`whole`, `subject` or `background`). Choosing Subject or Background on a
  clip without a matte gives it the people matte, uncut. Turning Remove
  background off keeps the matte while a grade or effects still use it.
- **The grade** is mixed in `quad.wgsl`, in light, between the ungraded and
  the graded colour by the matte (`M_GRADE_SUBJECT`, `M_GRADE_BACKGROUND`).
- **The effects** run over the whole clip layer as always; then a pass
  (`render::matte_mix`) mixes the layer before and after them by the matte,
  drawn into its own layer over the clip's quad (`M_MATTE_OUT`), in
  premultiplied colour. Background weight is `1 − matte` inside the clip and
  1 outside it, so an effect's spill past the clip's edge stays.
- A frame whose matte is not baked yet shows the grade and the effects on
  the whole clip, as a frame without a matte is drawn uncut.

## Why

- **Cache, not document**, for the same reasons as the mattes (0025):
  pixels are derived data, a project stays small, and a newer model can
  re-make them without a migration. The document records only the mode.
- **Drawn in place of the decoded frame**, not as a pre-pass that warps the
  decoded frames on the GPU: RIFE is a network, not a shader, and it runs in
  the worker process, which must not open the editor's Vulkan device. The
  substitution keeps every clip feature without a second path (decision
  0026 named the pre-pass as what would change its mind; the bake made the
  pre-pass unnecessary).
- **Phases in 64ths:** a constant 0.25x lands on 16, 32 and 48 exactly, a
  24 fps clip on a 30 fps timeline lands within 1/128 of a frame, and a
  speed curve's arbitrary phases land on a grid a re-render finds again. A
  render whose time rounds to the neighbouring 64th takes that frame.
- **RIFE over FILM and IFRNet:** MIT, 22 MB, one network run per frame,
  and the de-facto standard for this job, with ONNX exports that take the
  phase as an input. FILM (Apache-2.0) is a several times larger network
  (`docs/research/ml-features.md` §3.11 estimates 2–5 fps at 1080p on this
  card); IFRNet (MIT) would have needed an ONNX export of our own.
  Two RIFE exports were checked on a moving square (`walterlow`'s and
  `yuvraj108c`'s RIFE 4.9): both put the square within one pixel of its true
  position at t = 0.25 and 0.5; the second's repository states no licence.
- **JPEG over PNG:** a 1080p PNG is ~3 MB and 25 ms to decode on the render
  thread; JPEG 4:4:4 at quality 95 is 0.5–1 MB and a few milliseconds, and
  the new frame sits between two lossy decoded ones.
- **The matte as a mask on the same clip** rather than "duplicate the clip
  and cut the copy" (which 0025's amendment recommended as a workaround):
  one clip to trim and move, one decode, and an effect such as a background
  blur sees the whole picture, so no dark halo is left where a cut-out
  subject would have been.
- **Mixing after the effects, by a matte drawn into the frame**, because
  effects run in frame space on the clip's layer (shake, glow, blur spread
  beyond the quad) while the matte lives in source space; drawing it with
  the clip's own placement puts it exactly where the clip's pixels are.

## What it costs

- **Time.** RIFE on the RTX 3060 (CUDA, fp32, release worker) is the
  numbers in `docs/STATUS.md`, "AI slow motion and matte masks"; a 10 s
  clip at 0.25x needs 900 new frames. On the CPU a 720p frame takes seconds.
- **Disk.** 0.3–1 MB per frame at 1080p; 900 frames are about 0.5 GB.
- **Edges.** The frame between two decodes is made from the software
  decoder's RGB; a clip that the preview decodes on NVDEC may differ from
  it by a code value. RIFE smears at occlusions, fast rotation and
  particles; scene cuts inside the clip interpolate across the cut.
- **Compound clips.** Their inside is baked on the preview's frame grid
  only; an outer speed change on the compound clip falls back to the blend.
  The nested-render cache keeps a blended frame until its fingerprint
  changes.
- **Matte masks** cost one more layer and one full-frame pass per effected
  clip, and the grade samples the matte once more per pixel. Not inside a
  transition window (the transition sides keep whole-clip effects) nor
  while a blur animation runs.

## What would change our minds

- A runtime on the editor's own device (burn or WebGPU in ONNX Runtime)
  fast enough to make the frame live: then the bake becomes a cache of a
  live path rather than the only path.
- A TensorRT or fp16 path that makes 1080p real-time on mid-range GPUs:
  the preview could interpolate while it plays.
- Users asking for a separate mask per effect: the target would move from
  the clip's matte setting onto each effect.

## Amendment, 2026-10-04 (agent/polish3)

Missing frames are also baked when a project opens, not only after an edit
or before an export: `modules::prepare`, decision 0029's amendment.
