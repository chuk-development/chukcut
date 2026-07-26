/**
 * Higher-level edits, expressed as `EditCommand`s.
 *
 * Everything here builds a command and hands it to Rust; nothing mutates the
 * document. Each function is one completed gesture, and therefore one undo
 * step — which is why a duplicate is a single insert and flipping a lane switch
 * sends the whole `TrackFlags` row rather than one edit per field.
 */

import { newId } from "@/lib/ids";
import { frameDuration, MICROS_PER_SECOND } from "@/lib/time";
import { projectGet, projectImportMedia } from "@/modules/project/lib/api";
import { describeError, runEdit, useProjectStore } from "@/modules/project/store";
import type {
  Id,
  ImportedMaterial,
  Micros,
  Project,
  Segment,
  TimeRange,
  Track,
  TrackKind,
} from "@/modules/project/types";
import { findSegment, rangeEnd } from "@/modules/project/types";
import {
  type EditCommand,
  type TrackFlags,
  timelineApply,
  timelineApplyMany,
  timelineLink,
  timelineRedo,
  timelineSplit,
  timelineUndo,
  timelineUnlink,
} from "@/modules/timeline/lib/api";
import {
  type ClipboardEntry,
  clipboardSpan,
  copyEntries,
  hasMaterial,
  planPaste,
} from "@/modules/timeline/lib/clipboard";

export { newId };

export function undo(): Promise<boolean> {
  return runEdit(timelineUndo);
}

export function redo(): Promise<boolean> {
  return runEdit(timelineRedo);
}

export function deleteSegment(project: Project, segmentId: Id): Promise<boolean> {
  return deleteSegments(project, [segmentId]);
}

/**
 * Take a whole selection off the timeline in one undo step.
 *
 * One `remove_segment` per clip and nothing else: link partners and the order
 * the removals apply in are Rust's to work out, in `compose_edits`, because
 * both are facts about the document rather than about what the user could see.
 * A selection holding both halves of a linked pair must not delete the sound
 * twice, and only the document knows the two are a pair.
 */
export function deleteSegments(project: Project, segmentIds: readonly Id[]): Promise<boolean> {
  const commands = removalCommands(project, segmentIds);
  if (commands.length === 0) return Promise.resolve(false);
  return runEdit(() =>
    timelineApplyMany(commands, commands.length > 1 ? "Delete clips" : "Delete clip"),
  );
}

function removalCommands(project: Project, segmentIds: readonly Id[]): EditCommand[] {
  const commands: EditCommand[] = [];
  for (const segmentId of segmentIds) {
    const found = findSegment(project, segmentId);
    // Silently skipped rather than refused: an id in the selection that is no
    // longer in the document is an ordinary consequence of an undo, and the
    // rest of the selection is still there to delete.
    if (found) {
      commands.push({
        type: "remove_segment",
        track_id: found.track.id,
        segment: found.segment,
        index: found.index,
      });
    }
  }
  return commands;
}

/**
 * Place a copy of each selected clip immediately after the block they form, on
 * the lane it came from, sliding to the next gap when that is taken. Rust
 * rejects an occupied range, so finding the gap here is what makes the command
 * succeed rather than merely be attempted.
 *
 * The copies are not members of the originals' link groups. A copy has no
 * partner of its own — duplicating a linked clip duplicates one clip — and a
 * third member would mean dragging the copy dragged the original's sound with
 * it. Copying *both* halves is different and does produce a linked pair; see
 * `clipboard.ts`.
 */
export function duplicateSegment(project: Project, segmentId: Id): Promise<boolean> {
  return duplicateSegments(project, [segmentId]);
}

export function duplicateSegments(project: Project, segmentIds: readonly Id[]): Promise<boolean> {
  const entries = copyEntries(project, segmentIds);
  if (entries.length === 0) return Promise.resolve(false);

  const earliest = Math.min(
    ...entries.map((entry) => entry.segment.target_range.start - entry.offset),
  );
  // Immediately after the block, which for one clip is immediately after that
  // clip — the behaviour Duplicate has always had.
  const plan = planPaste(project, entries, earliest + clipboardSpan(entries), { spill: "time" });
  if (!plan) return Promise.resolve(false);
  return runEdit(() => timelineApply(labelled(plan.command, "Duplicate")));
}

/**
 * Put the clipboard down at `at`.
 *
 * Returns the ids of the clips that landed, so the caller can select them —
 * pasting and then having to find what you pasted is the difference between the
 * feature working and it appearing to work.
 *
 * Pasting into a *different* project first brings the files over; see
 * [`adopt`]. That is an IPC round trip per unknown file and none at all in the
 * ordinary case, where the material is already in the pool.
 */
export async function pasteEntries(
  project: Project,
  entries: readonly ClipboardEntry[],
  at: Micros,
): Promise<Id[]> {
  const adopted = await adopt(project, entries);
  if (adopted.entries.length === 0) return [];

  const plan = planPaste(adopted.project, adopted.entries, at);
  if (!plan) return [];
  const ok = await runEdit(() => timelineApply(plan.command));
  return ok ? plan.segmentIds : [];
}

/**
 * Make the clipboard's materials mean something in *this* document.
 *
 * A material id identifies a file inside one project and nothing at all in the
 * next, so a clip pasted across projects would reference a material the
 * document has never heard of — which `Project::validate()` calls an error, and
 * which the renderer and the exporter would have nothing to draw for. The file
 * is the identity the two documents can agree on, so the fix is to import it:
 * `project_import_media` is keyed by path and hands back the existing material
 * when there already is one, so this is idempotent and costs nothing when the
 * clip came from the project it is going back into.
 *
 * The import is not undoable — it never has been, for the same reason the media
 * library's own import is not: a material with no clip on it is inert, and a
 * Ctrl+Z after a paste emptying the media panel would be worse.
 *
 * A clip whose material has no file — a title — is dropped rather than pasted
 * broken. Carrying a `TextMaterial` across would need an edit command that
 * writes the material pool, which does not exist yet.
 */
async function adopt(
  project: Project,
  entries: readonly ClipboardEntry[],
): Promise<{ project: Project; entries: ClipboardEntry[] }> {
  const missing = [...new Set(entries.map((entry) => entry.segment.material_id))].filter(
    (id) => !hasMaterial(project, id),
  );
  if (missing.length === 0) return { project, entries: [...entries] };

  /** Old material id → the id this project knows the same file by. */
  const adopted = new Map<Id, Id>();
  for (const materialId of missing) {
    const path = entries.find((entry) => entry.segment.material_id === materialId)?.materialPath;
    if (!path) continue;
    try {
      const material = await projectImportMedia(path);
      adopted.set(materialId, material.id);
    } catch (error) {
      useProjectStore.getState().setError(describeError(error));
    }
  }

  // The pool changed under us. The document is server state, so it is re-read
  // rather than patched locally — and the paste has to be planned against the
  // version that has the new materials in it.
  let current = project;
  if (adopted.size > 0) {
    try {
      const fresh = await projectGet();
      if (fresh) {
        useProjectStore.getState().refreshDocument(fresh);
        current = fresh;
      }
    } catch (error) {
      useProjectStore.getState().setError(describeError(error));
    }
  }

  return {
    project: current,
    entries: entries
      .filter(
        (entry) =>
          adopted.has(entry.segment.material_id) || hasMaterial(current, entry.segment.material_id),
      )
      .map((entry) => {
        const replacement = adopted.get(entry.segment.material_id);
        return replacement
          ? { ...entry, segment: { ...entry.segment, material_id: replacement } }
          : entry;
      }),
  };
}

/** Detach clips from the document, for the clipboard to hold. */
export function copySegments(project: Project, segmentIds: readonly Id[]): ClipboardEntry[] {
  return copyEntries(project, segmentIds);
}

/** One clip's share of a drag. */
export interface SegmentMove {
  segmentId: Id;
  fromTrackId: Id;
  toTrackId: Id;
  fromStart: Micros;
  toStart: Micros;
}

/** Move a whole selection as one undo step. */
export function moveSegments(moves: readonly SegmentMove[]): Promise<boolean> {
  if (moves.length === 0) return Promise.resolve(false);
  const commands: EditCommand[] = moves.map((move) => ({
    type: "move_segment",
    segment_id: move.segmentId,
    from_track: move.fromTrackId,
    to_track: move.toTrackId,
    from_start: move.fromStart,
    to_start: move.toStart,
  }));
  return runEdit(() => timelineApplyMany(commands, moves.length > 1 ? "Move clips" : "Move clip"));
}

/** One clip's share of a trim. Both ranges on both sides, as the command wants. */
export interface SegmentTrim {
  segmentId: Id;
  beforeTarget: TimeRange;
  beforeSource: TimeRange;
  afterTarget: TimeRange;
  afterSource: TimeRange;
}

/** Trim a whole selection at the same edge, as one undo step. */
export function trimSegments(trims: readonly SegmentTrim[]): Promise<boolean> {
  if (trims.length === 0) return Promise.resolve(false);
  const commands: EditCommand[] = trims.map((trim) => ({
    type: "trim_segment",
    segment_id: trim.segmentId,
    before_target: trim.beforeTarget,
    before_source: trim.beforeSource,
    after_target: trim.afterTarget,
    after_source: trim.afterSource,
  }));
  return runEdit(() => timelineApplyMany(commands, trims.length > 1 ? "Trim clips" : "Trim clip"));
}

/** A composite's label, without rebuilding it. */
function labelled(command: EditCommand, label: string): EditCommand {
  return command.type === "composite" ? { ...command, label } : command;
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
 *
 * Takes the occupants rather than a lane because a file with both streams
 * lands on *two* lanes at the same instant, and the instant has to be free on
 * both of them: an insert that Rust refuses because the audio lane was busy
 * would take the picture down with it.
 */
export function findFreeSlot(segments: readonly Segment[], at: Micros, duration: Micros): Micros {
  const sorted = [...segments].sort((a, b) => a.target_range.start - b.target_range.start);
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

/** The audio lane a file's sound should land on, if the project has one free. */
export function audioLaneFor(project: Project): Track | null {
  return project.tracks.find((track) => track.kind === "audio" && !track.locked) ?? null;
}

/** A fresh audio lane, for a project whose only one is gone or locked. */
export function blankAudioTrack(project: Project): Track {
  const count = project.tracks.filter((track) => track.kind === "audio").length;
  return {
    id: newId(),
    kind: "audio",
    name: `Audio ${count + 1}`,
    segments: [],
    muted: false,
    locked: false,
    hidden: false,
    volume: 1,
  };
}

/** Where a dropped material is going, and the single edit that puts it there. */
export interface Placement {
  command: EditCommand;
  /** The lane the picture landed on — what the caller selects and scrolls to. */
  trackId: Id;
  start: Micros;
  /** The lane the sound landed on, when the file had both streams. */
  audioTrackId: Id | null;
}

/**
 * The edit that brings a material onto the timeline.
 *
 * A file that carries **both** streams becomes two clips, not one: the picture
 * on a video lane and the sound on an audio lane, at the same instant, linked
 * so that every later edit moves them together. That is what every editor does,
 * and the reason is that the alternative — sound welded to the clip it arrived
 * with — makes the ordinary jobs (duck the music under a line, keep the audio
 * while cutting away from the speaker) impossible without first undoing the
 * decision.
 *
 * The whole thing is one `Composite`, which is one undo step: a drop is one
 * gesture, and a Ctrl+Z that leaves the sound behind is a bug the user has to
 * clean up by hand.
 *
 * Pure, so that what a drop *would* do can be tested without a backend.
 */
export function planPlacement(
  project: Project,
  material: Pick<ImportedMaterial, "id" | "kind" | "duration" | "has_audio">,
  at: Micros,
  aimedAtTrack: Id | null,
): Placement | null {
  const track = preferredTrack(project, material.kind, aimedAtTrack);
  if (!track) return null;

  const duration = material.duration > 0 ? material.duration : STILL_DURATION;
  const wanted = Math.max(0, Math.round(at));

  // Sound of its own only for a picture that has some. A file that is only
  // audio is already on an audio lane, and splitting a still into two would be
  // two clips where one is meant.
  const splitsOff = material.kind === "video" && material.has_audio && track.kind !== "audio";
  if (!splitsOff) {
    const start = findFreeSlot(track.segments, wanted, duration);
    return {
      command: {
        type: "insert_segment",
        track_id: track.id,
        segment: blankSegment(material.id, start, duration),
        index: indexFor(track, start),
      },
      trackId: track.id,
      start,
      audioTrackId: null,
    };
  }

  const existingLane = audioLaneFor(project);
  const lane = existingLane ?? blankAudioTrack(project);
  // The instant has to be free on both lanes, because the pair has to land at
  // the same `target_range` to be worth linking.
  const start = findFreeSlot([...track.segments, ...lane.segments], wanted, duration);

  const picture = blankSegment(material.id, start, duration);
  const sound = blankSegment(material.id, start, duration);
  const group = newId();

  const commands: EditCommand[] = [];
  if (!existingLane) {
    commands.push({ type: "add_track", track: lane, index: project.tracks.length });
  }
  commands.push(
    {
      type: "insert_segment",
      track_id: track.id,
      segment: picture,
      index: indexFor(track, start),
    },
    {
      type: "insert_segment",
      track_id: lane.id,
      segment: sound,
      index: indexFor(lane, start),
    },
    // After the inserts, because a link group is set on segments that exist.
    { type: "set_link_group", segment_id: picture.id, before: null, after: group },
    { type: "set_link_group", segment_id: sound.id, before: null, after: group },
  );

  return {
    command: { type: "composite", label: "Add clip", commands },
    trackId: track.id,
    start,
    audioTrackId: lane.id,
  };
}

/** Where a clip starting at `start` sits in a lane's segment order. */
function indexFor(track: Track, start: Micros): number {
  return track.segments.filter((s) => s.target_range.start < start).length;
}

/**
 * Put an imported material on a lane at `at`, sliding to the next gap if that
 * spot is taken. Returns the position it actually landed at, or null if the
 * edit was rejected.
 */
export async function insertMaterial(
  project: Project,
  material: Pick<ImportedMaterial, "id" | "kind" | "duration" | "has_audio">,
  at: Micros,
  aimedAtTrack: Id | null,
): Promise<{ trackId: Id; start: Micros } | null> {
  const placement = planPlacement(project, material, at, aimedAtTrack);
  if (!placement) return null;

  const ok = await runEdit(() => timelineApply(placement.command));
  return ok ? { trackId: placement.trackId, start: placement.start } : null;
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

/**
 * Break the group a clip belongs to.
 *
 * Which clips that is, is Rust's to decide: the group may hold a clip the panel
 * cannot see, and releasing only the two on screen would leave a link badge on
 * a clip with no partner.
 */
export function unlinkSegment(segmentId: Id): Promise<boolean> {
  return runEdit(() => timelineUnlink(segmentId));
}

/** Make several clips move, trim and delete as one. */
export function linkSegments(segmentIds: Id[]): Promise<boolean> {
  return runEdit(() => timelineLink(segmentIds));
}
