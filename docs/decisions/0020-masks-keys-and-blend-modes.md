# 0020 — Masks, chroma key and blend mode share one material; masks and key are quad alpha, blend modes are a layer pass

Date: 2026-10-03. Status: accepted (masks agent).

## What was decided

A clip's shape masks, its chroma key and its blend mode are one
`CompositingMaterial` (`project/compositing.rs`) in the new pool category
`MaterialPool::compositing`. The clip's `extras` names it, like a colour
adjustment (decision 0007). An edit never changes it in place: it makes a new
material and swaps the reference with a `RemoveSegment` + `InsertSegment`
composite (`modules/compositing/edit.rs`). A material that changes nothing (no
mask, no key, normal blend) is not stored. The edit takes the reference off
the clip instead.

The renderer applies the three parts in two places:

- **Masks and the key are per-pixel alpha in `quad.wgsl`.** The key runs on
  the encoded colour of the source, before the grade. The masks run after the
  grade, in the quad's own frame (`in.local`). Both have their own flags
  (`matte_flags`), so a clip without them runs the shader path it ran before.
  Every path that draws a clip draws it through this shader: an ordinary
  draw, a transition side, an effected clip's layer and a blended clip's
  layer. So a masked clip is masked in all of them.
- **A blend mode is a layer and one fullscreen pass.** The compositor draws
  the clip into a layer (its effects run on the layer). Then it ends the
  composite pass and runs `fs_blend` (`fx.wgsl`). This pass reads the frame
  so far and the layer, and writes a new target. Compositing continues in
  that target. An effect clip splits the pass in the same way (decision
  0016). Normal blending, and a mode that this build does not know, use the
  ordinary draw.

Document details:

- Shapes, mask operations and blend modes are strings, read through enums
  with an `Other` variant. A file from a later build opens. An unknown mask
  draws nothing, an unknown blend mode draws as normal, and both save back
  unchanged.
- Mask units are relative to the clip as it is drawn: the centre is a
  fraction of the clip's width and height, and the size and the feather are
  fractions of the clip's shorter side. A circle stays round on any clip. The
  preview and the export agree (only the one-pixel anti-alias width depends on
  the render size).
- Mask keyframes are in the clip's source time, as effect keyframes are
  (decision 0016).
- "Show matte" is a `#[serde(skip)]` flag on the material. The app sets it
  only on the copy of the project that it gives to the preview. It is never
  saved, never undone and never exported.

## Why

- **One material, not three categories.** The three are one panel family and
  one "how this clip meets the frame" question. Three categories would give a
  clip three references to resolve per frame and three mint-and-swap paths
  with the same code.
- **Not a field on `Segment`.** That breaks every `Segment { .. }` literal in
  the tree (decision 0007's reason).
- **Masks and key in the quad shader, not as effects.** They are per-pixel
  and need only the pixel itself (and, for edge shrink, a few source taps). An
  effect needs two canvas-sized layers per clip. A masked clip in the quad
  shader costs nothing extra in memory, and it is masked in a transition and
  in an effect layer without special code.
- **Blend modes as a pass, not a blend state.** Fixed-function blending can
  do multiply, screen, add, darken and lighten. It cannot do overlay, soft
  light, colour dodge or burn, difference or exclusion: these need the colour
  below. A render pass cannot read its own attachment, so the frame so far
  must be a texture. The effect-clip split already does this.
- **Blend formulas on encoded colour.** Users expect Photoshop and CapCut
  results, which are the W3C formulas on gamma-encoded values. The result then
  goes on with the same source-over in linear light as a normal clip. So
  "normal" through `fs_blend` is the ordinary draw.

## What it costs

- The quad uniform grew from 416 to 848 bytes (one 1 KiB slot per draw with a
  256-byte offset alignment).
- A masked clip runs a signed-distance function per mask per pixel. The heart
  is a 32-edge polygon, the star 10 edges. Edge shrink adds 16 source taps,
  each with a key evaluation. Not measured yet.
- A blended clip costs one canvas-sized layer and one fullscreen pass, and it
  splits the composite pass.
- A blended clip in a transition window, or with motion blur, draws as normal
  for that time. The transition and the motion blur use their own layer path.
- Superseded materials stay in the pool for undo. The saved file prunes them
  (`project/prune.rs`).

## What would change our minds

- Text-shaped, brush or pen masks. They need a raster per clip, which means a
  mask texture binding in `quad.wgsl` (or a mask layer). The `MaskShape` enum
  takes new shapes without a schema change.
- A grade that only applies inside a mask (CapCut's Adjust › Mask). That is a
  second coverage function in the grade, not a change to this material.
- Measured frame time over budget with many masked clips. The fix is local:
  bake a mask to a texture once per frame.
