/**
 * Detach a video clip's sound onto a linked audio lane, and re-attach it.
 *
 * "Detach audio" builds, after the fact, exactly what importing the file would
 * have built in the first place: the picture where it is, the sound as a second
 * segment of the **same material** on an audio lane at the same instant, the
 * two held together by one link group. The importer's plan is `planPlacement`
 * in `edits.ts`; a test compares this planner's output against it so the two
 * shapes cannot drift apart — the mixer's rule ("a video clip whose sound sits
 * on a linked audio lane defers to it", `sound_is_on_a_linked_lane` in Rust)
 * only recognises that one shape.
 *
 * Both plans are one `Composite` through `timeline_apply`, so each is one undo
 * step; the primitives inside are the same ones the importer sends.
 */

import { newId } from "@/lib/ids";
import { runEdit } from "@/modules/project/store";
import type { Id, Project, Segment, Track } from "@/modules/project/types";
import { findSegment, linkGroupOf, soundIsOnALinkedLane } from "@/modules/project/types";
import { type EditCommand, timelineApply } from "@/modules/timeline/lib/api";
import { blankAudioTrack, blankSegment } from "@/modules/timeline/lib/edits";
import { isRangeFree } from "@/modules/timeline/lib/placement";

/**
 * Whether "Detach audio" applies to this clip: a video clip, on a non-audio
 * lane, whose file carries sound that is not already living on a linked lane.
 */
export function canDetachAudio(project: Project, segmentId: Id): boolean {
  const found = findSegment(project, segmentId);
  if (!found || found.track.kind === "audio") return false;
  const material = project.materials.videos.find((m) => m.id === found.segment.material_id);
  if (!material?.has_audio) return false;
  return !soundIsOnALinkedLane(project, found.track, found.segment);
}

/**
 * The edit that splits a clip's sound onto an audio lane. Null when the clip
 * is not detachable.
 */
export function planDetachAudio(
  project: Project,
  segmentId: Id,
  makeId: () => Id = newId,
): EditCommand | null {
  if (!canDetachAudio(project, segmentId)) return null;
  const found = findSegment(project, segmentId);
  if (!found) return null;
  const { segment } = found;

  // The sound has to land at the very instant the picture sits at — that is
  // what makes the pair worth linking — so a lane that is busy there is no
  // use, and a fresh one is less surprising than a refusal.
  const existingLane = project.tracks.find(
    (lane) =>
      lane.kind === "audio" &&
      !lane.locked &&
      isRangeFree(lane.segments, segment.target_range, null),
  );
  const lane: Track = existingLane ?? blankAudioTrack(project);

  // The importer's sound half is `blankSegment` of the same material at the
  // same instant. A clip that has since been trimmed, slipped, retimed or
  // turned down carries that into its sound, or detaching would audibly change
  // playback: the mixer stops reading these values off the video clip and
  // starts reading them off this one.
  const sound: Segment = {
    ...blankSegment(segment.material_id, segment.target_range.start, segment.target_range.duration),
    id: makeId(),
    source_range: { ...segment.source_range },
    speed: segment.speed,
    volume: segment.volume,
  };

  const commands: EditCommand[] = [];
  if (!existingLane) {
    commands.push({ type: "add_track", track: lane, index: project.tracks.length });
  }
  commands.push({
    type: "insert_segment",
    track_id: lane.id,
    segment: sound,
    index: lane.segments.filter((s) => s.target_range.start < sound.target_range.start).length,
  });

  // Into the picture's existing group when it has one — a third member, so a
  // hand-linked pair keeps moving together — and a fresh group of two
  // otherwise, in exactly the order `planPlacement` emits: picture first,
  // sound second, both after the inserts because a link group is set on
  // segments that exist.
  const existingGroup = linkGroupOf(project, segment);
  if (existingGroup) {
    commands.push({
      type: "set_link_group",
      segment_id: sound.id,
      before: null,
      after: existingGroup,
    });
  } else {
    const group = makeId();
    commands.push(
      { type: "set_link_group", segment_id: segment.id, before: null, after: group },
      { type: "set_link_group", segment_id: sound.id, before: null, after: group },
    );
  }

  return { type: "composite", label: "Detach audio", commands };
}

/**
 * The linked audio segment that carries this clip's sound, if there is one.
 * This existing is precisely what `soundIsOnALinkedLane` asserts.
 */
function detachedPartner(
  project: Project,
  segment: Segment,
  group: Id,
): { track: Track; segment: Segment; index: number } | null {
  for (const lane of project.tracks) {
    if (lane.kind !== "audio") continue;
    const index = lane.segments.findIndex(
      (other) =>
        other.id !== segment.id &&
        other.material_id === segment.material_id &&
        other.extras.includes(group),
    );
    if (index !== -1) return { track: lane, segment: lane.segments[index], index };
  }
  return null;
}

/**
 * The inverse gesture: remove the detached sound clip and dissolve the link,
 * so the video clip is heard from itself again — the mixer's rule flips back
 * the moment no linked audio clip of the same material exists.
 *
 * Trivial on purpose, which is why it is offered at all: it is three of the
 * same primitives detaching used, in one composite. The link edits come first
 * so the `remove_segment` is already inside a composite when `History::apply`
 * looks — composites are not link-mirrored, so the removal cannot drag the
 * video clip down with it.
 */
export function planReattachAudio(
  project: Project,
  segmentId: Id,
  // What the sound clip may have collected since it was detached — a trim it
  // took part in is shared with the picture, but a fade or a grade of its own
  // would be silently destroyed. The caller may want to warn; the plan
  // proceeds either way because the user asked for the picture's own sound.
): EditCommand | null {
  const found = findSegment(project, segmentId);
  if (!found || !soundIsOnALinkedLane(project, found.track, found.segment)) return null;
  const group = linkGroupOf(project, found.segment);
  if (!group) return null;
  const partner = detachedPartner(project, found.segment, group);
  if (!partner) return null;

  const members = project.tracks
    .flatMap((lane) => lane.segments)
    .filter((s) => s.extras.includes(group));

  const commands: EditCommand[] = [
    { type: "set_link_group", segment_id: partner.segment.id, before: group, after: null },
  ];
  // A group of one is not a group: when only the pair was linked, release the
  // picture too, or it keeps a link badge with no partner behind it. A group
  // the user grew by hand keeps its other members.
  if (members.length === 2) {
    commands.push({
      type: "set_link_group",
      segment_id: found.segment.id,
      before: group,
      after: null,
    });
  }
  commands.push({
    type: "remove_segment",
    track_id: partner.track.id,
    segment: partner.segment,
    index: partner.index,
  });

  return { type: "composite", label: "Re-attach audio", commands };
}

export async function detachAudio(project: Project, segmentId: Id): Promise<boolean> {
  const plan = planDetachAudio(project, segmentId);
  if (!plan) return false;
  return runEdit(() => timelineApply(plan));
}

export async function reattachAudio(project: Project, segmentId: Id): Promise<boolean> {
  const plan = planReattachAudio(project, segmentId);
  if (!plan) return false;
  return runEdit(() => timelineApply(plan));
}
