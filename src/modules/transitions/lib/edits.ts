/**
 * The four gestures that change a transition, as one function each.
 *
 * Each is one completed gesture and therefore one undo step. Nothing here
 * builds an `EditCommand`: the `transitions_*` commands do that in Rust,
 * because minting an id, knowing the default duration and knowing how short a
 * clip shortens it to are three pieces of policy and policy in the webview is
 * policy in two places.
 */

import { runEdit } from "@/modules/project/store";
import type {
  Id,
  Micros,
  Project,
  TransitionKind,
  TransitionMaterial,
} from "@/modules/project/types";
import {
  transitionsAdd,
  transitionsRemove,
  transitionsRetime,
  transitionsSet,
} from "@/modules/transitions/lib/api";
import { joinAt, overlapToDuration } from "@/modules/transitions/lib/geometry";

/** Put a transition at the head of `segmentId`. */
export function addTransition(
  segmentId: Id,
  kind: TransitionKind = "dissolve",
  duration?: Micros,
): Promise<boolean> {
  return runEdit(() => transitionsAdd(segmentId, kind, duration));
}

export function removeTransition(segmentId: Id): Promise<boolean> {
  return runEdit(() => transitionsRemove(segmentId));
}

/**
 * Set a transition's length.
 *
 * A duration of zero removes it, so that dragging the handle onto the cut is
 * how a transition is undone with the same gesture that made it — and so that
 * the drag never has to decide separately whether it has gone far enough to
 * mean "no transition".
 */
export function retimeTransition(segmentId: Id, duration: Micros): Promise<boolean> {
  if (duration <= 0) return removeTransition(segmentId);
  return runEdit(() => transitionsRetime(segmentId, duration));
}

/** Replace the parameters, keeping the transition where it is. */
export function setTransition(segmentId: Id, transition: TransitionMaterial): Promise<boolean> {
  return runEdit(() => transitionsSet(segmentId, transition));
}

/**
 * What dragging a clip back over its left-hand neighbour should do.
 *
 * The two clips cannot actually overlap — the document forbids it — so the
 * overlap the user drew is read as the length of a transition and neither clip
 * moves. Returns `null` when the drag is not that gesture: no neighbour, a
 * neighbour that does not touch, or a drag that does not go back far enough to
 * be worth anything, in which case the caller should treat it as an ordinary
 * move.
 */
export function overlapGesture(
  project: Project,
  segmentId: Id,
  proposedStart: Micros,
): { segmentId: Id; duration: Micros } | null {
  const join = joinAt(project, segmentId);
  if (!join) return null;
  const duration = overlapToDuration(join.cut, proposedStart, join.max);
  return duration > 0 ? { segmentId, duration } : null;
}

/**
 * Apply the overlap gesture: add a transition, or lengthen the one that is
 * already there.
 */
export function applyOverlap(
  project: Project,
  segmentId: Id,
  proposedStart: Micros,
): Promise<boolean> {
  const gesture = overlapGesture(project, segmentId, proposedStart);
  if (!gesture) return Promise.resolve(false);
  const existing = joinAt(project, segmentId)?.transition;
  return existing
    ? retimeTransition(gesture.segmentId, gesture.duration)
    : addTransition(gesture.segmentId, "dissolve", gesture.duration);
}
