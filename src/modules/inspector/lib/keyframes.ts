/**
 * Reading a segment's animation.
 *
 * `sampleTrack` and `easingApply` mirror `KeyframeTrack::sample` and
 * `Easing::apply` in `src-tauri/src/modules/project/document.rs`, function for
 * function. They are duplicated rather than asked for over IPC because the
 * inspector samples on every playhead move and on every pointer move of a curve
 * drag; a round trip per frame to learn a number the document already contains
 * would be absurd. The duplication is why `keyframes.test.ts` pins the same
 * cases the Rust tests pin — if the two ever disagree, the slider and the
 * rendered frame disagree, which is the worst kind of bug to look at.
 *
 * Every `time` in this file is **relative to the segment start**, never to the
 * timeline. `relativeTime()` is the single place the conversion happens, and it
 * is what makes an animation survive its clip being moved or trimmed.
 */

import { clamp, frameDuration, frameToMicros, microsToFrame } from "@/lib/time";
import type {
  AnimatableProperty,
  Easing,
  Keyframe,
  KeyframeTrack,
  Micros,
  Segment,
} from "@/modules/project/types";

/** Remap a normalized 0..1 progress. Mirrors `Easing::apply`. */
export function easingApply(easing: Easing, progress: number): number {
  const t = clamp(progress, 0, 1);
  switch (easing) {
    case "hold":
      return 0;
    case "linear":
      return t;
    case "ease_in":
      return t * t;
    case "ease_out":
      return t * (2 - t);
    case "ease_in_out":
      return t < 0.5 ? 2 * t * t : -1 + (4 - 2 * t) * t;
  }
}

/** Value of a track at `time`, interpolated between neighbours. Mirrors `KeyframeTrack::sample`. */
export function sampleTrack(track: KeyframeTrack, time: Micros): number | null {
  const keys = track.keyframes;
  if (keys.length === 0) return null;
  const first = keys[0];
  const last = keys[keys.length - 1];
  if (time <= first.time) return first.value;
  if (time >= last.time) return last.value;

  const index = keys.findIndex((k) => k.time > time);
  const a = keys[index - 1];
  const b = keys[index];
  const span = b.time - a.time;
  const t = span <= 0 ? 0 : (time - a.time) / span;
  return a.value + (b.value - a.value) * easingApply(a.easing, t);
}

export function findTrack(segment: Segment, property: AnimatableProperty): KeyframeTrack | null {
  return segment.keyframes.find((track) => track.property === property) ?? null;
}

/** Whether a property carries animation at all. An empty track never exists — Rust drops it. */
export function isAnimated(segment: Segment, property: AnimatableProperty): boolean {
  const track = findTrack(segment, property);
  return track !== null && track.keyframes.length > 0;
}

/**
 * The document value of one animatable property, ignoring animation.
 *
 * The mirror of the match in `animated_transform`
 * (`src-tauri/src/modules/render/layout.rs`): a keyframe track overrides the
 * static value rather than offsetting it, so this is what the property looks
 * like when nothing is animating it — and what the first keyframe takes as its
 * value when animation is switched on.
 */
export function staticValue(segment: Segment, property: AnimatableProperty): number {
  switch (property) {
    case "position_x":
      return segment.transform.position[0];
    case "position_y":
      return segment.transform.position[1];
    case "scale_x":
      return segment.transform.scale[0];
    case "scale_y":
      return segment.transform.scale[1];
    case "rotation":
      return segment.transform.rotation;
    case "opacity":
      return segment.transform.opacity;
    case "volume":
      return segment.volume;
  }
}

/** What the property is actually worth at a segment-relative instant, animation included. */
export function propertyValue(
  segment: Segment,
  property: AnimatableProperty,
  time: Micros,
): number {
  const track = findTrack(segment, property);
  const sampled = track ? sampleTrack(track, time) : null;
  return sampled ?? staticValue(segment, property);
}

/** Timeline instant → segment-relative instant. Negative before the clip, past its duration after. */
export function relativeTime(segment: Segment, timelineTime: Micros): Micros {
  return timelineTime - segment.target_range.start;
}

/** Segment-relative instant → timeline instant, for moving the playhead onto a keyframe. */
export function timelineTime(segment: Segment, relative: Micros): Micros {
  return segment.target_range.start + relative;
}

export function isInsideSegment(segment: Segment, timelineTime: Micros): boolean {
  const relative = relativeTime(segment, timelineTime);
  return relative >= 0 && relative < segment.target_range.duration;
}

/**
 * How close the playhead has to be to count as sitting *on* a keyframe.
 *
 * Half a frame. Keyframe times are snapped to the frame grid when they are
 * created and when they are dragged, so exact equality would work — right up
 * until the playhead lands mid-frame from a click on the ruler, and the diamond
 * that is visibly under the playhead refuses to admit it.
 */
export function keyframeTolerance(fps: number): Micros {
  return Math.floor(frameDuration(fps) / 2);
}

/**
 * Round a segment-relative time onto the project's frame grid.
 *
 * Through the frame number rather than by rounding to a multiple of
 * `frameDuration`: the latter is itself rounded to whole microseconds, so at 30
 * fps it is 33333 µs and a keyframe placed thirty seconds in would land a
 * millisecond early.
 */
export function snapToFrame(time: Micros, fps: number): Micros {
  return frameToMicros(microsToFrame(time, fps), fps);
}

export function keyframeAt(
  track: KeyframeTrack | null,
  time: Micros,
  tolerance: Micros,
): Keyframe | null {
  if (!track) return null;
  let best: Keyframe | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const keyframe of track.keyframes) {
    const distance = Math.abs(keyframe.time - time);
    if (distance <= tolerance && distance < bestDistance) {
      best = keyframe;
      bestDistance = distance;
    }
  }
  return best;
}

/** The last keyframe strictly before `time`, ignoring one the playhead is sitting on. */
export function previousKeyframe(
  track: KeyframeTrack | null,
  time: Micros,
  tolerance: Micros,
): Keyframe | null {
  if (!track) return null;
  let found: Keyframe | null = null;
  for (const keyframe of track.keyframes) {
    if (keyframe.time < time - tolerance) found = keyframe;
  }
  return found;
}

/** The first keyframe strictly after `time`, ignoring one the playhead is sitting on. */
export function nextKeyframe(
  track: KeyframeTrack | null,
  time: Micros,
  tolerance: Micros,
): Keyframe | null {
  if (!track) return null;
  return track.keyframes.find((keyframe) => keyframe.time > time + tolerance) ?? null;
}

/**
 * The easing a keyframe inserted at `time` should adopt.
 *
 * Each keyframe carries the easing used to reach the *next* one, so a keyframe
 * dropped into the middle of an ease-in-out span inherits that span's easing.
 * Adopting the default instead would silently straighten a curve the user
 * shaped, which is not what "add a keyframe here" means.
 */
export function easingForInsertion(track: KeyframeTrack | null, time: Micros): Easing {
  if (!track) return "linear";
  let easing: Easing = "linear";
  for (const keyframe of track.keyframes) {
    if (keyframe.time <= time) easing = keyframe.easing;
  }
  return easing;
}
