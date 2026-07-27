/**
 * Ripple edits: removing time from a lane, not only material.
 *
 * A ripple delete takes a clip out and pulls everything later **on the same
 * track** left by its duration; closing a gap is the same pull without a
 * removal. Both are one batch through `timeline_apply_many`, which is one undo
 * step — and it is Rust's `compose_edits` that expands each move onto its link
 * partners, so a rippled video clip brings its sound along without this side
 * naming it.
 *
 * The order of the parts is load-bearing: the removal first, then the moves
 * left to right, so every destination is vacated before something arrives.
 * `compose_edits` deliberately leaves a mixed batch in the caller's order —
 * this is the caller it leaves it for. If any part still cannot land (a
 * mirrored move onto an occupied audio lane), Rust rolls the whole batch back
 * and the timeline is exactly as it was, which is how "a ripple never creates
 * an overlap" is kept: it is refused, not repaired.
 */

import { runEdit } from "@/modules/project/store";
import type { Id, Micros, Project, Track } from "@/modules/project/types";
import { findSegment, rangeEnd } from "@/modules/project/types";
import { type EditCommand, timelineApplyMany } from "@/modules/timeline/lib/api";

/** The leftward move of every clip on `track` starting at or after `from`. */
function pullLeft(track: Track, from: Micros, by: Micros, except?: Id): EditCommand[] {
  return track.segments
    .filter((segment) => segment.id !== except && segment.target_range.start >= from)
    .sort((a, b) => a.target_range.start - b.target_range.start)
    .map((segment) => ({
      type: "move_segment",
      segment_id: segment.id,
      from_track: track.id,
      to_track: track.id,
      from_start: segment.target_range.start,
      to_start: segment.target_range.start - by,
    }));
}

/**
 * The batch that removes a clip and closes the hole it leaves.
 *
 * Later clips move by the clip's whole duration — the gap that closes is the
 * one the removal makes, plus nothing: a gap that already sat between the clip
 * and its neighbour was put there on purpose and survives.
 */
export function rippleDeleteCommands(project: Project, segmentId: Id): EditCommand[] {
  const found = findSegment(project, segmentId);
  if (!found) return [];
  const { track, segment, index } = found;
  return [
    { type: "remove_segment", track_id: track.id, segment, index },
    ...pullLeft(track, rangeEnd(segment.target_range), segment.target_range.duration, segment.id),
  ];
}

export function rippleDeleteSegment(project: Project, segmentId: Id): Promise<boolean> {
  const commands = rippleDeleteCommands(project, segmentId);
  if (commands.length === 0) return Promise.resolve(false);
  return runEdit(() => timelineApplyMany(commands, "Ripple delete"));
}

/** A stretch of empty lane between two clips. */
export interface Gap {
  start: Micros;
  end: Micros;
}

/**
 * The gap under `at` on a lane, if there is one.
 *
 * Only a *bounded* gap counts: the empty run after the last clip has nothing
 * later to pull, so closing it would be a no-op offered as a command. The run
 * before the first clip is a gap — closing it snaps the lane to zero.
 */
export function gapAt(track: Track, at: Micros): Gap | null {
  let previousEnd: Micros = 0;
  const sorted = [...track.segments].sort((a, b) => a.target_range.start - b.target_range.start);
  for (const segment of sorted) {
    const { start } = segment.target_range;
    if (at < start) {
      return at >= previousEnd && start > previousEnd ? { start: previousEnd, end: start } : null;
    }
    previousEnd = Math.max(previousEnd, rangeEnd(segment.target_range));
    if (at < previousEnd) return null;
  }
  return null;
}

/** The batch that closes the gap under `at`: everything later, pulled left by its width. */
export function closeGapCommands(project: Project, trackId: Id, at: Micros): EditCommand[] {
  const track = project.tracks.find((candidate) => candidate.id === trackId);
  if (!track || track.locked) return [];
  const gap = gapAt(track, at);
  if (!gap) return [];
  return pullLeft(track, gap.end, gap.end - gap.start);
}

export function closeGap(project: Project, trackId: Id, at: Micros): Promise<boolean> {
  const commands = closeGapCommands(project, trackId, at);
  if (commands.length === 0) return Promise.resolve(false);
  return runEdit(() => timelineApplyMany(commands, "Close gap"));
}

/** Whether "Ripple delete" has anything to ripple — used to word the menu honestly. */
export function segmentsAfter(project: Project, segmentId: Id): number {
  const found = findSegment(project, segmentId);
  if (!found) return 0;
  return found.track.segments.filter(
    (segment) =>
      segment.id !== segmentId &&
      segment.target_range.start >= rangeEnd(found.segment.target_range),
  ).length;
}
