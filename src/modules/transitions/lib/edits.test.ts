/**
 * The four gestures, as they cross IPC.
 *
 * What is defended here is which command each gesture sends and with what
 * payload — `segmentId` against `segment_id`, a retime against an add — because
 * that is the part a refactor on either side of the boundary can silently
 * change. The arithmetic behind them is `geometry.test.ts`'s.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useProjectStore } from "@/modules/project/store";
import type { Project, TransitionMaterial } from "@/modules/project/types";
import { applyOverlap, overlapGesture, retimeTransition } from "@/modules/transitions/lib/edits";
import { makeEditResponse, makeProject, makeSegment, makeTrack } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

function transition(): TransitionMaterial {
  return {
    id: "t-1",
    kind: "dissolve",
    duration: 1_000_000,
    easing: "ease_in_out",
    direction: "right",
    color: [0, 0, 0, 1],
    softness: 0.04,
    zoom: 0.35,
  };
}

function cutProject(material?: TransitionMaterial): Project {
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
    tracks: [
      makeTrack("lane", {
        segments: [
          makeSegment("left", { target_range: { start: 0, duration: 4_000_000 } }),
          makeSegment("right", {
            target_range: { start: 4_000_000, duration: 4_000_000 },
            extras: material ? [material.id] : [],
          }),
        ],
      }),
    ],
  });
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  useProjectStore.setState({ project: null, error: null });
});

afterEach(() => {
  ipc.restore();
});

describe("the overlap gesture", () => {
  it("reads a drag back over the neighbour as a transition of that length", () => {
    expect(overlapGesture(cutProject(), "right", 3_600_000)).toEqual({
      segmentId: "right",
      duration: 400_000,
    });
  });

  it("is not the gesture when the drag goes the other way", () => {
    expect(overlapGesture(cutProject(), "right", 4_200_000)).toBeNull();
  });

  it("is not the gesture for a clip with nothing before it", () => {
    expect(overlapGesture(cutProject(), "left", -500_000)).toBeNull();
  });

  it("adds a transition when the cut is bare", async () => {
    const project = cutProject();
    ipc.handle("transitions_add", makeEditResponse(project));
    expect(await applyOverlap(project, "right", 3_600_000)).toBe(true);
    expect(ipc.lastCall("transitions_add")).toEqual({
      segmentId: "right",
      kind: "dissolve",
      duration: 400_000,
    });
  });

  it("lengthens the one that is already there rather than adding a second", async () => {
    const project = cutProject(transition());
    ipc.handle("transitions_retime", makeEditResponse(project));
    expect(await applyOverlap(project, "right", 3_000_000)).toBe(true);
    expect(ipc.count("transitions_add")).toBe(0);
    expect(ipc.lastCall("transitions_retime")).toEqual({
      segmentId: "right",
      duration: 1_000_000,
    });
  });
});

describe("retiming to nothing", () => {
  it("removes the transition rather than sending a zero-length one", async () => {
    ipc.handle("transitions_remove", makeEditResponse(cutProject()));
    await retimeTransition("right", 0);
    expect(ipc.count("transitions_retime")).toBe(0);
    expect(ipc.lastCall("transitions_remove")).toEqual({ segmentId: "right" });
  });
});

describe("a refused edit", () => {
  it("becomes a message on the project store rather than an exception", async () => {
    ipc.fail("transitions_add", "the clips either side are too short");
    expect(await applyOverlap(cutProject(), "right", 3_600_000)).toBe(false);
    expect(useProjectStore.getState().error).toBe("the clips either side are too short");
  });
});
