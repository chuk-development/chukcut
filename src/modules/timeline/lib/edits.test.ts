/**
 * What dropping a file on the timeline produces.
 *
 * `planPlacement` is pure so that this can be asserted without a backend, and
 * the thing being asserted is the whole feature: a file that carries picture
 * *and* sound becomes two clips on two lanes, at one instant, linked, in one
 * undo step. Every part of that sentence is a separate way to get it wrong.
 */

import { describe, expect, it } from "vitest";

import type { Segment } from "@/modules/project/types";
import type { EditCommand } from "@/modules/timeline/lib/api";
import {
  planPlacement,
  STILL_DURATION,
  setSpeedCommands,
  toggleMuteCommand,
} from "@/modules/timeline/lib/edits";
import { makeMaterial, makeProject, makeSegment, makeTrack, range } from "@/test/fixtures";

const SECOND = 1_000_000;

/** A project with one video lane and one audio lane, as `project_new` builds. */
function project(videoSegments: Segment[] = [], audioSegments: Segment[] = []) {
  return makeProject({
    tracks: [
      makeTrack("video-1", { kind: "video", segments: videoSegments }),
      makeTrack("audio-1", { kind: "audio", name: "Audio 1", segments: audioSegments }),
    ],
  });
}

function parts(command: EditCommand): EditCommand[] {
  return command.type === "composite" ? command.commands : [command];
}

function inserts(command: EditCommand) {
  return parts(command).filter((part) => part.type === "insert_segment");
}

describe("planPlacement", () => {
  it("puts a file with both streams on two lanes at the same instant, linked", () => {
    const material = makeMaterial("m1", { duration: 4 * SECOND, has_audio: true });
    const placement = planPlacement(project(), material, 2 * SECOND, "video-1");
    if (!placement) throw new Error("a video should be placeable");

    // One command, so one undo step. A Ctrl+Z that left the sound behind would
    // be a mess the user has to clean up by hand.
    expect(placement.command.type).toBe("composite");

    const placed = inserts(placement.command);
    expect(placed).toHaveLength(2);
    const [picture, sound] = placed;
    if (picture.type !== "insert_segment" || sound.type !== "insert_segment") {
      throw new Error("both parts are inserts");
    }

    expect(picture.track_id).toBe("video-1");
    expect(sound.track_id).toBe("audio-1");
    expect(placement.audioTrackId).toBe("audio-1");

    // The same file, at the same place, for the same length. Anything else and
    // the two are not two views of one clip.
    expect(sound.segment.material_id).toBe(picture.segment.material_id);
    expect(sound.segment.target_range).toEqual(picture.segment.target_range);
    expect(sound.segment.source_range).toEqual(picture.segment.source_range);
    expect(picture.segment.target_range).toEqual(range(2 * SECOND, 4 * SECOND));
    expect(sound.segment.id).not.toBe(picture.segment.id);

    // And one group holding them together, set *after* both exist.
    const links = parts(placement.command).filter((part) => part.type === "set_link_group");
    expect(links).toHaveLength(2);
    const groups = links.map((part) => (part.type === "set_link_group" ? part.after : null));
    expect(groups[0]).toBeTruthy();
    expect(groups[1]).toBe(groups[0]);
    expect(links.map((part) => (part.type === "set_link_group" ? part.segment_id : null))).toEqual([
      picture.segment.id,
      sound.segment.id,
    ]);
    expect(
      parts(placement.command).findIndex((part) => part.type === "set_link_group"),
    ).toBeGreaterThan(parts(placement.command).findIndex((part) => part.type === "insert_segment"));
  });

  it("leaves a silent file, a still and a song as one clip", () => {
    const silent = planPlacement(
      project(),
      makeMaterial("m1", { duration: 4 * SECOND, has_audio: false }),
      0,
      "video-1",
    );
    expect(silent?.command.type).toBe("insert_segment");
    expect(silent?.audioTrackId).toBeNull();

    const still = planPlacement(
      project(),
      makeMaterial("m2", { kind: "image", duration: 0, has_audio: false }),
      0,
      "video-1",
    );
    expect(still?.command.type).toBe("insert_segment");
    if (still?.command.type !== "insert_segment") throw new Error("an insert");
    expect(still.command.segment.target_range.duration).toBe(STILL_DURATION);

    // A song is already sound: splitting it off itself would be two clips where
    // one is meant.
    const song = planPlacement(
      project(),
      makeMaterial("m3", { kind: "audio", duration: 4 * SECOND, has_audio: true }),
      0,
      "audio-1",
    );
    expect(song?.command.type).toBe("insert_segment");
    if (song?.command.type !== "insert_segment") throw new Error("an insert");
    expect(song.command.track_id).toBe("audio-1");
  });

  it("slides the pair to an instant that is free on both lanes", () => {
    // The video lane is clear where the drop landed and the audio lane is not.
    // Placing the picture there anyway would produce an insert Rust refuses,
    // and refusing it takes the whole composite down — so the drop would appear
    // to do nothing at all.
    const busy = project([], [makeSegment("music", { target_range: range(0, 6 * SECOND) })]);
    const placement = planPlacement(
      busy,
      makeMaterial("m1", { duration: 4 * SECOND, has_audio: true }),
      SECOND,
      "video-1",
    );
    if (!placement) throw new Error("placeable");

    expect(placement.start).toBe(6 * SECOND);
    for (const part of inserts(placement.command)) {
      if (part.type !== "insert_segment") throw new Error("an insert");
      expect(part.segment.target_range.start).toBe(6 * SECOND);
    }
  });

  it("makes an audio lane when the project has none", () => {
    const videoOnly = makeProject({
      tracks: [makeTrack("video-1", { kind: "video" })],
    });
    const placement = planPlacement(
      videoOnly,
      makeMaterial("m1", { duration: 4 * SECOND, has_audio: true }),
      0,
      "video-1",
    );
    if (!placement) throw new Error("placeable");

    const [first, ...rest] = parts(placement.command);
    expect(first.type).toBe("add_track");
    if (first.type !== "add_track") throw new Error("an add_track");
    expect(first.track.kind).toBe("audio");
    expect(first.index).toBe(1);
    // The lane has to exist before anything is inserted into it.
    const sound = rest.find(
      (part) => part.type === "insert_segment" && part.track_id === first.track.id,
    );
    expect(sound).toBeDefined();
    expect(placement.audioTrackId).toBe(first.track.id);
  });

  it("has nowhere to put anything in a project with no lanes", () => {
    expect(planPlacement(makeProject(), makeMaterial("m1"), 0, null)).toBeNull();
  });
});

describe("toggleMuteCommand", () => {
  it("mutes with the current level in `before`, so undo restores it", () => {
    const segment = makeSegment("a", { volume: 0.7 });
    const command = toggleMuteCommand(segment, null);
    expect(command).toEqual({ type: "set_volume", segment_id: "a", before: 0.7, after: 0 });
  });

  it("un-mutes to the remembered level, and to full volume with nothing remembered", () => {
    const muted = makeSegment("a", { volume: 0 });
    expect(toggleMuteCommand(muted, 0.7)).toEqual({
      type: "set_volume",
      segment_id: "a",
      before: 0,
      after: 0.7,
    });
    // Nothing remembered — a restarted session — falls back to full rather
    // than staying silent.
    expect(toggleMuteCommand(muted, null)).toEqual({
      type: "set_volume",
      segment_id: "a",
      before: 0,
      after: 1,
    });
  });
});

describe("setSpeedCommands", () => {
  it("retimes the clip and its link partners together, skipping ones already there", () => {
    const project = makeProject({
      materials: {
        videos: [],
        audios: [],
        images: [],
        texts: [],
        links: ["g"],
        transitions: [],
        extras: {},
      },
      tracks: [
        makeTrack("video-1", {
          segments: [makeSegment("pic", { extras: ["g"] })],
        }),
        makeTrack("audio-1", {
          kind: "audio",
          segments: [makeSegment("snd", { extras: ["g"] }), makeSegment("bed")],
        }),
      ],
    });

    const commands = setSpeedCommands(project, "pic", 2);
    // The pair retimes together — a linked pair is one piece of footage — and
    // the unrelated bed is untouched.
    expect(commands).toEqual([
      { type: "set_speed", segment_id: "pic", before: 1, after: 2 },
      { type: "set_speed", segment_id: "snd", before: 1, after: 2 },
    ]);

    // Already at the asked-for speed: nothing to send, no empty undo step.
    expect(setSpeedCommands(project, "pic", 1)).toEqual([]);
  });
});
