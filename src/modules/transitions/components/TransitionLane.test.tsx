/**
 * The gestures that create, retime and delete a transition.
 *
 * What is worth defending here is the *shape* of each gesture and the payload
 * it puts on the wire, not the pixels: a drag is one edit on release, the
 * duration it sends is twice the pointer's distance from the cut because the
 * window is centred on it, and a drag that reaches the cut is a removal rather
 * than a zero-length transition.
 *
 * Driven through real pointer events on the rendered marker, so a change to how
 * the drag is wired up fails here rather than passing against the props.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { useProjectStore } from "@/modules/project/store";
import type { Micros, Project, TransitionMaterial } from "@/modules/project/types";
import { TransitionLane } from "@/modules/transitions/components/TransitionLane";
import { useTransitionStore } from "@/modules/transitions/store";
import { makeEditResponse, makeProject, makeSegment, makeTrack } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

/** 100 px per second, the timeline's default. */
const ZOOM = 1e-4;

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

/** Two four-second clips meeting at 4 s, optionally with a transition on the cut. */
function cutProject(material?: TransitionMaterial, kind: "video" | "audio" = "video"): Project {
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
        kind,
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

/** The lane, with the timeline's own pixel-to-time mapping stubbed in. */
function open(project: Project) {
  useProjectStore.getState().loadProject(project);
  // The provider is the app's, in `App.tsx`; the lane is rendered inside it.
  return render(
    <TooltipProvider>
      <TransitionLane
        project={project}
        track={project.tracks[0]}
        zoom={ZOOM}
        laneHeight={56}
        timeAtClientX={(clientX: number) => Math.round(clientX / ZOOM) as Micros}
      />
    </TooltipProvider>,
  );
}

/** Press on `element` at the instant `from`, move to `to`, release. */
function drag(element: HTMLElement, from: Micros, to: Micros) {
  fireEvent.pointerDown(element, { pointerId: 1, button: 0, clientX: from * ZOOM });
  fireEvent.pointerMove(window, { pointerId: 1, clientX: to * ZOOM });
  fireEvent.pointerUp(window, { pointerId: 1, clientX: to * ZOOM });
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  useProjectStore.setState({ project: null, error: null });
  useTransitionStore.setState({ selectedSegmentId: null, dragDuration: null, error: null });
});

afterEach(() => {
  ipc.restore();
});

describe("a cut with no transition", () => {
  it("offers a button that adds one at the head of the incoming clip", async () => {
    const project = cutProject();
    ipc.handle("transitions_add", makeEditResponse(project));
    open(project);

    const button = screen.getByLabelText("Add a transition between left and right");
    fireEvent.click(button);

    await waitFor(() => expect(ipc.count("transitions_add")).toBe(1));
    expect(ipc.lastCall("transitions_add")).toEqual({
      segmentId: "right",
      kind: "dissolve",
      duration: null,
    });
  });

  it("puts the button on the cut", () => {
    open(cutProject());
    const button = screen.getByLabelText("Add a transition between left and right");
    // 4 s at 100 px/s is 400 px, less half the 16 px button.
    expect(button.style.left).toBe("392px");
  });
});

describe("a cut with a transition", () => {
  it("draws the window rather than the cut", () => {
    open(cutProject(transition()));
    const marker = screen.getByLabelText("Transition into right");
    // A one-second transition centred on 4 s: [3.5 s, 4.5 s), which is 350 px
    // wide 100 px.
    expect(marker.style.left).toBe("350px");
    expect(marker.style.width).toBe("100px");
  });

  it("sends one retime on release, of twice the distance from the cut", async () => {
    const project = cutProject(transition());
    ipc.handle("transitions_retime", makeEditResponse(project));
    open(project);

    // Drag the left edge out to 3 s: a full second before the cut, so the
    // transition the user drew is two seconds long.
    drag(screen.getByLabelText("Transition into right"), 3_500_000, 3_000_000);

    await waitFor(() => expect(ipc.count("transitions_retime")).toBe(1));
    expect(ipc.lastCall("transitions_retime")).toEqual({
      segmentId: "right",
      duration: 2_000_000,
    });
  });

  it("clamps the drag to what the two clips can carry", async () => {
    const project = cutProject(transition());
    ipc.handle("transitions_retime", makeEditResponse(project));
    open(project);

    // Ten seconds back from a cut at 4 s, between two four-second clips: the
    // most they can carry is eight.
    drag(screen.getByLabelText("Transition into right"), 3_500_000, -6_000_000);

    await waitFor(() => expect(ipc.count("transitions_retime")).toBe(1));
    expect(ipc.lastCall("transitions_retime")?.duration).toBe(8_000_000);
  });

  it("removes the transition when the drag reaches the cut", async () => {
    const project = cutProject(transition());
    ipc.handle("transitions_remove", makeEditResponse(project));
    open(project);

    drag(screen.getByLabelText("Transition into right"), 3_500_000, 4_000_000);

    await waitFor(() => expect(ipc.count("transitions_remove")).toBe(1));
    expect(ipc.lastCall("transitions_remove")).toEqual({ segmentId: "right" });
    expect(ipc.count("transitions_retime")).toBe(0);
  });

  it("selects rather than retimes when the pointer does not move", async () => {
    const project = cutProject(transition());
    open(project);

    const marker = screen.getByLabelText("Transition into right");
    fireEvent.pointerDown(marker, { pointerId: 1, button: 0, clientX: 400 });
    fireEvent.pointerUp(window, { pointerId: 1, clientX: 400 });

    await waitFor(() => expect(useTransitionStore.getState().selectedSegmentId).toBe("right"));
    expect(ipc.count("transitions_retime")).toBe(0);
    expect(ipc.count("transitions_remove")).toBe(0);
  });

  it("deletes on a double click", async () => {
    const project = cutProject(transition());
    ipc.handle("transitions_remove", makeEditResponse(project));
    open(project);

    fireEvent.doubleClick(screen.getByLabelText("Transition into right"));

    await waitFor(() => expect(ipc.count("transitions_remove")).toBe(1));
  });
});

describe("lanes that take no transitions", () => {
  it("draws nothing on an audio lane", () => {
    open(cutProject(undefined, "audio"));
    expect(screen.queryByLabelText(/Add a transition/)).toBeNull();
  });

  it("draws nothing where two clips do not touch", () => {
    const project = cutProject();
    project.tracks[0].segments[0].target_range.duration = 3_000_000;
    open(project);
    expect(screen.queryByLabelText(/Add a transition/)).toBeNull();
  });
});
