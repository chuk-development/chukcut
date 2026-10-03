# 0014 — Motion tracks are pool materials, and overlays follow them by link

**Decided:** 2026-10-03, while building motion tracking (T1 of
`docs/research/ml-features.md` §2).

## What was decided

- A tracking result is a **`TrackingMaterial`** in `MaterialPool::trackings`:
  one sample per analysed frame, in the tracked file's **source time**, with
  the box as fractions of the displayed source frame, a rotation, a confidence
  and flags (`ANCHOR`, `LOST`). Smoothing is a setting read at evaluation, not
  baked into the samples.
- An overlay follows a track through a **`FollowMaterial`** in
  `MaterialPool::follows`, named in the overlay's `Segment::extras` exactly
  like a colour adjustment. It names the track, the video clip the track is
  seen through, the mode (position / + scale / + rotation), an offset and a
  reference source time.
- The compositor resolves the link for every segment it draws
  (`tracking::follow::resolve`): timeline time → the target clip's source time
  → the pose → through the clip's crop and transform onto the canvas. Preview
  and export share that one function.
- Tracking edits are a **third command kind** on the one undo stack,
  `tracking::TrackingCommand`, beside `EditCommand` and `ConfigureCommand`
  (decision 0008 named this as its extension point). It sets or clears one
  tracking or follow material, wraps an `EditCommand` for the segment half of
  an edit, and composes, so "track, then attach" and "bake to keyframes" are
  each one undo step.

## Why

- **Link, not keyframes.** Keyframes baked onto the overlay are relative to
  the overlay and break the moment the video is trimmed, slipped, retimed,
  moved or scaled. A link evaluated through the video clip survives all of
  those, a re-track updates every follower, and one track can drive several
  overlays. "Bake to keyframes" is still there for hand-editing the motion.
- **Pool material referenced from `extras`, not a field on `Segment`.** There
  are well over a hundred `Segment { … }` literals across the tree and several
  agents editing them; a new field would have touched all of them. The
  `extras` route changes no constructor (the colour adjustment set this
  precedent).
- **Samples in the document, not the cache.** A track defines the edit: a
  re-run on another machine or tracker version gives slightly different
  numbers, and the text would move. A minute at 30 fps is ~1 800 samples; the
  field names are one letter to keep the pretty-printed file reasonable.
- **A third command kind, not `EditCommand` variants.** `timeline/ops.rs` is
  owned by concurrent timeline work; see 0008 for why an enum variant is the
  edit that conflicts.

## Costs

- A follower whose named target clip no longer covers the instant falls back
  to the topmost visible clip of the **same file** that does (a split, a
  copy), and failing that holds the named clip's nearest edge. A follower
  whose track or every target is gone draws with its own transform — no
  validation warning yet.
- The follower's own position is "where it sits at the reference frame"; the
  follow adds the object's motion since then. Re-attaching a clip that
  already follows starts from its stored transform, which can jump.
- The object's scale and rotation are relative to the track; the target
  clip's own animated scale and rotation are not added to the follower's.

## What would change our minds

- A tracker that needs per-frame state too large for the document (SAM
  memory features): that goes in the cache, keyed as §2.5 of the research
  says, and the document keeps only the samples.
- The timeline module growing material-level commands of its own: fold
  `TrackingCommand` into it as 0008 describes for `ConfigureCommand`.
