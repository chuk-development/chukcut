/**
 * Track management: adding, deleting and reordering lanes.
 *
 * Each function is one completed gesture and therefore one undo step. Render
 * order is never touched here — it is derived from track order on the Rust
 * side (`reindex_render_order`), so moving a lane *is* restacking the
 * composite, and there is nothing else to keep in sync.
 */

import { newId } from "@/lib/ids";
import { runEdit } from "@/modules/project/store";
import type { Id, Project, Track, TrackKind } from "@/modules/project/types";
import { timelineApply } from "@/modules/timeline/lib/api";
import { trackHeight } from "@/modules/timeline/store";

/** The lane kinds a user can add by hand. Sticker and effect lanes arrive with their content. */
export const ADDABLE_KINDS: TrackKind[] = ["video", "audio", "text"];

const KIND_LABEL: Record<TrackKind, string> = {
  video: "Video",
  audio: "Audio",
  text: "Text",
  sticker: "Sticker",
  effect: "Effect",
};

/** "Video 3" for the third video lane, whatever order the lanes sit in. */
export function nextTrackName(project: Project, kind: TrackKind): string {
  const count = project.tracks.filter((track) => track.kind === kind).length;
  return `${KIND_LABEL[kind]} ${count + 1}`;
}

export function blankTrack(project: Project, kind: TrackKind): Track {
  return {
    id: newId(),
    kind,
    name: nextTrackName(project, kind),
    segments: [],
    muted: false,
    locked: false,
    hidden: false,
    volume: 1,
  };
}

/**
 * Append a fresh lane at the bottom of the list.
 *
 * The bottom of the list is the top of the stack — render order follows track
 * order — which is where a new lane is useful: whatever is dropped on it paints
 * over what is already there instead of vanishing behind it.
 */
export function addTrack(project: Project, kind: TrackKind): Promise<boolean> {
  return runEdit(() =>
    timelineApply({
      type: "add_track",
      track: blankTrack(project, kind),
      index: project.tracks.length,
    }),
  );
}

/**
 * Delete a lane, clips and all.
 *
 * The command carries the whole track, so one undo puts the lane back with
 * everything on it. Confirmation for a non-empty lane is the caller's job —
 * this function is the part that runs after the user has said yes.
 */
export function deleteTrack(project: Project, trackId: Id): Promise<boolean> {
  const index = project.tracks.findIndex((track) => track.id === trackId);
  if (index === -1) return Promise.resolve(false);
  return runEdit(() =>
    timelineApply({ type: "remove_track", track: project.tracks[index], index }),
  );
}

/** Move a lane so it ends up at `toIndex` in the new order. */
export function moveTrack(project: Project, trackId: Id, toIndex: number): Promise<boolean> {
  const fromIndex = project.tracks.findIndex((track) => track.id === trackId);
  if (fromIndex === -1 || toIndex === fromIndex) return Promise.resolve(false);
  if (toIndex < 0 || toIndex >= project.tracks.length) return Promise.resolve(false);
  return runEdit(() =>
    timelineApply({
      type: "move_track",
      track_id: trackId,
      from_index: fromIndex,
      to_index: toIndex,
    }),
  );
}

/**
 * Where a header dragged by `deltaY` pixels would land in the track order.
 *
 * Pure geometry over the lanes' own heights — lanes are not all the same
 * height, so an index cannot be had by dividing. A lane yields its place once
 * the dragged header's centre passes *its* centre, both measured in the layout
 * as it stands: measuring the others in the list with the dragged lane taken
 * out shifts everything below it up by its height, and a lane then yields to a
 * header that has not moved at all.
 */
export function reorderTarget(tracks: readonly Track[], fromIndex: number, deltaY: number): number {
  const heights = tracks.map((track) => trackHeight(track.kind));
  const topOf = (index: number) => heights.slice(0, index).reduce((sum, h) => sum + h, 0);
  const centreOf = (index: number) => topOf(index) + heights[index] / 2;
  const centre = centreOf(fromIndex) + deltaY;

  let target = fromIndex;
  if (deltaY > 0) {
    for (let index = fromIndex + 1; index < tracks.length; index++) {
      if (centre > centreOf(index)) target = index;
    }
  } else {
    for (let index = fromIndex - 1; index >= 0; index--) {
      if (centre < centreOf(index)) target = index;
    }
  }
  return target;
}
