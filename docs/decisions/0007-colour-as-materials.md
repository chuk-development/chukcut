# 0007 — Colour adjustments are pool materials, and edits are reference swaps

Date: 2026-07-27. Status: accepted.

## What was decided

Per-clip colour adjustments (brightness, contrast, saturation, temperature)
live in the material pool as a typed category, `ColorAdjustMaterial`,
referenced from `Segment::extras` — the transitions pattern. A committed
change **never mutates a material in place**: it mints a fresh material,
pushes it into the pool directly (inert until referenced), and the undoable
edit is the segment's reference swapping over, spelled as a `Composite` of
`RemoveSegment` + `InsertSegment`. Superseded materials stay in the pool
unreferenced. Opacity is deliberately *not* part of the category — it stays a
`Transform` field, because it scales blend coverage rather than colour and was
keyframable before grading existed; only its slider moved into the colour
panel.

Crop takes the same remove+insert route for its edit, but needs no material:
`Segment::crop` already existed.

## Why

- **Fields on `Segment` were rejected** because a new struct-literal field
  breaks every `Segment { .. }` constructor in the tree — including ones in
  modules owned by concurrent work — for a feature most clips never use. A
  pool category referenced through the untyped `extras` list changes no
  existing segment, constructor or test; that payoff is documented on
  `TransitionMaterial` and it held a second time here.
- **A JSON blob in `MaterialPool::extras` was rejected** because the
  compositor resolves the grade on every frame of every graded clip, and that
  map exists for parameter blocks nothing hot reads.
- **Remove+insert rather than a new `EditCommand` variant** because the enum
  lives in `timeline/ops.rs`, which was owned by other work when this landed —
  the exact situation `text_set` documents, but unlike `text_set` this loses
  nothing: both primitives snapshot the whole segment, so the composite is
  exactly invertible, re-runs every entry check, and is deliberately not
  link-mirrored (cropping a video clip must not touch its linked audio).
- **Immutable materials** are what make colour undo exact without any command
  that mutates the pool: undo swings the reference back to a material that is
  still there. In-place mutation would need a new command variant or would
  silently break undo.

## What it costs

- One superseded material per committed slider release, a few dozen bytes of
  JSON each, never collected. The same class of inert residue as emptied link
  groups, and accepted for the same reason.
- Remove+insert is a heavier wire payload than a dedicated variant (two full
  segment snapshots per crop drag commit).
- The frontend's `MaterialPool.color_adjusts` is optional-typed until other
  modules' fixtures are touched; consumers carry a `?? []`.

## Addendum, same day: LUTs live on the same material

`.cube` LUT support followed within the day and stayed inside this decision
rather than reopening it: `LutRef { path, intensity }` is a field on
`ColorAdjustMaterial`, not a category of its own. Grade and look are one
panel gesture, one uniform block, and one application order (grade first,
look second); a second category would scatter that order across two
materials per segment. The mint-and-swap undo covers the LUT for free.

Two sub-decisions worth naming:

- **Only the path is stored.** The parsed cube is runtime cache
  (`render::lut::LutCache`, keyed path+mtime). A project whose LUT file went
  away opens with a `validate()` warning and renders the clip unadjusted —
  the missing-media rule, applied to looks.
- **The shader interpolates by hand** (eight `textureLoad`s over
  `Rgba32Float`) instead of using hardware trilinear, because hardware
  filtering quantises its weights and an identity LUT must render
  byte-identically to no LUT. The cost is measured in the composite bench's
  `grade+lut` rows, and the whole pass is behind `lut_active`, so clips
  without a look pay nothing.

## Addendum, 2026-10-03: the extended grade is one field on the same material

Tone, presence, effects, HSL, curves and colour wheels went onto
`ColorAdjustMaterial` as one nested field, `grade: Grade`
(`project/grade.rs`), for the reason the LUT did: one gesture family, one
uniform block, one order of operations (`render/grade.rs`). It is
`#[serde(default, skip_serializing_if = "Grade::is_identity")]`, so files
without it open unchanged and files that never use it save unchanged. The
mint-and-swap undo covers it with no new command variant. Curves are stored
canonical (sorted, clamped, identity as the empty list) so "reset" leaves no
residue, the rule the scalars already follow.

## What would change our minds

When `timeline/ops.rs` is free to grow `SetCrop` / `SetColorAdjust` variants,
the builders in `modules/inspector/edit.rs` shrink to constructing those
variants and the wire payload thins out — the document format, the pool
category and the immutable-swap discipline all survive that change unchanged.
If profiling ever showed pool garbage mattering (it will not at these sizes),
a save-time sweep of unreferenced colour materials would be safe *only* if it
also cleared the undo history, which is why it is not done today.
