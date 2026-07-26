/**
 * What a paste puts down, and where.
 *
 * Two invariants carry this feature, and both are asserted here rather than
 * discovered in the app:
 *
 * - **A paste never overlaps.** Segments on a track may not overlap, Rust
 *   refuses an insert that would, and a refused composite leaves the user
 *   pressing Ctrl+V and watching nothing happen. So every destination is
 *   checked against the lane *including the clips this same paste has already
 *   placed*.
 * - **A copy is the whole clip.** Source range, speed, transform, effects and
 *   keyframes: the copy is not "a clip of the same file", it is that clip
 *   again, and anything dropped on the way is work the user has to redo.
 */

import { describe, expect, it } from "vitest";

import type { Project, Segment } from "@/modules/project/types";
import { rangeEnd } from "@/modules/project/types";
import type { EditCommand } from "@/modules/timeline/lib/api";
import { copyEntries, planPaste } from "@/modules/timeline/lib/clipboard";
import { makeProject, makeSegment, makeTrack, range } from "@/test/fixtures";

const SECOND = 1_000_000;

/** Ids a test can read: paste-1, paste-2, … */
function counter() {
  let next = 0;
  return () => {
    next += 1;
    return `paste-${next}`;
  };
}

function parts(command: EditCommand): EditCommand[] {
  return command.type === "composite" ? command.commands : [command];
}

function inserts(command: EditCommand) {
  return parts(command).filter((part) => part.type === "insert_segment");
}

/** Every clip that would be on `trackId` after the plan is applied. */
function occupancyAfter(project: Project, command: EditCommand, trackId: string) {
  const existing = (project.tracks.find((t) => t.id === trackId)?.segments ?? []).map(
    (s) => s.target_range,
  );
  const added = inserts(command)
    .filter((part) => part.type === "insert_segment" && part.track_id === trackId)
    .map((part) => (part.type === "insert_segment" ? part.segment.target_range : null))
    .filter((r): r is NonNullable<typeof r> => r !== null);
  return [...existing, ...added].sort((a, b) => a.start - b.start);
}

function overlaps(ranges: { start: number; duration: number }[]): boolean {
  return ranges.some((range, index) => {
    const next = ranges[index + 1];
    return next !== undefined && rangeEnd(range) > next.start;
  });
}

function twoLanes(video: Segment[] = [], audio: Segment[] = []) {
  return makeProject({
    tracks: [
      makeTrack("video-1", { segments: video }),
      makeTrack("audio-1", { kind: "audio", name: "Audio 1", segments: audio }),
    ],
  });
}

describe("copying", () => {
  it("takes the whole clip, not a reference to it", () => {
    const original = makeSegment("a", {
      target_range: range(SECOND, 2 * SECOND),
      source_range: range(4 * SECOND, 4 * SECOND),
      speed: 2,
      volume: 0.5,
      transform: {
        position: [0.25, -0.25],
        scale: [1.5, 1.5],
        rotation: 90,
        opacity: 0.4,
        flip_h: true,
        flip_v: false,
      },
      extras: ["effect-1"],
      keyframes: [
        {
          property: "opacity",
          keyframes: [
            { time: 0, value: 0, easing: "linear" },
            { time: SECOND, value: 1, easing: "ease_out" },
          ],
        },
      ],
    });
    const entries = copyEntries(twoLanes([original]), ["a"]);
    expect(entries).toHaveLength(1);

    const [entry] = entries;
    expect(entry.trackId).toBe("video-1");
    expect(entry.trackKind).toBe("video");
    expect(entry.offset).toBe(0);
    expect(entry.segment).toEqual(original);
    // A copy, not the document's own object: the store is replaced wholesale on
    // the next edit, and a clipboard pointing into the old one would go stale.
    expect(entry.segment).not.toBe(original);
    expect(entry.segment.keyframes).not.toBe(original.keyframes);
  });

  it("measures each clip from the earliest one, so a batch keeps its shape", () => {
    const project = twoLanes(
      [
        makeSegment("a", { target_range: range(SECOND, SECOND) }),
        makeSegment("b", { target_range: range(3 * SECOND, SECOND) }),
      ],
      [makeSegment("music", { target_range: range(SECOND, SECOND) })],
    );
    const entries = copyEntries(project, ["a", "b", "music"]);
    expect(entries.map((entry) => entry.offset)).toEqual([0, 2 * SECOND, 0]);
  });

  it("has nothing to say about ids that are not in the document", () => {
    expect(copyEntries(twoLanes(), ["gone"])).toEqual([]);
  });
});

describe("pasting", () => {
  it("lands at the playhead, on the lane the clip came from", () => {
    const project = twoLanes([makeSegment("a", { target_range: range(0, SECOND) })]);
    const entries = copyEntries(project, ["a"]);
    const plan = planPaste(project, entries, 5 * SECOND, { makeId: counter() });
    if (!plan) throw new Error("a clip can be pasted");

    expect(plan.command.type).toBe("insert_segment");
    const [insert] = inserts(plan.command);
    if (insert.type !== "insert_segment") throw new Error("an insert");
    expect(insert.track_id).toBe("video-1");
    expect(insert.segment.target_range).toEqual(range(5 * SECOND, SECOND));
    expect(insert.segment.id).toBe("paste-1");
    expect(plan.segmentIds).toEqual(["paste-1"]);
  });

  it("keeps the source range, the transform, the effects and the keyframes", () => {
    const original = makeSegment("a", {
      target_range: range(0, 2 * SECOND),
      source_range: range(6 * SECOND, 4 * SECOND),
      speed: 2,
      extras: ["effect-1"],
      keyframes: [{ property: "scale_x", keyframes: [{ time: 0, value: 1, easing: "ease_in" }] }],
    });
    const project = twoLanes([original]);
    const plan = planPaste(project, copyEntries(project, ["a"]), 4 * SECOND, {
      makeId: counter(),
    });
    if (!plan) throw new Error("a clip can be pasted");

    const [insert] = inserts(plan.command);
    if (insert.type !== "insert_segment") throw new Error("an insert");
    expect(insert.segment.source_range).toEqual(range(6 * SECOND, 4 * SECOND));
    expect(insert.segment.speed).toBe(2);
    expect(insert.segment.extras).toEqual(["effect-1"]);
    expect(insert.segment.keyframes).toEqual(original.keyframes);
    // Everything but the identity and the place.
    expect(insert.segment).toEqual({
      ...original,
      id: "paste-1",
      target_range: range(4 * SECOND, 2 * SECOND),
    });
  });

  it("goes to the next free lane when the playhead's instant is taken", () => {
    // The rule: at the playhead, on the original lane when that space is free,
    // and on the next free lane otherwise. Never on top of anything.
    const project = makeProject({
      tracks: [
        makeTrack("video-1", { segments: [makeSegment("a", { target_range: range(0, SECOND) })] }),
        makeTrack("video-2", { name: "Video 2" }),
      ],
    });
    const plan = planPaste(project, copyEntries(project, ["a"]), 0, { makeId: counter() });
    if (!plan) throw new Error("a clip can be pasted");

    const [insert] = inserts(plan.command);
    if (insert.type !== "insert_segment") throw new Error("an insert");
    expect(insert.track_id).toBe("video-2");
    // The time is what the user asked for; only the lane gave way.
    expect(insert.segment.target_range).toEqual(range(0, SECOND));
  });

  it("makes a lane when every one of the right kind is busy", () => {
    const project = twoLanes([makeSegment("a", { target_range: range(0, SECOND) })]);
    const plan = planPaste(project, copyEntries(project, ["a"]), 0, { makeId: counter() });
    if (!plan) throw new Error("a clip can be pasted");

    const [first] = parts(plan.command);
    expect(first.type).toBe("add_track");
    if (first.type !== "add_track") throw new Error("an add_track");
    expect(first.track.kind).toBe("video");
    // The lane has to exist before anything is inserted into it.
    const insert = inserts(plan.command)[0];
    if (insert.type !== "insert_segment") throw new Error("an insert");
    expect(insert.track_id).toBe(first.track.id);
  });

  it("never overlaps, even pasting a batch onto itself", () => {
    // Three clips in a row, copied and pasted back over their own originals.
    // Every destination is occupied, and each copy also has to miss the copies
    // placed before it.
    const project = twoLanes([
      makeSegment("a", { target_range: range(0, SECOND) }),
      makeSegment("b", { target_range: range(SECOND, SECOND) }),
      makeSegment("c", { target_range: range(2 * SECOND, SECOND) }),
    ]);
    const plan = planPaste(project, copyEntries(project, ["a", "b", "c"]), 0, {
      makeId: counter(),
    });
    if (!plan) throw new Error("a batch can be pasted");

    expect(inserts(plan.command)).toHaveLength(3);
    for (const track of [...project.tracks, ...trackIdsAdded(plan.command)]) {
      const id = typeof track === "string" ? track : track.id;
      expect(overlaps(occupancyAfter(project, plan.command, id)), `lane ${id}`).toBe(false);
    }
  });

  it("keeps the relative timing of the clips in a batch", () => {
    const project = twoLanes([
      makeSegment("a", { target_range: range(0, SECOND) }),
      makeSegment("b", { target_range: range(3 * SECOND, SECOND) }),
    ]);
    const plan = planPaste(project, copyEntries(project, ["a", "b"]), 10 * SECOND, {
      makeId: counter(),
    });
    if (!plan) throw new Error("a batch can be pasted");

    const starts = inserts(plan.command).map((part) =>
      part.type === "insert_segment" ? part.segment.target_range.start : -1,
    );
    // Three seconds apart before, three seconds apart after.
    expect(starts).toEqual([10 * SECOND, 13 * SECOND]);
  });

  it("relinks a pair that was copied whole, into a group of its own", () => {
    const project = makeProject({
      materials: {
        videos: [],
        audios: [],
        images: [],
        texts: [],
        links: ["group-1"],
        transitions: [],
        extras: {},
      },
      tracks: [
        makeTrack("video-1", {
          segments: [makeSegment("v", { target_range: range(0, SECOND), extras: ["group-1"] })],
        }),
        makeTrack("audio-1", {
          kind: "audio",
          segments: [makeSegment("a", { target_range: range(0, SECOND), extras: ["group-1"] })],
        }),
      ],
    });
    const plan = planPaste(project, copyEntries(project, ["v", "a"]), 5 * SECOND, {
      makeId: counter(),
    });
    if (!plan) throw new Error("a pair can be pasted");

    const links = parts(plan.command).filter((part) => part.type === "set_link_group");
    expect(links).toHaveLength(2);
    const groups = links.map((part) => (part.type === "set_link_group" ? part.after : null));
    expect(groups[0]).toBeTruthy();
    expect(groups[1]).toBe(groups[0]);
    // A group of its own, not the original's: dragging the copy must not drag
    // the original's sound.
    expect(groups[0]).not.toBe("group-1");
    for (const insert of inserts(plan.command)) {
      if (insert.type !== "insert_segment") throw new Error("an insert");
      expect(insert.segment.extras).not.toContain("group-1");
    }
    // And the links are set after both clips exist.
    const firstLink = parts(plan.command).findIndex((part) => part.type === "set_link_group");
    const lastInsert = parts(plan.command)
      .map((part) => part.type)
      .lastIndexOf("insert_segment");
    expect(firstLink).toBeGreaterThan(lastInsert);
  });

  it("pastes half a pair as a plain clip", () => {
    const project = makeProject({
      materials: {
        videos: [],
        audios: [],
        images: [],
        texts: [],
        links: ["group-1"],
        transitions: [],
        extras: {},
      },
      tracks: [
        makeTrack("video-1", {
          segments: [makeSegment("v", { target_range: range(0, SECOND), extras: ["group-1"] })],
        }),
      ],
    });
    const plan = planPaste(project, copyEntries(project, ["v"]), 5 * SECOND, {
      makeId: counter(),
    });
    if (!plan) throw new Error("a clip can be pasted");
    expect(parts(plan.command).some((part) => part.type === "set_link_group")).toBe(false);
  });

  it("has nothing to do with an empty clipboard", () => {
    expect(planPaste(twoLanes(), [], 0)).toBeNull();
  });
});

describe("duplicating", () => {
  it("slides along the lane rather than moving to another one", () => {
    // The `time` spill, which is what Duplicate uses: "another one of these,
    // here" is not a request to put it on a lane the user was not looking at.
    const project = twoLanes([
      makeSegment("a", { target_range: range(0, SECOND) }),
      makeSegment("b", { target_range: range(SECOND, SECOND) }),
    ]);
    const plan = planPaste(project, copyEntries(project, ["a"]), SECOND, {
      spill: "time",
      makeId: counter(),
    });
    if (!plan) throw new Error("a clip can be duplicated");

    const [insert] = inserts(plan.command);
    if (insert.type !== "insert_segment") throw new Error("an insert");
    expect(insert.track_id).toBe("video-1");
    // 1s is taken by "b", so the copy goes to the far side of it.
    expect(insert.segment.target_range).toEqual(range(2 * SECOND, SECOND));
  });
});

/** The lanes a plan adds, so the overlap check covers them too. */
function trackIdsAdded(command: EditCommand): string[] {
  return parts(command)
    .filter((part) => part.type === "add_track")
    .map((part) => (part.type === "add_track" ? part.track.id : ""));
}
