# 0012 — Animations are parameters, evaluated per frame, not baked keyframes

Date: 2026-10-03. Status: accepted.

## Decision

A clip's In, Out and Combo animation, its text animator (by letter, word or
line) and its punch-in zoom are stored as **parameters** — preset, duration,
easing, strength, pivot — in one `AnimationMaterial` per animated segment
(`MaterialPool::animations`, referenced from `Segment::extras`). The
compositor evaluates them on every frame against the clip's *current* start
and end (`motion::pose::clip_motion`). Nothing writes a keyframe.

## Why

`docs/research/resolve-plugins.md` §6.3: the plug-ins creators pay for (Magic
Animate, Neo Zoom) let you change the duration after applying a preset, and
CapCut stores its animations the same way. Baked keyframes break the moment a
clip is trimmed: the fade-out stays where the old end was.

With parameters:

- **Trim** needs no code. An Out animation is measured from the end, so it
  moves with the end. When In + Out no longer fit, both shrink in proportion
  (`pose::windows`) instead of overlapping.
- **Split** is one hook in `timeline::ops::split_at`: the left half keeps the
  entrance, the right half takes the exit, both keep the loop and the zoom
  (`motion::edit::split_commands`). Known seams, accepted: a Combo restarts its
  loop at the cut, and a cut inside a zoom ramp jumps to the full zoom.
- **Undo** is exact to the byte: `EditCommand::SetAnimation` carries both
  materials, the pool is kept sorted by id, and a material leaves the pool only
  when no segment refers to it, so a duplicated clip's shared material
  survives the other clip's edit.

Keyframes still compose: the pose is applied on top of the keyframed
transform (offsets add, scales multiply), so a preset animates a clip *to
where the user put it*.

## How each kind reaches the picture

- Transform, opacity: `Pose::apply` on the transform before `place_quad`.
- Wipe: `render::layout::reveal` narrows the quad and its UV rectangle.
- Blur: drawn through the transition pipeline as a `TransitionKind::Blur`
  with an empty outgoing side, so the quad shader (which the colour grade owns)
  is untouched. `Blur` is also a user-facing transition now.
- Text animator: while it runs, the compositor asks `motion::text` instead of
  the source provider. It moves glyph sprites cut from the cached title raster
  (`text::animate::compose`); the background box is a second cached raster
  faded as one piece. At rest the provider's cached title is used, unchanged.

## Costs

- A running text animator composites on the CPU every frame (a full-frame f32
  buffer). Fine for titles in the preview; an export at 1080x1920 pays some
  tens of milliseconds per animated title frame. Restricting the buffer to the
  text's bounding box is the obvious next step.
- No per-glyph blur, and a glyph whose shadow spills further than its grown
  ink rectangle loses the tail of the shadow while it moves.

## What would change our minds

A request for per-property hand editing of an applied preset ("this pop, but
with the third keyframe later"). That is a *convert to keyframes* command,
written once on top of this model — not a reason to store presets as
keyframes.

## Traps found while building it

- An SVG inside an `overflow_hidden` box smaller than itself does not clip
  the way its layout box suggests in GPUI; the wipe tiles draw plain boxes.
- `pkill -f target/debug/chukcut` matches the shell running it; use
  `pgrep -x chukcut`.
- The player's provider was rebuilt only when video and image files changed,
  so a title added after start-up drew as missing media. Titles are now part
  of `player::material_key`.
