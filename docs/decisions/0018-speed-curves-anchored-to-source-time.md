# 0018 — Speed curves are anchored to source time, and the clip's length is their integral

Date: 2026-10-03. Status: accepted.

## Decision

A speed ramp is a `SpeedCurveMaterial` in `MaterialPool::speed_curves`,
referenced from the clip's `extras`. Its points are **source instants**
(microseconds into the material) with a speed each, from 0.1x to 10x.
Between two points the *slowness* (`1 / speed`) follows a smoothstep; before
the first point and after the last the edge speed holds.

The clip's timeline length is the integral of the slowness over its source
range, rounded to the microsecond (`project::speed::curve_target_duration`).
The source range is the authority; the length follows. While a curve is on,
`Segment::speed` is dormant: kept, so that removing the curve returns the clip
to its old constant speed, and read by nothing.

Every place that turns a timeline instant into a frame of the file, or back,
asks one type: `project::speed::TimeMap` (`materials.time_map(segment)`). At
constant speed it does exactly the arithmetic the code did before.

`EditCommand::SetSpeedCurve` changes the curve **and** the clip's timeline
range in one step, so the document never passes through a state where the
length disagrees with the curve. `speed::edit::set_curve_command` adds the
link partners (one shared curve) and the moves of the clips after it.

## Why

- **Split is exact.** Both halves reference the same curve, and each plays its
  own stretch of it. Nothing is resampled at the cut, so the speed is
  continuous across it and the halves play frame for frame what the whole
  played (`tests/speed_curve.rs`, `a_split_ramp_plays_exactly_like_the_whole_one`).
  A curve over the clip's *normalised* length would need its points resampled
  at every cut, and a smooth curve restricted to part of itself is not the
  same kind of curve.
- **Trim does not move the slow motion.** Trimming a tail drops frames; the
  ramp stays on the moment in the footage it was put on.
- **The integral is closed-form.** Interpolating the slowness by a smoothstep
  makes the time to reach any source instant a polynomial per piece, so the
  length is exact and cheap, and the inverse (which frame plays at a timeline
  instant) is one bracketed Newton solve. The smoothstep is flat at every
  point, so the speed never overshoots the values the user set and never
  reaches zero.
- **A pool material, not a segment field.** Adding a field to `Segment` breaks
  every struct literal in the code base and in every other branch; a pool
  category changes none (the same reason `color_adjusts` gives).

## What it costs

- A curved clip is **muted** (preview and export). The mixer resamples at one
  constant rate per clip and there is no pitch-preserving stretch; a voice
  sliding through octaves inside one ramp is not usable. The Curve tab says so.
- A slip edit of a curved clip changes how long it plays, because a different
  stretch of the curve is under it. The timeline's slip is not curve-aware
  yet; trims are.
- Points outside the clip's current source window (after a trim) are kept but
  not shown in the editor.

## What would change our minds

A pitch-preserving time stretch in the audio engine: then curved clips play
their sound through `TimeMap::speed_at`, and nothing about the model changes.
