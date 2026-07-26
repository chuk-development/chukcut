/**
 * Where a transition sits, in time and in pixels.
 *
 * The arithmetic is the mirror of `src-tauri/src/modules/transitions/resolve.rs`
 * and it is duplicated on purpose: the renderer must not ask the webview where
 * to draw, and the webview must not ask Rust where to put a handle it is
 * dragging at sixty frames a second. What matters is that the two agree, so
 * every rule below names the function it mirrors and the tests use the same
 * numbers the Rust tests do.
 *
 * The one rule to hold on to: **a transition is centred on the cut and neither
 * clip moves.** Its start is not stored anywhere — it is derived from the
 * incoming clip's start and the duration — so nothing here may cache it.
 */

import type {
  Id,
  Micros,
  Project,
  Segment,
  TimeRange,
  Track,
  TransitionMaterial,
} from "@/modules/project/types";

/** Every transition in the pool, or an empty list on a document without any. */
export function transitionPool(project: Project): TransitionMaterial[] {
  return project.materials.transitions ?? [];
}

/**
 * The transition a segment is entered by, if it has one.
 *
 * `extras` is a bare list of material ids with no type tag — the kind of an id
 * is whichever pool category it resolves in — so this is a lookup rather than a
 * filter on a field.
 */
export function transitionOf(project: Project, segment: Segment): TransitionMaterial | undefined {
  const pool = transitionPool(project);
  for (const extra of segment.extras) {
    const found = pool.find((transition) => transition.id === extra);
    if (found) return found;
  }
  return undefined;
}

export function end(range: TimeRange): Micros {
  return range.start + range.duration;
}

/**
 * The window a transition of `duration` occupies between two clips.
 *
 * Mirrors `resolve::window_for`. Centred on the cut, then each half clamped
 * independently into the clip it eats into — so an over-long transition becomes
 * asymmetric rather than being refused, which is what lets a clip be trimmed
 * under a transition that was legal when it was placed.
 *
 * `null` when there is no window at all.
 */
export function windowFor(
  cut: Micros,
  duration: Micros,
  from: Segment,
  to: Segment,
): TimeRange | null {
  if (duration <= 0) return null;
  // Integer halves, and the odd microsecond goes to the incoming side, exactly
  // as the Rust does — otherwise a one-frame transition is a microsecond
  // different in the two places.
  const half = Math.floor(duration / 2);
  const before = Math.min(half, Math.max(from.target_range.duration, 0));
  const after = Math.min(duration - half, Math.max(to.target_range.duration, 0));
  const total = before + after;
  return total > 0 ? { start: cut - before, duration: total } : null;
}

/**
 * The longest transition the two clips either side of a cut can carry.
 *
 * Mirrors `resolve::max_duration`: half the effect eats into each clip, so the
 * limit is twice the shorter of them. The UI clamps a drag to this rather than
 * discovering the limit by having the edit rejected — but Rust clamps too, and
 * it is Rust that is authoritative.
 */
export function maxDuration(from: Segment, to: Segment): Micros {
  return 2 * Math.max(0, Math.min(from.target_range.duration, to.target_range.duration));
}

/** One cut on a track, with whatever transition is on it. */
export interface Join {
  from: Segment;
  to: Segment;
  /** The instant the two clips meet. */
  cut: Micros;
  transition: TransitionMaterial | null;
  /** `null` when there is no transition, or when it resolves to no window. */
  window: TimeRange | null;
  /** What a drag on the duration handle may not exceed. */
  max: Micros;
}

/**
 * Every place two clips on `track` touch, in timeline order.
 *
 * Mirrors `resolve::spans`, plus the joins that have no transition yet — those
 * are where the boundary button goes, and they are the same set: a pair of
 * clips whose edges meet exactly. A gap is not a cut, and a transition on a
 * pair that no longer touches is skipped here exactly as the renderer skips it.
 */
export function joins(project: Project, track: Track): Join[] {
  const out: Join[] = [];
  for (let i = 0; i + 1 < track.segments.length; i += 1) {
    const from = track.segments[i];
    const to = track.segments[i + 1];
    const cut = to.target_range.start;
    if (end(from.target_range) !== cut) continue;
    const transition = transitionOf(project, to) ?? null;
    out.push({
      from,
      to,
      cut,
      transition,
      window: transition ? windowFor(cut, transition.duration, from, to) : null,
      max: maxDuration(from, to),
    });
  }
  return out;
}

/** The join at the head of `segmentId`, wherever on the project it is. */
export function joinAt(project: Project, segmentId: Id): Join | undefined {
  for (const track of project.tracks) {
    const found = joins(project, track).find((join) => join.to.id === segmentId);
    if (found) return found;
  }
  return undefined;
}

/**
 * The duration a drag on the window's edge asks for.
 *
 * The handle is one edge of the window and the window is symmetric about the
 * cut, so dragging an edge to `at` asks for twice the distance from the cut —
 * whichever edge is being dragged and whichever side of the cut the pointer
 * strays onto. Clamped into `[0, max]`, and a drag that reaches the cut asks
 * for nothing, which the caller reads as "remove it".
 */
export function durationFromDrag(cut: Micros, at: Micros, max: Micros): Micros {
  return Math.max(0, Math.min(max, Math.round(2 * Math.abs(at - cut))));
}

/**
 * The transition a drag-to-overlap gesture asks for.
 *
 * Dragging a clip over its left-hand neighbour cannot make the two overlap —
 * "segments within a track never overlap" is an invariant the whole editing
 * model rests on — so the overlap the user drew is read as the *length* of a
 * transition instead, and neither clip moves. `proposedStart` is where the drag
 * would have put the incoming clip; anything at or past the neighbour's end is
 * not an overlap at all and asks for nothing.
 */
export function overlapToDuration(
  neighbourEnd: Micros,
  proposedStart: Micros,
  max: Micros,
): Micros {
  return Math.max(0, Math.min(max, Math.round(neighbourEnd - proposedStart)));
}

/** Where a join's marker goes, in pixels, at `zoom` pixels per microsecond. */
export interface MarkerBox {
  left: number;
  width: number;
}

/**
 * The window in pixels, never narrower than `minWidth`.
 *
 * A half-second transition at the default zoom is 50 px, but the same
 * transition zoomed out to an hour on screen is a tenth of a pixel — and a
 * marker nobody can hit is a transition nobody can select or delete. The floor
 * is centred on the cut so it grows symmetrically, which keeps the marker over
 * the cut it belongs to instead of drifting off it.
 */
export function markerBox(join: Join, zoom: number, minWidth = 14): MarkerBox {
  const window = join.window;
  const width = window ? window.duration * zoom : 0;
  if (width >= minWidth) {
    return { left: (window as TimeRange).start * zoom, width };
  }
  return { left: join.cut * zoom - minWidth / 2, width: minWidth };
}
