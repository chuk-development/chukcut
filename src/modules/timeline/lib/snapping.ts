/**
 * Snapping.
 *
 * This is deliberately a frontend concern (see
 * `docs/architecture/timeline-editing.md`): computing it in Rust would mean a
 * round trip per mouse-move. The drag stays local and smooth, and Rust receives
 * the already-snapped value and validates it like any other edit.
 */

import { MICROS_PER_SECOND } from "@/lib/time";
import type { Id, Micros, Project } from "@/modules/project/types";
import { rangeEnd } from "@/modules/project/types";

/**
 * Snap radius, in pixels.
 *
 * Pixels and not microseconds because the gesture is made with a pointer: a
 * fixed radius in time would be an invisible hair at frame zoom and half the
 * screen with the whole project in view. `snapRadius` converts it at the
 * current scale, so the pull feels the same wherever the zoom slider is.
 */
export const SNAP_RADIUS_PX = 9;

export type SnapKind = "clip" | "playhead" | "marker" | "origin" | "second";

/**
 * Tie-break order, lowest first.
 *
 * Two candidates the same distance away are not equally interesting: docking to
 * material is the edit that was meant, and a grid line that happens to sit at
 * the same place is a coincidence.
 */
const PRIORITY: Record<SnapKind, number> = {
  clip: 0,
  playhead: 1,
  marker: 1,
  origin: 2,
  second: 3,
};

export interface SnapCandidate {
  time: Micros;
  kind: SnapKind;
}

export interface SnapContext {
  /** Clip edges on every track, the playhead, markers and zero. */
  candidates: SnapCandidate[];
  /**
   * Whole seconds are derived per edge rather than enumerated: an hour of
   * timeline has 3600 of them and only the one nearest the edge can ever win.
   */
  secondGrid: boolean;
}

export interface SnapHit {
  /** Where the gesture's anchor ends up — a range's start, or the instant. */
  value: Micros;
  /** The candidate that was docked to. This is where the guide line is drawn. */
  at: Micros;
  kind: SnapKind;
  /** Which edge of the moving range docked. Always `start` for an instant. */
  edge: "start" | "end";
}

/**
 * Everything a gesture can dock to.
 *
 * `excludeSegmentId` is the clip being dragged: its own edges travel with it,
 * so they would only ever snap it to where it already is. `playhead` is null
 * when the playhead itself is what moves.
 */
export function buildSnapContext(
  project: Project | null,
  playhead: Micros | null,
  excludeSegmentId: Id | null,
  markers: readonly Micros[] = [],
): SnapContext {
  const candidates: SnapCandidate[] = [{ time: 0, kind: "origin" }];
  if (playhead !== null) candidates.push({ time: playhead, kind: "playhead" });
  for (const marker of markers) candidates.push({ time: marker, kind: "marker" });

  if (project) {
    for (const track of project.tracks) {
      for (const segment of track.segments) {
        if (segment.id === excludeSegmentId) continue;
        candidates.push({ time: segment.target_range.start, kind: "clip" });
        candidates.push({ time: rangeEnd(segment.target_range), kind: "clip" });
      }
    }
  }

  return { candidates, secondGrid: true };
}

interface MovingEdge {
  edge: "start" | "end";
  /** How far this edge sits from the gesture's anchor. */
  offset: Micros;
}

const INSTANT: readonly MovingEdge[] = [{ edge: "start", offset: 0 }];

/** Nearest wins; equally near, the more meaningful kind wins; then the leading edge. */
function beats(best: SnapHit | null, bestDelta: Micros, delta: Micros, kind: SnapKind): boolean {
  if (best === null) return true;
  if (delta !== bestDelta) return delta < bestDelta;
  return PRIORITY[kind] < PRIORITY[best.kind];
}

function bestHit(
  anchor: Micros,
  edges: readonly MovingEdge[],
  context: SnapContext,
  radius: Micros,
): SnapHit | null {
  let best: SnapHit | null = null;
  let bestDelta = Number.POSITIVE_INFINITY;

  for (const edge of edges) {
    const position = anchor + edge.offset;

    for (const candidate of context.candidates) {
      const delta = Math.abs(candidate.time - position);
      if (delta > radius || !beats(best, bestDelta, delta, candidate.kind)) continue;
      best = {
        value: candidate.time - edge.offset,
        at: candidate.time,
        kind: candidate.kind,
        edge: edge.edge,
      };
      bestDelta = delta;
    }

    if (context.secondGrid) {
      const second = Math.round(position / MICROS_PER_SECOND) * MICROS_PER_SECOND;
      const delta = Math.abs(second - position);
      if (delta <= radius && beats(best, bestDelta, delta, "second")) {
        best = { value: second - edge.offset, at: second, kind: "second", edge: edge.edge };
        bestDelta = delta;
      }
    }
  }

  return best;
}

/**
 * Snap a single instant — the playhead, or the edge being trimmed.
 *
 * Null when nothing is in range, which is what lets a caller tell "nothing to
 * snap to" apart from "snapped to where it already was". Reporting the latter
 * as a zero-distance snap is what used to stop a clip's trailing edge from ever
 * winning a drag.
 */
export function snapInstant(time: Micros, context: SnapContext, radius: Micros): SnapHit | null {
  return bestHit(time, INSTANT, context, radius);
}

/**
 * Snap a whole clip: both edges compete and the smaller correction wins.
 *
 * Considering only the leading edge is what makes a clip refuse to butt up
 * against the clip on its right — its start is a whole clip length away from
 * anything worth docking to.
 */
export function snapRange(
  start: Micros,
  duration: Micros,
  context: SnapContext,
  radius: Micros,
): SnapHit | null {
  return bestHit(
    start,
    [
      { edge: "start", offset: 0 },
      { edge: "end", offset: duration },
    ],
    context,
    radius,
  );
}

/** The snap radius in microseconds at the current zoom, which is pixels per microsecond. */
export function snapRadius(zoom: number): Micros {
  return SNAP_RADIUS_PX / zoom;
}
