/**
 * Where a point on screen lands on the timeline.
 *
 * Desktop file drops arrive as a window-level event with a position, not as a
 * DOM event on the element under the cursor, so something has to turn a
 * viewport coordinate into "this lane, at this instant". Only the timeline
 * knows its own geometry, so it registers the answer here and the shell asks.
 *
 * A module-level registry rather than context because the caller is an event
 * handler outside React's tree, and there is exactly one timeline.
 */

import type { Id, Micros } from "@/modules/project/types";

export interface TimelineDropPoint {
  trackId: Id | null;
  at: Micros;
}

type Resolver = (clientX: number, clientY: number) => TimelineDropPoint | null;

let resolver: Resolver | null = null;

/** Called by the timeline on mount. Returns the teardown. */
export function registerTimelineDropTarget(next: Resolver): () => void {
  resolver = next;
  return () => {
    if (resolver === next) resolver = null;
  };
}

/** Null when the point is not over the timeline at all. */
export function resolveTimelineDrop(clientX: number, clientY: number): TimelineDropPoint | null {
  return resolver?.(clientX, clientY) ?? null;
}
