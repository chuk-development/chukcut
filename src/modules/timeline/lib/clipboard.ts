/**
 * Cut, copy and paste for clips.
 *
 * ## Where the clipboard lives, and why it is not in the document
 *
 * A cut clip is a `Segment` with nowhere to be. The project document has no
 * place for one — a segment belongs to a track, and every `EditCommand` that
 * touches a segment names the track it is on — and giving it one would be a
 * schema change that reaches the file on disk, the validator, the exporter and
 * every reader of `project-format.md`, in order to store something the user
 * does not think of as part of their project at all.
 *
 * So the clipboard is a list of detached segments in the timeline store, next
 * to the zoom and the playhead. Three consequences, all of them wanted:
 *
 * - It **survives closing a project and opening another**, because the store is
 *   never reloaded within a session while the document is replaced wholesale on
 *   every edit. Copying from one cut and pasting into the next is the reason
 *   anyone has a clipboard.
 * - It is **not undoable**, which is right: copying is not an edit. The paste
 *   is, and it is one step.
 * - It is **not saved**, which is also right — a clipboard that came back with
 *   the file three days later would be a surprise, and one more thing in the
 *   format to keep compatible.
 *
 * What it costs: nothing in Rust knows what is on the clipboard, so the native
 * menu is told one bit — "there is something to paste" — through `MenuState`.
 *
 * ## Where a paste lands
 *
 * At the playhead, on the lane the clip came from, and never on top of anything
 * — segments on a track may not overlap, and Rust refuses an insert that would.
 * A refusal is not a feature: the user pressed Ctrl+V and something has to
 * happen. So when the lane is busy at that instant the copy goes to the next
 * lane of the same kind that is free there, and if every one of them is busy, to
 * a new lane. Relative timing within a multi-clip paste is preserved by pasting
 * everything at its own offset from the earliest clip in the batch.
 */

import { newId } from "@/lib/ids";
import type {
  Id,
  Micros,
  Project,
  Segment,
  TimeRange,
  Track,
  TrackKind,
} from "@/modules/project/types";
import { linkGroupOf, rangeEnd } from "@/modules/project/types";
import type { EditCommand } from "@/modules/timeline/lib/api";
import { isRangeFree } from "@/modules/timeline/lib/placement";

/** One clip on the clipboard, detached from the document it came from. */
export interface ClipboardEntry {
  /**
   * The clip itself, whole: source range, speed, volume, transform, crop,
   * effects and keyframes. Its `id` is the *source* clip's and is never
   * pasted — it is kept only so that two entries can be recognised as having
   * come from the same clip. Pasting mints a fresh id.
   */
  segment: Segment;
  /** The lane it was on, which is where a paste tries first. */
  trackId: Id;
  /** What kind of lane that was, for when it has to land on a different one. */
  trackKind: TrackKind;
  /** How far after the earliest clip in the batch it started. */
  offset: Micros;
  /**
   * The link group it belonged to, if any.
   *
   * Kept so that copying *both* halves of a linked pair pastes a linked pair.
   * The group id itself is not reused — the copies are their own group, or
   * dragging a pasted clip would drag the original's partner.
   */
  linkGroup: Id | null;
  /**
   * The file behind the clip, when it has one.
   *
   * Only for the cross-project case, and it is the whole reason that case
   * works: a material id is meaningful inside one document and meaningless in
   * the next, so pasting into another project has to bring the *file* over and
   * take whatever id that project's pool gives it. The path is the identity the
   * two documents can agree on — `project_import_media` is keyed by it and
   * returns the existing material when the file is already in the pool.
   *
   * Null for a title, which has no file. Pasting one into another project is
   * therefore not possible yet; see `pasteEntries`.
   */
  materialPath: string | null;
}

/** How a paste behaves when the instant it wants is already occupied. */
export type Spill =
  /** Take the next free lane of the same kind, keeping the time. Paste. */
  | "lane"
  /** Stay on the lane and slide to the next gap. Duplicate. */
  | "time";

export interface PasteOptions {
  spill?: Spill;
  /** Injected so a test can assert on the ids it gets back. */
  makeId?: () => Id;
}

export interface PastePlan {
  /** One edit, so a paste is one undo step however many clips it puts down. */
  command: EditCommand;
  /** The ids of the pasted clips, which the caller selects. */
  segmentIds: Id[];
  /** Where the first of them landed, for scrolling to it. */
  start: Micros;
}

/**
 * Detach clips from the document.
 *
 * The segments are deep-copied: the store holds the document Rust last sent,
 * and it is replaced on the next edit, so a clipboard that pointed into it
 * would go stale — or worse, would still be there and describe clips that have
 * since been trimmed.
 */
export function copyEntries(project: Project, segmentIds: readonly Id[]): ClipboardEntry[] {
  const wanted = new Set(segmentIds);
  const found: { segment: Segment; track: Track }[] = [];
  for (const track of project.tracks) {
    for (const segment of track.segments) {
      if (wanted.has(segment.id)) found.push({ segment, track });
    }
  }
  if (found.length === 0) return [];

  const earliest = Math.min(...found.map(({ segment }) => segment.target_range.start));
  return found.map(({ segment, track }) => ({
    segment: structuredClone(segment),
    trackId: track.id,
    trackKind: track.kind,
    offset: segment.target_range.start - earliest,
    linkGroup: linkGroupOf(project, segment),
    materialPath: materialPathOf(project, segment.material_id),
  }));
}

/** Whether a material id means anything in this document. */
export function hasMaterial(project: Project, materialId: Id): boolean {
  const { videos, audios, images, texts } = project.materials;
  return (
    videos.some((m) => m.id === materialId) ||
    audios.some((m) => m.id === materialId) ||
    images.some((m) => m.id === materialId) ||
    texts.some((m) => m.id === materialId)
  );
}

/** The file behind a material, or null for the generated kinds. */
export function materialPathOf(project: Project, materialId: Id): string | null {
  const { videos, audios, images } = project.materials;
  return (
    videos.find((m) => m.id === materialId)?.path ??
    audios.find((m) => m.id === materialId)?.path ??
    images.find((m) => m.id === materialId)?.path ??
    null
  );
}

/** How long the whole batch is, from the first clip's start to the last one's end. */
export function clipboardSpan(entries: readonly ClipboardEntry[]): Micros {
  if (entries.length === 0) return 0;
  return Math.max(...entries.map((entry) => entry.offset + entry.segment.target_range.duration));
}

/**
 * The edit that puts the clipboard down at `at`.
 *
 * Pure, and it never produces an insert Rust would refuse: every destination is
 * checked against the lanes *including the clips this same paste has already
 * placed*, which is the case a per-clip check would miss.
 */
export function planPaste(
  project: Project,
  entries: readonly ClipboardEntry[],
  at: Micros,
  options: PasteOptions = {},
): PastePlan | null {
  const { spill = "lane", makeId = newId } = options;
  if (entries.length === 0) return null;

  // A working copy of what is where. Clips placed by this paste go in as they
  // are decided, so two copies of the same clip cannot land on each other.
  const occupancy = new Map<Id, { id: Id; target_range: TimeRange }[]>();
  for (const track of project.tracks) {
    occupancy.set(
      track.id,
      track.segments.map((s) => ({ id: s.id, target_range: s.target_range })),
    );
  }

  const addedTracks: Track[] = [];
  const lanes = (): Track[] => [...project.tracks, ...addedTracks];
  const inserts: EditCommand[] = [];
  const newTrackCommands: EditCommand[] = [];
  const ids: Id[] = [];
  /** Old group id → the clips of this paste that came out of it. */
  const groups = new Map<Id, Id[]>();
  let firstStart: Micros | null = null;

  // Earliest first, so that a batch fills a lane in the order it was copied and
  // the second clip is never asked to fit before the first one has claimed its
  // place.
  const ordered = [...entries].sort((a, b) => a.offset - b.offset);

  for (const entry of ordered) {
    const duration = entry.segment.target_range.duration;
    const wanted = Math.max(0, Math.round(at + entry.offset));
    const landing = place(entry, wanted, duration);
    if (!landing) return null;

    const id = makeId();
    ids.push(id);
    if (firstStart === null || landing.start < firstStart) firstStart = landing.start;

    const occupants = occupancy.get(landing.trackId) ?? [];
    occupants.push({ id, target_range: { start: landing.start, duration } });
    occupancy.set(landing.trackId, occupants);

    inserts.push({
      type: "insert_segment",
      track_id: landing.trackId,
      segment: {
        ...structuredClone(entry.segment),
        id,
        target_range: { start: landing.start, duration },
        // The link group is re-created below if both ends of a pair were
        // copied; anything else in `extras` — an effect, a transition — is
        // what the user copied and comes along.
        extras: entry.segment.extras.filter((extra) => extra !== entry.linkGroup),
      },
      index: occupants.filter((other) => other.target_range.start < landing.start).length,
    });

    if (entry.linkGroup) {
      groups.set(entry.linkGroup, [...(groups.get(entry.linkGroup) ?? []), id]);
    }
  }

  // A group of one is not a group: copying only the picture of a linked pair
  // gives a plain clip, exactly as duplicating one does.
  const links: EditCommand[] = [];
  for (const members of groups.values()) {
    if (members.length < 2) continue;
    const group = makeId();
    for (const segmentId of members) {
      // After the inserts, because a link group is set on segments that exist.
      links.push({ type: "set_link_group", segment_id: segmentId, before: null, after: group });
    }
  }

  const commands = [...newTrackCommands, ...inserts, ...links];
  return {
    command: commands.length === 1 ? commands[0] : { type: "composite", label: "Paste", commands },
    segmentIds: ids,
    start: firstStart ?? 0,
  };

  /** Where one clip of the batch goes. */
  function place(
    entry: ClipboardEntry,
    wanted: Micros,
    duration: Micros,
  ): { trackId: Id; start: Micros } | null {
    const free = (trackId: Id, start: Micros) =>
      isRangeFree(occupancy.get(trackId) ?? [], { start, duration }, null);

    const home = lanes().find((track) => track.id === entry.trackId && !track.locked);
    if (home && free(home.id, wanted)) return { trackId: home.id, start: wanted };

    if (spill === "time") {
      // Duplicate: stay on the lane the clip came from and slide forward. The
      // gesture is "another one of these, here", and moving it to a lane the
      // user was not looking at is not that.
      const lane = home ?? compatible(entry)[0];
      if (!lane) return null;
      return { trackId: lane.id, start: nextGap(occupancy.get(lane.id) ?? [], wanted, duration) };
    }

    for (const track of compatible(entry)) {
      if (track.id === home?.id) continue;
      if (free(track.id, wanted)) return { trackId: track.id, start: wanted };
    }

    // Every lane of the right kind is busy at that instant. A new one always
    // is free, and is a great deal less surprising than a paste that silently
    // did nothing.
    const fresh = blankTrack(entry.trackKind, [...lanes()]);
    addedTracks.push(fresh);
    occupancy.set(fresh.id, []);
    newTrackCommands.push({
      type: "add_track",
      track: fresh,
      index: project.tracks.length + addedTracks.length - 1,
    });
    return { trackId: fresh.id, start: wanted };
  }

  function compatible(entry: ClipboardEntry): Track[] {
    return lanes().filter((track) => track.kind === entry.trackKind && !track.locked);
  }

  function blankTrack(kind: TrackKind, existing: Track[]): Track {
    const count = existing.filter((track) => track.kind === kind).length;
    return {
      id: makeId(),
      kind,
      name: `${kind.charAt(0).toUpperCase()}${kind.slice(1)} ${count + 1}`,
      segments: [],
      muted: false,
      locked: false,
      hidden: false,
      volume: 1,
    };
  }
}

/** First position at or after `at` where `duration` fits between the occupants. */
function nextGap(
  occupants: readonly { target_range: TimeRange }[],
  at: Micros,
  duration: Micros,
): Micros {
  const sorted = [...occupants].sort((a, b) => a.target_range.start - b.target_range.start);
  let start = Math.max(0, at);
  for (const other of sorted) {
    const end = rangeEnd(other.target_range);
    if (end <= start) continue;
    if (other.target_range.start - start >= duration) break;
    start = end;
  }
  return start;
}
