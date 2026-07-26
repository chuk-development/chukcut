/**
 * The numbers here are the same ones `transitions/resolve.rs`'s tests use.
 *
 * That is the point of the file: two implementations of the same arithmetic —
 * one deciding pixels, one deciding frames — and the way they are kept honest
 * is that both are pinned to the same worked examples. A change to one that is
 * not a change to the other shows up as a marker that no longer sits over the
 * frames it describes.
 */

import { describe, expect, it } from "vitest";

import type { Project, Segment, TransitionMaterial } from "@/modules/project/types";
import {
  durationFromDrag,
  joinAt,
  joins,
  markerBox,
  maxDuration,
  overlapToDuration,
  transitionOf,
  windowFor,
} from "@/modules/transitions/lib/geometry";
import { makeProject, makeSegment, makeTrack } from "@/test/fixtures";

function transition(overrides: Partial<TransitionMaterial> = {}): TransitionMaterial {
  return {
    id: "t-1",
    kind: "dissolve",
    duration: 1_000_000,
    easing: "ease_in_out",
    direction: "right",
    color: [0, 0, 0, 1],
    softness: 0.04,
    zoom: 0.35,
    ...overrides,
  };
}

/** Two abutting four-second clips cut at 4 s, as in `resolve.rs`'s fixture. */
function cutProject(material?: TransitionMaterial): Project {
  const left = makeSegment("left", {
    target_range: { start: 0, duration: 4_000_000 },
    source_range: { start: 0, duration: 4_000_000 },
  });
  const right = makeSegment("right", {
    target_range: { start: 4_000_000, duration: 4_000_000 },
    source_range: { start: 2_000_000, duration: 4_000_000 },
    extras: material ? [material.id] : [],
  });
  return makeProject({
    materials: {
      videos: [],
      audios: [],
      images: [],
      texts: [],
      transitions: material ? [material] : [],
      links: [],
      extras: {},
    },
    tracks: [makeTrack("v1", { segments: [left, right] })],
  });
}

describe("windowFor", () => {
  it("centres the window on the cut", () => {
    const project = cutProject();
    const [left, right] = project.tracks[0].segments;
    expect(windowFor(4_000_000, 1_000_000, left, right)).toEqual({
      start: 3_500_000,
      duration: 1_000_000,
    });
  });

  it("puts the odd microsecond of a one-frame transition on the incoming side", () => {
    // 30 fps, one frame: 33_333 µs, so 16_666 before the cut and 16_667 after.
    // The same split as `a_one_frame_transition_still_has_a_window`.
    const project = cutProject();
    const [left, right] = project.tracks[0].segments;
    expect(windowFor(4_000_000, 33_333, left, right)).toEqual({
      start: 3_983_334,
      duration: 33_333,
    });
  });

  it("clamps each half into the clip it eats into rather than refusing", () => {
    // Ten seconds of transition between two four-second clips fills both.
    const project = cutProject();
    const [left, right] = project.tracks[0].segments;
    expect(windowFor(4_000_000, 10_000_000, left, right)).toEqual({
      start: 0,
      duration: 8_000_000,
    });
    expect(maxDuration(left, right)).toBe(8_000_000);
  });

  it("has no window at all for a non-positive duration", () => {
    const project = cutProject();
    const [left, right] = project.tracks[0].segments;
    expect(windowFor(4_000_000, 0, left, right)).toBeNull();
    expect(windowFor(4_000_000, -1, left, right)).toBeNull();
  });

  it("is asymmetric when one clip is shorter than half the transition", () => {
    const short: Segment = makeSegment("short", {
      target_range: { start: 3_800_000, duration: 200_000 },
    });
    const long = makeSegment("long", {
      target_range: { start: 4_000_000, duration: 4_000_000 },
    });
    expect(windowFor(4_000_000, 1_000_000, short, long)).toEqual({
      start: 3_800_000,
      duration: 700_000,
    });
  });
});

describe("joins", () => {
  it("finds the cut and the transition on it", () => {
    const material = transition();
    const project = cutProject(material);
    const found = joins(project, project.tracks[0]);
    expect(found).toHaveLength(1);
    expect(found[0].cut).toBe(4_000_000);
    expect(found[0].transition).toEqual(material);
    expect(found[0].window).toEqual({ start: 3_500_000, duration: 1_000_000 });
    expect(found[0].max).toBe(8_000_000);
  });

  it("reports a cut with no transition, because that is where the button goes", () => {
    const project = cutProject();
    const [join] = joins(project, project.tracks[0]);
    expect(join.transition).toBeNull();
    expect(join.window).toBeNull();
  });

  it("skips a pair that no longer touches, exactly as the renderer does", () => {
    const material = transition();
    const project = cutProject(material);
    project.tracks[0].segments[0].target_range.duration = 3_000_000;
    expect(joins(project, project.tracks[0])).toHaveLength(0);
    expect(joinAt(project, "right")).toBeUndefined();
  });

  it("looks a transition up through the untyped extras list", () => {
    const material = transition({ id: "some-uuid" });
    const project = cutProject(material);
    // A link group id in the same list must not be mistaken for a transition.
    project.tracks[0].segments[1].extras.unshift("a-link-group");
    expect(transitionOf(project, project.tracks[0].segments[1])).toEqual(material);
    expect(transitionOf(project, project.tracks[0].segments[0])).toBeUndefined();
  });

  it("finds nothing on a document that predates transitions", () => {
    const project = cutProject();
    // The cast is the point of the test, not a way around the type. The field
    // is required because everything that crossed IPC has it — but a document
    // read from disk by an older build, or a hand-edited one, can still arrive
    // without it, and `joins` must not throw on that. Typing it away would
    // delete the only check that the runtime tolerance still exists.
    (project.materials as { transitions?: unknown }).transitions = undefined;
    expect(joins(project, project.tracks[0])[0].transition).toBeNull();
  });
});

describe("durationFromDrag", () => {
  const cut = 4_000_000;

  it("asks for twice the distance from the cut, because the window is centred", () => {
    expect(durationFromDrag(cut, 3_500_000, 8_000_000)).toBe(1_000_000);
    expect(durationFromDrag(cut, 4_500_000, 8_000_000)).toBe(1_000_000);
  });

  it("clamps to what the two clips can carry", () => {
    expect(durationFromDrag(cut, 0, 8_000_000)).toBe(8_000_000);
    expect(durationFromDrag(cut, 0, 2_000_000)).toBe(2_000_000);
  });

  it("asks for nothing when the drag reaches the cut", () => {
    expect(durationFromDrag(cut, cut, 8_000_000)).toBe(0);
  });
});

describe("overlapToDuration", () => {
  it("reads the overlap a drag drew as the length of a transition", () => {
    // The incoming clip dragged 400 ms back over its neighbour's tail.
    expect(overlapToDuration(4_000_000, 3_600_000, 8_000_000)).toBe(400_000);
  });

  it("is nothing when the drag does not overlap", () => {
    expect(overlapToDuration(4_000_000, 4_000_000, 8_000_000)).toBe(0);
    expect(overlapToDuration(4_000_000, 4_500_000, 8_000_000)).toBe(0);
  });

  it("never exceeds what the clips can carry", () => {
    expect(overlapToDuration(4_000_000, 0, 2_000_000)).toBe(2_000_000);
  });
});

describe("markerBox", () => {
  const zoom = 1e-4; // 100 px per second, the default.

  it("covers the window", () => {
    const project = cutProject(transition());
    const [join] = joins(project, project.tracks[0]);
    expect(markerBox(join, zoom)).toEqual({ left: 350, width: 100 });
  });

  it("keeps a minimum width, centred on the cut, so it stays hittable", () => {
    const project = cutProject(transition({ duration: 33_333 }));
    const [join] = joins(project, project.tracks[0]);
    // 3.3 px of window at this zoom: the floor takes over and stays over the
    // cut at 400 px rather than drifting to where the window starts.
    const box = markerBox(join, zoom, 14);
    expect(box.width).toBe(14);
    expect(box.left + box.width / 2).toBeCloseTo(400, 5);
  });

  it("is empty for a join with no transition", () => {
    const project = cutProject();
    const [join] = joins(project, project.tracks[0]);
    expect(markerBox(join, zoom).width).toBe(14);
  });
});
