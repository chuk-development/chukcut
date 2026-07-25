/**
 * The inspector's sliders.
 *
 * The contract worth defending is the shape of the gesture, not the arithmetic:
 * dragging is local, releasing is one edit, and the edit carries the value the
 * property had *before* the drag so Rust can invert it. Ten commands from one
 * drag would put ten steps on the undo stack; a `before` sampled after the fact
 * would make undo a no-op.
 *
 * Driven through Radix's real pointer path rather than by calling the props, so
 * a change to how the slider is wired up shows up here.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { Inspector } from "@/modules/inspector/components/Inspector";
import { useProjectStore } from "@/modules/project/store";
import type { Project } from "@/modules/project/types";
import { useTimelineStore } from "@/modules/timeline/store";
import { stubRect } from "@/test/dom";
import {
  IDENTITY_TRANSFORM,
  makeEditResponse,
  makeSegment,
  makeTrack,
  projectWithSegments,
} from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

const TRACK_WIDTH = 200;

/** Grab a slider by its label and give it a real box to measure against. */
function slider(label: string): HTMLElement {
  const element = document.querySelector<HTMLElement>(`[aria-label="${label}"]`);
  if (!element) throw new Error(`no slider labelled "${label}"`);
  stubRect(element, { left: 0, width: TRACK_WIDTH });
  return element;
}

/** Press at `from`, move to `to`, release — the fractions are of the track's width. */
function drag(element: HTMLElement, from: number, to: number): void {
  fireEvent.pointerDown(element, { pointerId: 1, button: 0, clientX: from * TRACK_WIDTH });
  fireEvent.pointerMove(element, { pointerId: 1, clientX: to * TRACK_WIDTH });
  fireEvent.pointerUp(element, { pointerId: 1, clientX: to * TRACK_WIDTH });
}

function open(project: Project, selected: string | null = "segment-1") {
  useProjectStore.getState().loadProject(project);
  useTimelineStore.getState().select(selected);
  return render(<Inspector />);
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  useTimelineStore.getState().select(null);
  useProjectStore.setState({ project: null, error: null });
});

afterEach(() => {
  ipc.restore();
});

describe("a slider drag", () => {
  it("sends nothing until the thumb is released", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);
    const opacity = slider("Opacity");

    fireEvent.pointerDown(opacity, { pointerId: 1, button: 0, clientX: 0.25 * TRACK_WIDTH });
    for (let x = 25; x <= 150; x += 5) {
      fireEvent.pointerMove(opacity, { pointerId: 1, clientX: x });
    }

    // Twenty-six positions, twenty-six edits, twenty-six undo steps.
    expect(ipc.count("timeline_apply")).toBe(0);
  });

  it("shows the value being dragged even though the document has not changed", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);
    const opacity = slider("Opacity");
    const readout = () => opacity.closest('[data-slot="property-slider"]')?.lastElementChild;
    expect(readout()).toHaveTextContent("100%");

    fireEvent.pointerDown(opacity, { pointerId: 1, button: 0, clientX: 0.25 * TRACK_WIDTH });

    // The readout follows the thumb; the document is stale until release.
    expect(readout()).toHaveTextContent("25%");
    expect(ipc.count("timeline_apply")).toBe(0);
  });

  it("sends exactly one command when the thumb is released", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    drag(slider("Opacity"), 0.25, 0.75);

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(ipc.lastCall("timeline_apply")?.command).toEqual({
      type: "set_transform",
      segment_id: "segment-1",
      before: IDENTITY_TRANSFORM,
      after: { ...IDENTITY_TRANSFORM, opacity: 0.75 },
    });
  });

  it("carries the value from before the drag, so undo puts it back", () => {
    // The document said 0.4 when the gesture started; anything else in `before`
    // makes the inverse command restore the wrong value.
    const project = projectWithSegments(
      makeSegment("segment-1", { transform: { ...IDENTITY_TRANSFORM, opacity: 0.4 } }),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    drag(slider("Opacity"), 0.4, 0.75);

    expect(ipc.lastCall("timeline_apply")?.command).toMatchObject({
      before: { opacity: 0.4 },
      after: { opacity: 0.75 },
    });
  });

  it("sends nothing when the thumb is released where it started", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);
    const opacity = slider("Opacity");

    // Full opacity is the right-hand end of the track.
    fireEvent.pointerDown(opacity, { pointerId: 1, button: 0, clientX: TRACK_WIDTH });
    fireEvent.pointerUp(opacity, { pointerId: 1, clientX: TRACK_WIDTH });

    expect(ipc.count("timeline_apply")).toBe(0);
  });

  it("changes only the axis that was dragged", () => {
    const project = projectWithSegments(
      makeSegment("segment-1", { transform: { ...IDENTITY_TRANSFORM, position: [0.5, -0.25] } }),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    // Position X runs -2..2, so three quarters along is 1.
    drag(slider("Position X"), 0.5, 0.75);

    expect(ipc.lastCall("timeline_apply")?.command).toMatchObject({
      before: { position: [0.5, -0.25] },
      after: { position: [1, -0.25] },
    });
  });

  it("scales both axes together, because the control is a single scale", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    drag(slider("Scale"), 0.25, 0.5);

    const command = ipc.lastCall("timeline_apply")?.command as { after: { scale: number[] } };
    expect(command.after.scale[0]).toBe(command.after.scale[1]);
    expect(ipc.count("timeline_apply")).toBe(1);
  });
});

describe("the other clip properties", () => {
  it("sends a set_speed command of its own, not a transform", () => {
    const project = projectWithSegments(makeSegment("segment-1", { speed: 1 }));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    drag(slider("Speed"), 0.25, 0.5);

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(ipc.lastCall("timeline_apply")?.command).toMatchObject({
      type: "set_speed",
      segment_id: "segment-1",
      before: 1,
    });
  });

  it("sends a set_volume command of its own", () => {
    const project = projectWithSegments(makeSegment("segment-1", { volume: 1 }));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    drag(slider("Volume"), 0.5, 0.25);

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(ipc.lastCall("timeline_apply")?.command).toMatchObject({
      type: "set_volume",
      segment_id: "segment-1",
      before: 1,
      after: 0.5,
    });
  });

  it("refuses to offer volume on a lane that has no audio", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    project.tracks[0] = makeTrack("track-video", {
      kind: "text",
      segments: project.tracks[0].segments,
    });
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    const volume = slider("Volume");
    expect(volume).toHaveAttribute("data-disabled");

    drag(volume, 0.5, 0.25);
    expect(ipc.count("timeline_apply")).toBe(0);
  });
});

describe("the transform buttons", () => {
  it("flips one axis per click and leaves the rest of the transform alone", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    fireEvent.click(screen.getByRole("button", { name: /flip h/i }));

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(ipc.lastCall("timeline_apply")?.command).toEqual({
      type: "set_transform",
      segment_id: "segment-1",
      before: IDENTITY_TRANSFORM,
      after: { ...IDENTITY_TRANSFORM, flip_h: true },
    });
  });

  it("resets every field of the transform in a single undoable step", () => {
    const moved = {
      position: [0.5, 0.5] as [number, number],
      scale: [2, 2] as [number, number],
      rotation: 45,
      opacity: 0.3,
      flip_h: true,
      flip_v: true,
    };
    const project = projectWithSegments(makeSegment("segment-1", { transform: moved }));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    fireEvent.click(screen.getByRole("button", { name: /reset/i }));

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(ipc.lastCall("timeline_apply")?.command).toEqual({
      type: "set_transform",
      segment_id: "segment-1",
      before: moved,
      after: IDENTITY_TRANSFORM,
    });
  });
});

describe("with nothing selected", () => {
  it("describes the project and offers no clip controls", () => {
    open(projectWithSegments(makeSegment("segment-1")), null);

    expect(screen.getByText("Untitled")).toBeInTheDocument();
    expect(screen.getByText("1080 × 1920")).toBeInTheDocument();
    expect(document.querySelector('[aria-label="Opacity"]')).toBeNull();
    expect(ipc.log).toEqual([]);
  });

  it("says so when there is no project at all", () => {
    render(<Inspector />);

    expect(screen.getByText("No project open")).toBeInTheDocument();
    expect(ipc.log).toEqual([]);
  });
});
