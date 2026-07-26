/**
 * What a gesture selects.
 *
 * The rubber band is the reason this file exists. It is a hit test driven by a
 * pointer, which means the only way to check it by hand is to drag a mouse
 * across a running app and count highlighted rectangles — so the geometry is a
 * pure function and this is where it is pinned. The half-open comparison is the
 * part that is easy to get wrong and impossible to notice: off by one side and
 * a band drawn in the gap between two clips takes both of them.
 */

import { describe, expect, it } from "vitest";

import {
  allSelectableIds,
  liveSelection,
  runBetween,
  segmentsInBand,
} from "@/modules/timeline/lib/selection";
import { makeProject, makeSegment, makeTrack, range } from "@/test/fixtures";

const SECOND = 1_000_000;

/**
 * Two lanes. The video lane holds three one-second clips with a one-second gap
 * between the second and the third; the audio lane holds one clip under the
 * first two.
 */
function project(overrides: { locked?: boolean } = {}) {
  return makeProject({
    tracks: [
      makeTrack("video-1", {
        segments: [
          makeSegment("a", { target_range: range(0, SECOND) }),
          makeSegment("b", { target_range: range(SECOND, SECOND) }),
          makeSegment("c", { target_range: range(3 * SECOND, SECOND) }),
        ],
      }),
      makeTrack("audio-1", {
        kind: "audio",
        locked: overrides.locked ?? false,
        segments: [makeSegment("music", { target_range: range(0, 2 * SECOND) })],
      }),
    ],
  });
}

describe("a rubber band", () => {
  it("takes every clip it touches, on every lane it crossed", () => {
    const ids = segmentsInBand(project(), {
      from: 500_000,
      to: 1_500_000,
      trackIds: ["video-1", "audio-1"],
    });
    // Touched, not enclosed: a band dragged across the middle of a row takes
    // the whole row, which is what a band is for.
    expect(ids.sort()).toEqual(["a", "b", "music"]);
  });

  it("leaves alone the lanes it did not cross", () => {
    expect(
      segmentsInBand(project(), { from: 0, to: 4 * SECOND, trackIds: ["video-1"] }).sort(),
    ).toEqual(["a", "b", "c"]);
  });

  it("selects nothing when it is drawn in a gap", () => {
    // The gap runs from 2s to 3s. Both edges are exclusive, so a band that
    // stops exactly on a clip's leading edge does not take it — the same rule
    // the document uses for two clips not overlapping.
    expect(
      segmentsInBand(project(), { from: 2 * SECOND, to: 3 * SECOND, trackIds: ["video-1"] }),
    ).toEqual([]);
  });

  it("reads the same dragged left as dragged right", () => {
    const forwards = segmentsInBand(project(), {
      from: 500_000,
      to: 1_500_000,
      trackIds: ["video-1"],
    });
    const backwards = segmentsInBand(project(), {
      from: 1_500_000,
      to: 500_000,
      trackIds: ["video-1"],
    });
    expect(backwards).toEqual(forwards);
  });

  it("skips a locked lane, because nothing can be done to a clip on one", () => {
    const ids = segmentsInBand(project({ locked: true }), {
      from: 0,
      to: 4 * SECOND,
      trackIds: ["video-1", "audio-1"],
    });
    expect(ids).not.toContain("music");
  });

  it("has nothing to select with no document", () => {
    expect(segmentsInBand(null, { from: 0, to: SECOND, trackIds: ["video-1"] })).toEqual([]);
  });
});

describe("shift-clicking", () => {
  it("takes the run between the two clips, ends included", () => {
    expect(runBetween(project(), "a", "c")).toEqual(["a", "b", "c"]);
  });

  it("reads the same in either direction", () => {
    expect(runBetween(project(), "c", "a")).toEqual(["a", "b", "c"]);
  });

  it("takes only the clip clicked when the two are on different lanes", () => {
    // "Between" has no meaning across lanes: clips are ordered in time within a
    // track and not between tracks.
    expect(runBetween(project(), "music", "c")).toEqual(["c"]);
  });

  it("takes only the clip clicked when there is no anchor yet", () => {
    expect(runBetween(project(), null, "b")).toEqual(["b"]);
  });
});

describe("the selection after the document has moved on", () => {
  it("drops the ids of clips that are no longer there", () => {
    // The usual way this happens: an undo takes a clip away and leaves its id
    // in the selection. A Delete offered for it would do nothing.
    expect(liveSelection(project(), ["a", "gone", "c"])).toEqual(["a", "c"]);
  });

  it("drops the ids of clips whose lane has since been locked", () => {
    expect(liveSelection(project({ locked: true }), ["a", "music"])).toEqual(["a"]);
  });

  it("is empty with no document, whatever was selected in the last one", () => {
    expect(liveSelection(null, ["a"])).toEqual([]);
  });
});

describe("select all", () => {
  it("takes every clip on every unlocked lane", () => {
    expect(allSelectableIds(project()).sort()).toEqual(["a", "b", "c", "music"]);
    expect(allSelectableIds(project({ locked: true })).sort()).toEqual(["a", "b", "c"]);
  });

  it("selects nothing on an empty timeline", () => {
    expect(allSelectableIds(makeProject())).toEqual([]);
  });
});
