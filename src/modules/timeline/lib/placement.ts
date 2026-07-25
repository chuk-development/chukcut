/**
 * Where a clip is allowed to land on a lane.
 *
 * Rust rejects an overlap outright and is right to — the document invariant is
 * that segments on a track never overlap. But a rejection is not a feature. A
 * user shoving one half of a cut back against the other is aiming at a
 * neighbour, not at a microsecond, so the frontend resolves the aim into the
 * nearest legal position before the gesture ends and Rust never sees an edit it
 * has to refuse.
 */

import type { Id, Micros, TimeRange } from "@/modules/project/types";
import { rangeEnd } from "@/modules/project/types";

/**
 * Anything with a place on a lane.
 *
 * A `Segment` satisfies this structurally, so callers pass `track.segments`
 * straight in and tests do not have to build whole segments.
 */
export interface Placed {
  id: Id;
  target_range: TimeRange;
}

/** The lane's other occupants, in time order. */
function occupants(segments: readonly Placed[], excludeId: Id | null): TimeRange[] {
  return segments
    .filter((segment) => segment.id !== excludeId)
    .map((segment) => segment.target_range)
    .sort((a, b) => a.start - b.start);
}

function fits(ranges: readonly TimeRange[], start: Micros, duration: Micros): boolean {
  const end = start + duration;
  return !ranges.some((range) => range.start < end && start < rangeEnd(range));
}

/** Mirrors `Track::is_range_free` in Rust, so the UI can predict the verdict. */
export function isRangeFree(
  segments: readonly Placed[],
  range: TimeRange,
  excludeId: Id | null,
): boolean {
  return fits(occupants(segments, excludeId), range.start, range.duration);
}

/**
 * The closest position to `desiredStart` where a clip of `duration` fits.
 *
 * Every legal answer either butts against material or sits at the head of the
 * timeline, so the search is over exactly those positions: the far side of each
 * clip, the near side minus the dragged length, and zero. Dropping a clip onto
 * the left half of a neighbour pushes it up against that neighbour's leading
 * edge; onto the right half, against its trailing edge. Ties go to the earlier
 * position. There is always room after the last clip, so this never fails.
 */
export function nearestFreeStart(
  segments: readonly Placed[],
  desiredStart: Micros,
  duration: Micros,
  excludeId: Id | null,
): Micros {
  const ranges = occupants(segments, excludeId);
  const wanted = Math.max(0, desiredStart);
  if (fits(ranges, wanted, duration)) return wanted;

  const options: Micros[] = [0];
  for (const range of ranges) {
    options.push(rangeEnd(range));
    options.push(range.start - duration);
  }

  let best: Micros | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const option of options) {
    if (option < 0 || !fits(ranges, option, duration)) continue;
    const distance = Math.abs(option - wanted);
    if (distance < bestDistance) {
      best = option;
      bestDistance = distance;
    }
  }

  if (best !== null) return best;
  return ranges.length > 0 ? rangeEnd(ranges[ranges.length - 1]) : wanted;
}

/**
 * The gap a clip currently sits in: how far its edges can travel before they
 * run into a neighbour.
 *
 * Trimming past a neighbour is rejected by Rust as an overlap, which mid-drag
 * reads as the handle silently doing nothing on release. Clamping to this span
 * turns that into the edge stopping where the material is.
 */
export function freeSpan(
  segments: readonly Placed[],
  excludeId: Id | null,
  range: TimeRange,
): { min: Micros; max: Micros } {
  let min = 0;
  let max = Number.POSITIVE_INFINITY;

  for (const other of occupants(segments, excludeId)) {
    const otherEnd = rangeEnd(other);
    if (otherEnd <= range.start) min = Math.max(min, otherEnd);
    else if (other.start >= rangeEnd(range)) max = Math.min(max, other.start);
  }

  return { min, max };
}
