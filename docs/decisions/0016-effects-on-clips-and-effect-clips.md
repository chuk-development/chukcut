# 0016 — Effects live on clips and on effect clips, and both are compositor layers

Date: 2026-10-03. Status: accepted.

## What was decided

A built-in effect is one `EffectMaterial` in the material pool
(`project/effects.rs`, `MaterialPool::effects`). It reaches the picture in two
ways, and both are supported because the compositor makes the second one cheap
once the first exists:

1. **On a clip.** The effect's id sits in `Segment::extras`, in application
   order — the pattern transitions (and colour, decision 0007) already use.
   The effect sees only that clip.
2. **As an effect clip.** A segment on a `TrackKind::Effect` lane whose
   `material_id` is the effect. For its duration it applies to everything
   composited *beneath* it in the painter's order, CapCut's adjustment layer.
   More effects can stack on it through its own `extras`.

Both render the same way: the effect runs over a full-canvas **layer**.

- An effected clip is drawn into a layer of its own with the transition
  layer pipeline (blending off, so the layer is the clip's straight-alpha
  colour over transparency), the effect passes run over it, and the result is
  composited with straight-alpha source-over where the clip would have been.
- A transition side with effects runs them over its layer before the blend.
- An effect clip ends the composite pass, runs its effects over the target so
  far into a fresh target, and compositing carries on into that one.

Effect passes are our own WGSL (`fx/shaders/fx.wgsl`), in premultiplied linear
`Rgba16Float`, recorded into the compositor's one command encoder on the one
wgpu device. Lengths are fractions of the frame's shorter side, turned into
pixels per render, so the preview and the export look the same.

Document details that follow from the above:

- **`kind` is a string, `params` a sparse map.** An unknown kind from a later
  build loads, renders as nothing and saves unchanged; a parameter added later
  reads its catalog default in every existing file; a value equal to its
  default is not stored.
- **Materials are never edited in place** (decision 0007's rule). An edit
  mints a material and swaps the reference with a `RemoveSegment` +
  `InsertSegment` composite. That is also what makes the two halves of a split
  clip safe to share one material.
- **Keyframes and the animation clock are in the clip's source time.** A split
  keeps one continuous shake or sweep across the cut, and the shared material
  is right for both halves without rewriting anything. An effect clip's source
  starts at 0, so for it source time is time since its head.

## Why

- **Per-clip effects alone** cannot do what short-form editors use most: a
  shake, a flash or a glitch over a whole stretch of the edit, across several
  clips and titles at once. CapCut's effect lane is that, and its users expect
  it.
- **Effect clips alone** cannot frame one clip (rounded corners, border and
  shadow for picture in picture) or blur one clip without blurring the titles
  over it.
- **Supporting both costs one branch in `collect_draws`** and one loop in the
  composite pass. The layer machinery — pooled canvas-sized targets, the
  layer pipeline, uniform slots that are explicit rather than positional —
  already existed for transitions.
- **Layers rather than a pass in `quad.wgsl`.** Most of these effects read
  neighbouring pixels (blur, glow, zoom blur, glitch) or pixels outside the
  clip's quad (a glow into the letterbox, a shadow), which a per-quad fragment
  shader cannot do. Keeping effects out of `quad.wgsl` also leaves the grade's
  order of operations and its byte-identity tests untouched.

## What it costs

- An effected clip costs two canvas-sized `Rgba8UnormSrgb` targets plus one
  `Rgba16Float` texture per pass (two for a blur, four for a glow), all from
  the texture pool, so the steady state allocates nothing. A frame with no
  effect takes exactly the old code path — the effect frame is opened only
  when an item needs it — and an effect at rest or switched off is dropped
  before it costs a layer (a test pins both byte-identical).
- An effect clip ends and restarts the composite pass, and its result becomes
  the frame's target, so the target returned by `render_to_texture` is not
  always the one acquired first. Both come from the same pool key.
- Keyframes in source time mean a clip played at 2x runs its effect animation
  twice as fast. That matches what the footage does and is what the split
  argument needs; a speed-independent clock would need a second time base.

## What would change our minds

- Playback at 4K with several effected clips missing frame time. The fix is
  local to `fx/render.rs`: blur at reduced resolution, or ping-pong two
  textures instead of drawing a fresh one per pass.
- A need for effects that animate over the *timeline* rather than the clip
  (an effect clip spanning a cut already does that; a per-clip one does not).
