/**
 * Higher-level edits, expressed as `EditCommand`s.
 *
 * Everything here builds a command and hands it to Rust; nothing mutates the
 * document. Each function is one completed gesture, and therefore one undo
 * step — which is why a duplicate is a single insert and flipping a lane switch
 * sends the whole `TrackFlags` row rather than one edit per field.
 */

import { frameDuration, MICROS_PER_SECOND } from "@/lib/time";
import { runEdit } from "@/modules/project/store";
import type {
  Id,
  ImportedMaterial,
  Micros,
  Project,
  Segment,
  Track,
  TrackKind,
} from "@/modules/project/types";
import { findSegment, rangeEnd } from "@/modules/project/types";
import {
  type TrackFlags,
  timelineApply,
  timelineRedo,
  timelineSplit,
  timelineUndo,
} from "@/modules/timeline/lib/api";

/** UUID v4. `crypto.randomUUID` is missing on some webview origins, so fall back. */
export function newId(): Id {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (char) => {
    const random = (Math.random() * 16) | 0;
    const value = char === "x" ? random : (random & 0x3) | 0x8;
    return value.toString(16);
  });
}

export function undo(): Promise<boolean> {
  return runEdit(timelineUndo);
}

export function redo(): Promise<boolean> {
  return runEdit(timelineRedo);
}

export function deleteSegment(project: Project, segmentId: Id): Promise<boolean> {
  const found = findSegment(project, segmentId);
  if (!found) return Promise.resolve(false);
  return runEdit(() =>
    timelineApply({
      type: "remove_segment",
      track_id: found.track.id,
      segment: found.segment,
      index: found.index,
    }),
  );
}

/**
 * Place a copy immediately after the original, or in the first free slot after
 * it. Rust rejects an occupied range, so finding the gap here is what makes the
 * command succeed rather than merely be attempted.
 */
export function duplicateSegment(project: Project, segmentId: Id): Promise<boolean> {
  const found = findSegment(project, segmentId);
  if (!found) return Promise.resolve(false);

  const { track, segment, index } = found;
  const duration = segment.target_range.duration;
  let start = rangeEnd(segment.target_range);
  for (const other of track.segments) {
    if (other.id === segment.id) continue;
    if (other.target_range.start < start + duration && start < rangeEnd(other.target_range)) {
      start = rangeEnd(other.target_range);
    }
  }

  return runEdit(() =>
    timelineApply({
      type: "insert_segment",
      track_id: track.id,
      segment: {
        ...segment,
        id: newId(),
        target_range: { start, duration },
      },
      index: index + 1,
    }),
  );
}

/**
 * The clip the razor would cut.
 *
 * The selection wins whenever the playhead is inside it — that is the normal
 * path, and the one the razor tool uses. With nothing selected the fallback
 * prefers a *visual* lane over an audio one: with a video and a music bed both
 * running under the playhead, cutting the music and leaving the picture whole
 * is never what was meant. Within each group the upper lane wins, since that is
 * the one being looked at.
 */
export function segmentUnderPlayhead(
  project: Project,
  playhead: Micros,
  preferredId: Id | null,
): { trackId: Id; segmentId: Id } | null {
  if (preferredId) {
    const found = findSegment(project, preferredId);
    if (
      found &&
      playhead > found.segment.target_range.start &&
      playhead < rangeEnd(found.segment.target_range)
    ) {
      return { trackId: found.track.id, segmentId: found.segment.id };
    }
  }

  const hitIn = (kinds: (track: Track) => boolean) => {
    for (let i = project.tracks.length - 1; i >= 0; i--) {
      const track = project.tracks[i];
      if (track.locked || !kinds(track)) continue;
      const segment = track.segments.find(
        (s) => playhead > s.target_range.start && playhead < rangeEnd(s.target_range),
      );
      if (segment) return { trackId: track.id, segmentId: segment.id };
    }
    return null;
  };

  return hitIn((t) => t.kind !== "audio") ?? hitIn((t) => t.kind === "audio");
}

export function splitAt(segmentId: Id, at: Micros): Promise<boolean> {
  return runEdit(() => timelineSplit(segmentId, at));
}

export type TrackFlag = "muted" | "locked" | "hidden";

export function trackFlagsOf(track: Track): TrackFlags {
  return {
    muted: track.muted,
    locked: track.locked,
    hidden: track.hidden,
    volume: track.volume,
  };
}

/** Flip one lane switch. The other three ride along unchanged so undo restores the row as it was. */
export function toggleTrackFlag(track: Track, flag: TrackFlag): Promise<boolean> {
  const before = trackFlagsOf(track);
  return runEdit(() =>
    timelineApply({
      type: "set_track_flags",
      track_id: track.id,
      before,
      after: { ...before, [flag]: !before[flag] },
    }),
  );
}

export function setTrackVolume(track: Track, volume: number): Promise<boolean> {
  const before = trackFlagsOf(track);
  return runEdit(() =>
    timelineApply({
      type: "set_track_flags",
      track_id: track.id,
      before,
      after: { ...before, volume },
    }),
  );
}

/** Shortest legal clip: one frame. Trimming below this is rejected by Rust anyway. */
export function minClipDuration(fps: number): Micros {
  return frameDuration(fps);
}

// ---------------------------------------------------------------------------
// Bringing material onto the timeline
// ---------------------------------------------------------------------------

/** How long a still sits on the timeline when nothing says otherwise. */
export const STILL_DURATION: Micros = 5 * MICROS_PER_SECOND;

/**
 * First place at or after `at` where a clip of `duration` fits.
 *
 * Rust rejects an occupied range outright, which is the right behaviour for a
 * drag — the user aimed at a spot and deserves to be told it is taken. A drop
 * is different: the user is aiming at a lane, not at a microsecond, so sliding
 * to the next gap is what they meant. Always succeeds, because the space after
 * the last clip is unbounded.
 */
export function findFreeSlot(track: Track, at: Micros, duration: Micros): Micros {
  const sorted = [...track.segments].sort((a, b) => a.target_range.start - b.target_range.start);
  let start = Math.max(0, at);
  for (const segment of sorted) {
    const end = rangeEnd(segment.target_range);
    if (end <= start) continue;
    // Does the clip fit in the gap before this one?
    if (segment.target_range.start - start >= duration) break;
    start = end;
  }
  return start;
}

/**
 * The lane a material belongs on.
 *
 * A dropped song landing on the video track is technically legal — Rust does
 * not check kinds — and always wrong. Prefer the lane whose kind matches, and
 * fall back to whatever the user actually aimed at.
 */
export function preferredTrack(
  project: Project,
  kind: ImportedMaterial["kind"],
  aimedAt: Id | null,
): Track | null {
  const wanted: TrackKind = kind === "audio" ? "audio" : "video";
  const aimed = aimedAt ? project.tracks.find((t) => t.id === aimedAt) : null;
  if (aimed && !aimed.locked && aimed.kind === wanted) return aimed;

  const matching = project.tracks.find((t) => t.kind === wanted && !t.locked);
  if (matching) return matching;
  if (aimed && !aimed.locked) return aimed;
  return project.tracks.find((t) => !t.locked) ?? null;
}

export function blankSegment(materialId: Id, start: Micros, duration: Micros): Segment {
  return {
    id: newId(),
    material_id: materialId,
    target_range: { start, duration },
    // A freshly placed clip shows its material from the beginning; trimming is
    // what changes that.
    source_range: { start: 0, duration },
    render_index: 0,
    speed: 1,
    volume: 1,
    transform: {
      position: [0, 0],
      scale: [1, 1],
      rotation: 0,
      opacity: 1,
      flip_h: false,
      flip_v: false,
    },
    crop: null,
    extras: [],
    keyframes: [],
  };
}

/**
 * Put an imported material on a lane at `at`, sliding to the next gap if that
 * spot is taken. Returns the position it actually landed at, or null if the
 * edit was rejected.
 */
export async function insertMaterial(
  project: Project,
  material: Pick<ImportedMaterial, "id" | "kind" | "duration">,
  at: Micros,
  aimedAtTrack: Id | null,
): Promise<{ trackId: Id; start: Micros } | null> {
  const track = preferredTrack(project, material.kind, aimedAtTrack);
  if (!track) return null;

  const duration = material.duration > 0 ? material.duration : STILL_DURATION;
  const start = findFreeSlot(track, Math.max(0, Math.round(at)), duration);
  const index = track.segments.filter((s) => s.target_range.start < start).length;

  const ok = await runEdit(() =>
    timelineApply({
      type: "insert_segment",
      track_id: track.id,
      segment: blankSegment(material.id, start, duration),
      index,
    }),
  );
  return ok ? { trackId: track.id, start } : null;
}
