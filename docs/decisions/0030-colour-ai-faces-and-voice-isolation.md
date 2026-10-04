# 0030 — Colour tools write ordinary grade controls; faces are a cached landmark track; voice isolation is a rendered cache before the denoise

Date: 2026-10-04. Status: accepted (agent/colourai).

## What was decided

**Auto adjust and colour match choose values for the existing grade
controls; they add no filter of their own.** Both read a few frames of the
clip (eight spread over it, or the one at a given instant, 180 px tall),
fit on the CPU and write the answer through `inspector_set_grade` as one
undo step (`modules::grading`).

- *Auto adjust* owns exposure (towards an 18 % log-average, 70 % of the
  way, never clipping the brightest 2 %), temperature and tint (Newton
  steps against the mean a*/b* of the most neutral third of the pixels,
  85 % of the cast removed, half as much and at most ±0.4 when that third
  is still clearly coloured), whites and blacks (a levels stretch of the
  0.5 % and 99.5 % luma, never a compression), and a little vibrance on a
  dull picture. The white balance runs again after the levels, which
  scale whatever cast is left. Everything else the user set stays and is
  part of what is measured.
- *Colour match* owns exposure, contrast, saturation, temperature and tint
  (damped Gauss–Newton on the L*a*b* means and spreads of the clip against
  the reference *as graded*) and the red, green and blue curves (histogram
  specification at seven quantiles, when the clip has no LUT and no fade
  after the curves, and only if it brings the statistics closer).
- *The fit runs on the shader's CPU twin.* `render::grade::EncodedStages`
  is the reference the GPU tests compare against, prepared once per grade
  (the curve table baked once, not per pixel), with the linear-light
  exposure in front. What converges on the CPU converges in the picture:
  a match measured on the compositor's render landed within 3 L* units of
  the fit's prediction.
- *Grade presets* are JSON files of a whole `GradeEdit` in
  `$XDG_DATA_HOME/chukcut/grade-presets/`, listed in the Filters tab as
  "My presets" with a tile the compositor draws.

**Face landmarks are cache; a followed face is an ordinary motion track.**
MediaPipe's face mesh (478 points, Apache-2.0, 4.9 MB) runs in the ML
worker behind YuNet, which runs only on the first frame and after every
face is lost: each frame's points give the next frame's region, as
MediaPipe's own tracking loop does (protocol 6, `face_landmarks`).

- *The track* is one binary file per media file and model version under
  `~/.cache/chukcut/landmarks/` (quantised to 1/65535 of the frame, 1.9 kB
  per face per frame), written by a background job in `analysis::jobs`
  and re-read by the compositor when it changes, so the preview retouches
  frames as they are found.
- *Retouch* is an effect (`fx::catalog::RETOUCH`, category Face) with five
  sliders and four presets. The compositor gives the instance the faces of
  the frame (23 named points in the clip's unit quad); `fx::retouch` turns
  them into up to three passes per face: a jaw slim (a smooth local
  translation), an edge-preserving skin average limited to the face
  ellipse, a skin-colour test and away from the eyes, brows and lips, and
  a lift of the eyes and of light, unsaturated pixels inside the lips
  (teeth). Without landmarks the effect draws nothing; an export finds
  them first.
- *Follow a face* writes the face's pose per frame (an anchor point —
  face, eyes, forehead, nose, mouth or chin —, the face's size and the eye
  line's angle) as a `TrackingMaterial` stamped `face`, and attaches the
  overlay with the ordinary follow link, one undo step. Modes, smoothing,
  detach and bake to keyframes work unchanged.

**Voice isolation is rendered into the cache, before the denoise.**
HTDemucs fine-tuned (the vocals specialist, MIT, 316 MB, ONNX with the
STFT inside the graph) runs in the worker on fixed 7.8 s segments
(protocol 6, `separate`); the engine overlaps them by a quarter and
cross-fades linearly. Two cache files: the voice stem (per file and model,
the slow part) and the mix the clip plays (stem and original mixed by the
strength, "keep voice" or "keep background"; quick). The setting is part
of the clip's voice cleanup block and takes effect at once; the clip plays
as it was until the mix exists; the denoise reads what isolation kept; an
export renders what is missing first.

## Why

- **Ordinary controls, not a filter**: `docs/research/ml-features.md`
  §3.14 — the user sees what the tool did and can move any of it, and the
  grade keeps one shader path, tested once.
- **No model for colour**: robust percentiles, a neutral-pixel cast and an
  L*a*b* transfer are what a colourist reads off the scopes. A network
  would add a download for little.
- **Landmarks as cache**: 478 points per face per frame is megabytes per
  minute; decision 0019's line between derived data (cache) and edit data
  (document). The face track a follower uses *is* edit data, so it goes
  into the document as a motion track, where the follow machinery is.
- **No face-parsing model** for the skin mask: the common BiSeNet weights
  were trained on CelebAMask-HQ (non-commercial); the mesh already says
  where the eyes, brows and lips are (§3.15).
- **HTDemucs over the alternatives** (`docs/research/ml-features.md` §3.9):
  MIT code and weights, waveform in and out (no STFT of ours to get exactly
  right), good on speech under music. Mel-Band RoFormer is better but
  953 MB and about ten times slower; UVR's MDX-Net weights have a licence
  that is a README sentence; DeepFilterNet and DPDFNet remove noise, not
  music.

## What it costs

- Colour match is statistics, not semantics: a reference with a red wall
  pulls a clip without one towards red. "Frame at playhead" narrows the
  reference to one frame.
- Auto adjust's white balance is grey-world on the most neutral pixels:
  a coloured backdrop that a cast happens to turn grey is taken for grey,
  and the cast stays. The user moves temperature by hand.
- Landmarks need the face model (5 MB) and YuNet, whose training range is
  faces of about 10–300 px at the 1280 px analysis size. Retouch draws
  nothing on a frame without landmarks, so a clip whose face leaves the
  frame shows unretouched frames there — correctly.
- The warp and the average are per frame; the landmarks' frame-to-frame
  jitter (a pixel or two) is below what the passes show, and face tracks
  for followers get 30 % smoothing by default.
- HTDemucs is 316 MB on disk and the worker's resident memory peaked at
  6.8 GB on CUDA and 8.0 GB on the CPU while it ran (measured, 60 s clip).
  On the CPU it took 2.6 s per 7.8 s segment (36 s for a minute); on the
  RTX 3060 0.23 s per segment (11 s for a minute, start-up included).
  Isolation changes loudness; a normalisation set before it keeps its
  gain until normalised again.

## What would change our minds

- A grading model (learned auto colour) that users clearly prefer to the
  statistics — it would still write ordinary controls.
- Real-time landmarks in the preview (face mesh is 6 ms per face on the
  GPU): the cache could become a cache of a live path.
- A permissively licensed speech-separation model that is smaller and
  better on speech than HTDemucs.
