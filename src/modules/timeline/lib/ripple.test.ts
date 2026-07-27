/**
 * What a ripple sends across the boundary. Rust owns link expansion, ordering
 * within composites and the overlap rules; what this side owes it is a batch
 * whose parts are already in an order no intermediate state rejects — the
 * removal first, then the moves left to right — because `compose_edits`
 * deliberately leaves a mixed batch in the caller's order.
 */

import { describe, expect, it } from "vitest";

import {
  closeGapCommands,
  gapAt,
  rippleDeleteCommands,
  segmentsAfter,
} from "@/modules/timeline/lib/ripple";
import { makeProject, makeSegment, makeTrack, range } from "@/test/fixtures";

const SECOND = 1_000_000;

/** one[0..1s] · two[1..2s] · gap · three[3..4s], all on one lane. */
function rowWithGap() {
  const track = makeTrack("track-1", {
    segments: [
      makeSegment("one", { target_range: range(0, SECOND) }),
      makeSegment("two", { target_range: range(SECOND, SECOND) }),
      makeSegment("three", { target_range: range(3 * SECOND, SECOND) }),
    ],
  });
  return { project: makeProject({ tracks: [track] }), track };
}

describe("ripple delete", () => {
  it("removes the clip and pulls every later clip left by its duration", () => {
    const { project } = rowWithGap();
    const commands = rippleDeleteCommands(project, "one");

    expect(commands.map((c) => c.type)).toEqual(["remove_segment", "move_segment", "move_segment"]);
    expect(commands[0]).toMatchObject({ type: "remove_segment", track_id: "track-1", index: 0 });
    // Left to right, so each destination is vacated before something arrives.
    expect(commands[1]).toMatchObject({
      segment_id: "two",
      from_start: SECOND,
      to_start: 0,
    });
    expect(commands[2]).toMatchObject({
      segment_id: "three",
      from_start: 3 * SECOND,
      to_start: 2 * SECOND,
    });
  });

  it("moves nothing that sits before the clip", () => {
    const { project } = rowWithGap();
    const commands = rippleDeleteCommands(project, "two");

    expect(commands).toHaveLength(2);
    expect(commands[1]).toMatchObject({ segment_id: "three", to_start: 2 * SECOND });
  });

  it("is a plain delete for the last clip on the lane", () => {
    const { project } = rowWithGap();
    const commands = rippleDeleteCommands(project, "three");
    expect(commands.map((c) => c.type)).toEqual(["remove_segment"]);
    expect(segmentsAfter(project, "three")).toBe(0);
  });

  it("builds nothing for a clip that is no longer in the document", () => {
    const { project } = rowWithGap();
    expect(rippleDeleteCommands(project, "gone")).toEqual([]);
  });
});

describe("finding the gap under the pointer", () => {
  it("finds the bounded gap and its edges", () => {
    const { track } = rowWithGap();
    expect(gapAt(track, 2.5 * SECOND)).toEqual({ start: 2 * SECOND, end: 3 * SECOND });
  });

  it("counts the run before the first clip as a gap", () => {
    const track = makeTrack("track-1", {
      segments: [makeSegment("late", { target_range: range(2 * SECOND, SECOND) })],
    });
    expect(gapAt(track, SECOND)).toEqual({ start: 0, end: 2 * SECOND });
  });

  it("finds nothing inside a clip, and nothing after the last one", () => {
    const { track } = rowWithGap();
    expect(gapAt(track, SECOND / 2)).toBeNull();
    expect(gapAt(track, 10 * SECOND)).toBeNull();
  });
});

describe("close gap", () => {
  it("pulls everything after the gap left by its width", () => {
    const { project } = rowWithGap();
    const commands = closeGapCommands(project, "track-1", 2.5 * SECOND);

    expect(commands).toEqual([
      {
        type: "move_segment",
        segment_id: "three",
        from_track: "track-1",
        to_track: "track-1",
        from_start: 3 * SECOND,
        to_start: 2 * SECOND,
      },
    ]);
  });

  it("builds nothing on a locked lane or where there is no gap", () => {
    const { project } = rowWithGap();
    expect(closeGapCommands(project, "track-1", SECOND / 2)).toEqual([]);

    const locked = makeProject({
      tracks: [
        makeTrack("track-1", {
          locked: true,
          segments: [makeSegment("late", { target_range: range(2 * SECOND, SECOND) })],
        }),
      ],
    });
    expect(closeGapCommands(locked, "track-1", SECOND)).toEqual([]);
  });
});
