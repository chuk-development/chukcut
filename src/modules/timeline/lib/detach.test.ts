/**
 * Detach audio must create *the* linked shape, not *a* linked shape.
 *
 * The mixer's rule — "a video clip whose sound sits on a linked audio lane is
 * heard from there" (`sound_is_on_a_linked_lane` in Rust) — recognises exactly
 * what the importer builds: a second segment of the same material on an audio
 * lane, held to the picture by one link group. So the strongest test here is
 * not "detach looks reasonable" but "detach emits what `planPlacement` would
 * have emitted", compared command for command with only the minted ids
 * normalised. If the two planners drift apart, this file is what fails.
 */

import { describe, expect, it } from "vitest";

import type { EditCommand } from "@/modules/timeline/lib/api";
import { canDetachAudio, planDetachAudio, planReattachAudio } from "@/modules/timeline/lib/detach";
import { planPlacement } from "@/modules/timeline/lib/edits";
import { makeProject, makeSegment, makeTrack } from "@/test/fixtures";

const SECOND = 1_000_000;

/** Ids a test can read: id-1, id-2, … */
function counter(prefix: string) {
  let next = 0;
  return () => {
    next += 1;
    return `${prefix}-${next}`;
  };
}

function parts(command: EditCommand): EditCommand[] {
  return command.type === "composite" ? command.commands : [command];
}

/** A project with one video lane, one audio lane, and one video file with sound. */
function importable() {
  return makeProject({
    materials: {
      videos: [
        {
          id: "mat",
          path: "/media/clip.mp4",
          width: 1920,
          height: 1080,
          duration: 4 * SECOND,
          fps: 30,
          has_audio: true,
          rotation: 0,
        },
      ],
      audios: [],
      images: [],
      texts: [],
      links: [],
      transitions: [],
      extras: {},
    },
    tracks: [
      makeTrack("video-1", { segments: [] }),
      makeTrack("audio-1", { kind: "audio", name: "Audio 1", segments: [] }),
    ],
  });
}

const MATERIAL = { id: "mat", kind: "video" as const, duration: 4 * SECOND, has_audio: true };

describe("planDetachAudio", () => {
  it("emits exactly the linked shape planPlacement emits for the same file", () => {
    // What the importer would do with this file.
    const fresh = importable();
    const placed = planPlacement(fresh, MATERIAL, 0, "video-1");
    expect(placed).not.toBeNull();
    const placedParts = parts(placed?.command as EditCommand);
    // [insert picture, insert sound, link picture, link sound] — the audio
    // lane exists, so no add_track.
    expect(placedParts.map((part) => part.type)).toEqual([
      "insert_segment",
      "insert_segment",
      "set_link_group",
      "set_link_group",
    ]);
    const importedSound = placedParts[1];
    if (importedSound.type !== "insert_segment") throw new Error("shape asserted above");

    // The same project after "the sound was never split off": only the
    // picture, unlinked — which is also what deleting the audio half and
    // dissolving the group leaves behind.
    const detached = importable();
    const picture = placedParts[0];
    if (picture.type !== "insert_segment") throw new Error("shape asserted above");
    detached.tracks[0].segments = [structuredClone(picture.segment)];

    const plan = planDetachAudio(detached, picture.segment.id, counter("mint"));
    expect(plan).not.toBeNull();
    const planParts = parts(plan as EditCommand);

    // The same commands, in the same order, minus the picture insert the
    // detach does not need: sound insert, then picture link, then sound link.
    expect(planParts.map((part) => part.type)).toEqual([
      "insert_segment",
      "set_link_group",
      "set_link_group",
    ]);

    const sound = planParts[0];
    if (sound.type !== "insert_segment") throw new Error("shape asserted above");
    expect(sound.track_id).toBe(importedSound.track_id);
    expect(sound.index).toBe(importedSound.index);
    // The segments must be identical field for field once the minted id is
    // set aside — same material, same ranges, same defaults everywhere else.
    expect({ ...sound.segment, id: "x" }).toEqual({ ...importedSound.segment, id: "x" });

    // And the linking is the importer's: picture first, sound second, one
    // fresh group shared by exactly the two.
    const [linkPicture, linkSound] = planParts.slice(1);
    if (linkPicture.type !== "set_link_group" || linkSound.type !== "set_link_group") {
      throw new Error("shape asserted above");
    }
    expect(linkPicture.segment_id).toBe(picture.segment.id);
    expect(linkSound.segment_id).toBe(sound.segment.id);
    expect(linkPicture.before).toBeNull();
    expect(linkSound.before).toBeNull();
    expect(linkPicture.after).toBe(linkSound.after);
    expect(linkPicture.after).not.toBeNull();
  });

  it("carries the clip's trim, speed and volume into the sound", () => {
    // A clip edited since import: the mixer will stop reading these values off
    // the video clip the moment the linked audio clip exists, so they have to
    // come along or detaching audibly changes playback.
    const project = importable();
    const picture = makeSegment("pic", {
      material_id: "mat",
      target_range: { start: 2 * SECOND, duration: SECOND },
      source_range: { start: SECOND, duration: 2 * SECOND },
      speed: 2,
      volume: 0.5,
    });
    project.tracks[0].segments = [picture];

    const plan = planDetachAudio(project, "pic", counter("mint"));
    const sound = parts(plan as EditCommand)[0];
    if (sound.type !== "insert_segment") throw new Error("first command inserts the sound");
    expect(sound.segment.target_range).toEqual({ start: 2 * SECOND, duration: SECOND });
    expect(sound.segment.source_range).toEqual({ start: SECOND, duration: 2 * SECOND });
    expect(sound.segment.speed).toBe(2);
    expect(sound.segment.volume).toBe(0.5);
  });

  it("takes a fresh audio lane when the existing one is busy at that instant", () => {
    const project = importable();
    project.tracks[0].segments = [makeSegment("pic", { material_id: "mat" })];
    project.tracks[1].segments = [
      makeSegment("bed", { material_id: "mat", target_range: { start: 0, duration: SECOND } }),
    ];

    const plan = planDetachAudio(project, "pic", counter("mint"));
    const planParts = parts(plan as EditCommand);
    expect(planParts[0].type).toBe("add_track");
    const insert = planParts[1];
    if (insert.type !== "insert_segment") throw new Error("then the sound is inserted");
    const added = planParts[0];
    if (added.type !== "add_track") throw new Error("asserted above");
    expect(insert.track_id).toBe(added.track.id);
    expect(added.track.kind).toBe("audio");
  });

  it("is refused on the clips it cannot mean anything for", () => {
    const project = importable();
    project.tracks[0].segments = [makeSegment("pic", { material_id: "mat" })];

    // A clip whose sound is already on a linked lane: the entry flips to
    // Re-attach instead.
    const linked = structuredClone(project);
    linked.materials.links = ["g"];
    linked.tracks[0].segments[0].extras = ["g"];
    linked.tracks[1].segments = [makeSegment("snd", { material_id: "mat", extras: ["g"] })];
    expect(canDetachAudio(linked, "pic")).toBe(false);
    expect(planDetachAudio(linked, "pic")).toBeNull();

    // A silent file has no sound to detach.
    const silent = structuredClone(project);
    silent.materials.videos[0].has_audio = false;
    expect(planDetachAudio(silent, "pic")).toBeNull();

    // A clip already on an audio lane is heard from itself.
    const onAudio = importable();
    onAudio.tracks[1].segments = [makeSegment("pic", { material_id: "mat" })];
    expect(planDetachAudio(onAudio, "pic")).toBeNull();
  });
});

describe("planReattachAudio", () => {
  /** The detached state: picture and sound linked as one group of two. */
  function detachedPair() {
    const project = importable();
    project.materials.links = ["g"];
    project.tracks[0].segments = [makeSegment("pic", { material_id: "mat", extras: ["g"] })];
    project.tracks[1].segments = [makeSegment("snd", { material_id: "mat", extras: ["g"] })];
    return project;
  }

  it("removes the sound clip and dissolves the pair, link edits first", () => {
    const plan = planReattachAudio(detachedPair(), "pic");
    const planParts = parts(plan as EditCommand);
    // The link edits come first so the removal is already inside a composite
    // when the history looks — a bare remove of one half of a linked pair
    // would take the picture down with it.
    expect(planParts.map((part) => part.type)).toEqual([
      "set_link_group",
      "set_link_group",
      "remove_segment",
    ]);
    const [unlinkSound, unlinkPicture, remove] = planParts;
    if (
      unlinkSound.type !== "set_link_group" ||
      unlinkPicture.type !== "set_link_group" ||
      remove.type !== "remove_segment"
    ) {
      throw new Error("shape asserted above");
    }
    expect(unlinkSound.segment_id).toBe("snd");
    expect(unlinkSound.before).toBe("g");
    expect(unlinkSound.after).toBeNull();
    expect(unlinkPicture.segment_id).toBe("pic");
    expect(remove.segment.id).toBe("snd");
    expect(remove.track_id).toBe("audio-1");
  });

  it("keeps a hand-grown group alive for its other members", () => {
    const project = detachedPair();
    project.tracks[0].segments.push(
      makeSegment("title", {
        material_id: "mat",
        extras: ["g"],
        target_range: { start: 2 * SECOND, duration: SECOND },
      }),
    );
    const plan = planReattachAudio(project, "pic");
    const planParts = parts(plan as EditCommand);
    // Only the sound leaves the group: the picture still moves with "title".
    expect(planParts.map((part) => part.type)).toEqual(["set_link_group", "remove_segment"]);
  });

  it("has nothing to do on a clip whose sound is not detached", () => {
    const project = importable();
    project.tracks[0].segments = [makeSegment("pic", { material_id: "mat" })];
    expect(planReattachAudio(project, "pic")).toBeNull();
  });
});
