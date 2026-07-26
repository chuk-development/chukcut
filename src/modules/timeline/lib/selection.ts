/**
 * What a gesture selects.
 *
 * All of it is pure and takes the document as an argument, for the same reason
 * `placement.ts` is: a hit test that can only be exercised by dragging a mouse
 * across a real timeline is a hit test nobody checks. The component's job is to
 * turn pixels into a time range and a set of lanes; deciding which clips that
 * covers is here.
 *
 * A **locked** lane is never selected into. Nothing can be done to a clip on
 * one — it cannot be moved, trimmed or deleted — so putting it in a selection
 * only produces edits Rust refuses, and a rubber band that visibly grabs a clip
 * it cannot move is worse than one that skips it.
 */

import type { Id, Micros, Project, Track } from "@/modules/project/types";
import { rangeEnd } from "@/modules/project/types";

/** The rectangle a rubber band covers, once the pixels are gone from it. */
export interface SelectionBand {
  /** Earlier edge, in timeline microseconds. */
  from: Micros;
  /** Later edge. */
  to: Micros;
  /** The lanes the band's top and bottom edges reach, in any order. */
  trackIds: readonly Id[];
}

function selectable(track: Track): boolean {
  return !track.locked;
}

/** Every clip that could be selected, top lane first. */
export function allSelectableIds(project: Project | null): Id[] {
  if (!project) return [];
  return project.tracks
    .filter(selectable)
    .flatMap((track) => track.segments.map((segment) => segment.id));
}

/**
 * The clips a rubber band touches.
 *
 * *Touches*, not encloses: a band dragged across the middle of a row of clips
 * takes all of them, which is what a band is for. A clip is in when its
 * timeline range overlaps the band's and its lane is one the band crossed.
 *
 * The overlap is half-open on both sides, so a band that stops exactly on a
 * clip's leading edge does not take it — the same rule the document uses for
 * two clips not overlapping, and the reason a band drawn in the gap between two
 * clips selects neither.
 */
export function segmentsInBand(project: Project | null, band: SelectionBand): Id[] {
  if (!project) return [];
  const lanes = new Set(band.trackIds);
  const from = Math.min(band.from, band.to);
  const to = Math.max(band.from, band.to);

  return project.tracks
    .filter((track) => selectable(track) && lanes.has(track.id))
    .flatMap((track) =>
      track.segments
        .filter(
          (segment) => segment.target_range.start < to && from < rangeEnd(segment.target_range),
        )
        .map((segment) => segment.id),
    );
}

/**
 * The run of clips between two of them, for a shift-click.
 *
 * Along one lane, because that is the only place "between" has an unambiguous
 * meaning: clips on a track are ordered in time and cannot overlap, so the run
 * is every clip whose range lies between the two ends. Shift-clicking a clip on
 * another lane has no run to speak of, so it selects that one clip and nothing
 * in between — which is what the empty-ish answer here means.
 *
 * Returns the ends as well as what is between them, so the caller can hand the
 * whole thing to `extendSelection` without adding the ends back itself.
 */
export function runBetween(project: Project | null, anchorId: Id | null, targetId: Id): Id[] {
  if (!project) return [targetId];
  const track = anchorId
    ? project.tracks.find(
        (candidate) =>
          selectable(candidate) &&
          candidate.segments.some((segment) => segment.id === anchorId) &&
          candidate.segments.some((segment) => segment.id === targetId),
      )
    : undefined;
  if (!track) return [targetId];

  const first = track.segments.findIndex((segment) => segment.id === anchorId);
  const last = track.segments.findIndex((segment) => segment.id === targetId);
  if (first < 0 || last < 0) return [targetId];

  const [low, high] = first <= last ? [first, last] : [last, first];
  return track.segments.slice(low, high + 1).map((segment) => segment.id);
}

/**
 * The selection, minus whatever is no longer selectable.
 *
 * An edit replaces the whole document, and a clip that was deleted — or whose
 * lane was locked — leaves its id behind in the selection. Every consumer runs
 * the selection through this, so nothing downstream has to cope with an id that
 * names nothing: a Delete offered for a clip that is gone is the exact "looks
 * clickable, does nothing" failure the menu exists to prevent.
 */
export function liveSelection(project: Project | null, selection: readonly Id[]): Id[] {
  if (!project || selection.length === 0) return [];
  const alive = new Set(allSelectableIds(project));
  return selection.filter((id) => alive.has(id));
}
