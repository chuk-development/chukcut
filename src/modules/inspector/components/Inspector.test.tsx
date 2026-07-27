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

    // Exact-match: "Reset crop" and "Reset colour" are different buttons.
    fireEvent.click(screen.getByRole("button", { name: /^reset$/i }));

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(ipc.lastCall("timeline_apply")?.command).toEqual({
      type: "set_transform",
      segment_id: "segment-1",
      before: moved,
      after: IDENTITY_TRANSFORM,
    });
  });
});

describe("the rotate buttons", () => {
  it("turns a quarter clockwise through the ordinary transform edit", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    fireEvent.click(screen.getByRole("button", { name: /rotate right/i }));

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(ipc.lastCall("timeline_apply")?.command).toEqual({
      type: "set_transform",
      segment_id: "segment-1",
      before: IDENTITY_TRANSFORM,
      after: { ...IDENTITY_TRANSFORM, rotation: 90 },
    });
  });

  it("wraps rather than accumulating, so the slider can always show the angle", () => {
    const project = projectWithSegments(
      makeSegment("segment-1", { transform: { ...IDENTITY_TRANSFORM, rotation: 135 } }),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project);

    fireEvent.click(screen.getByRole("button", { name: /rotate right/i }));

    expect(ipc.lastCall("timeline_apply")?.command).toMatchObject({
      after: { rotation: -135 },
    });
  });
});

describe("the crop panel", () => {
  it("commits one inspector_set_crop per release, carrying the whole rectangle", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("inspector_set_crop", makeEditResponse(project));
    open(project);

    // The inset sliders run 0..MAX_INSET (0.45); 40% along is an inset of 0.18.
    drag(slider("Crop left"), 0, 0.4);

    expect(ipc.count("inspector_set_crop")).toBe(1);
    const payload = ipc.lastCall("inspector_set_crop") as {
      segmentId: string;
      crop: { left: number; top: number; right: number; bottom: number };
    };
    expect(payload.segmentId).toBe("segment-1");
    expect(payload.crop.left).toBeCloseTo(0.18, 5);
    expect(payload.crop.top).toBe(0);
    expect(payload.crop.right).toBe(1);
    expect(payload.crop.bottom).toBe(1);
  });

  it("keeps the other edges while one is dragged", () => {
    const project = projectWithSegments(
      makeSegment("segment-1", { crop: { left: 0.1, top: 0.2, right: 0.9, bottom: 1 } }),
    );
    ipc.handle("inspector_set_crop", makeEditResponse(project));
    open(project);

    drag(slider("Crop bottom"), 0, 0.4);

    const payload = ipc.lastCall("inspector_set_crop") as { crop: Record<string, number> };
    expect(payload.crop.left).toBeCloseTo(0.1, 5);
    expect(payload.crop.top).toBeCloseTo(0.2, 5);
    expect(payload.crop.right).toBeCloseTo(0.9, 5);
    expect(payload.crop.bottom).toBeCloseTo(0.82, 5);
  });

  it("resets by sending null, and only offers reset once there is a crop", () => {
    const uncropped = projectWithSegments(makeSegment("segment-1"));
    const { unmount } = open(uncropped);
    expect(screen.getByRole("button", { name: /reset crop/i })).toBeDisabled();
    unmount();

    const cropped = projectWithSegments(
      makeSegment("segment-1", { crop: { left: 0.25, top: 0, right: 0.75, bottom: 1 } }),
    );
    ipc.handle("inspector_set_crop", makeEditResponse(cropped));
    open(cropped);

    fireEvent.click(screen.getByRole("button", { name: /reset crop/i }));

    expect(ipc.lastCall("inspector_set_crop")).toMatchObject({
      segmentId: "segment-1",
      crop: null,
    });
  });
});

describe("the colour panel", () => {
  it("commits the whole grade with the dragged field changed", () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("inspector_set_color", makeEditResponse(project));
    open(project);

    // Brightness runs -1..1; three quarters along is +0.5.
    drag(slider("Brightness"), 0.5, 0.75);

    expect(ipc.count("inspector_set_color")).toBe(1);
    expect(ipc.lastCall("inspector_set_color")).toEqual({
      segmentId: "segment-1",
      color: { brightness: 0.5, contrast: 1, saturation: 1, temperature: 0, lut: null },
    });
  });

  it("starts from the applied grade rather than from identity", () => {
    const project = projectWithSegments(makeSegment("segment-1", { extras: ["grade-1"] }));
    project.materials.color_adjusts = [
      {
        id: "grade-1",
        brightness: 0.2,
        contrast: 1.4,
        saturation: 1,
        temperature: -0.3,
        lut: null,
      },
    ];
    ipc.handle("inspector_set_color", makeEditResponse(project));
    open(project);

    // Saturation runs 0..2; a quarter along is 0.5.
    drag(slider("Saturation"), 0.5, 0.25);

    expect(ipc.lastCall("inspector_set_color")).toEqual({
      segmentId: "segment-1",
      color: { brightness: 0.2, contrast: 1.4, saturation: 0.5, temperature: -0.3, lut: null },
    });
  });

  it("picks a LUT through the dialog, probes it, and commits it at full intensity", async () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("plugin:dialog|open", "/looks/warm.cube");
    ipc.handle("inspector_lut_probe", { title: "Warm Look", size: 33 });
    ipc.handle("inspector_set_color", makeEditResponse(project));
    open(project);

    fireEvent.click(screen.getByRole("button", { name: /choose lut/i }));
    // Let the async dialog → probe → commit chain settle.
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(ipc.count("inspector_lut_probe")).toBe(1);
    expect(ipc.lastCall("inspector_lut_probe")).toEqual({ path: "/looks/warm.cube" });
    expect(ipc.count("inspector_set_color")).toBe(1);
    expect(ipc.lastCall("inspector_set_color")).toEqual({
      segmentId: "segment-1",
      color: {
        brightness: 0,
        contrast: 1,
        saturation: 1,
        temperature: 0,
        lut: { path: "/looks/warm.cube", intensity: 1 },
      },
    });
  });

  it("refuses a malformed LUT at pick time and never touches the document", async () => {
    const project = projectWithSegments(makeSegment("segment-1"));
    ipc.handle("plugin:dialog|open", "/looks/broken.cube");
    ipc.fail("inspector_lut_probe", "line 3: a data line needs three numbers");
    open(project);

    fireEvent.click(screen.getByRole("button", { name: /choose lut/i }));
    // Let the async pick settle.
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(ipc.count("inspector_set_color")).toBe(0);
    expect(useProjectStore.getState().error).toContain("line 3");
  });

  it("shows an applied LUT with its intensity, and drags commit the blend", () => {
    const project = projectWithSegments(makeSegment("segment-1", { extras: ["grade-1"] }));
    project.materials.color_adjusts = [
      {
        id: "grade-1",
        brightness: 0,
        contrast: 1,
        saturation: 1,
        temperature: 0,
        lut: { path: "/looks/warm.cube", intensity: 1 },
      },
    ];
    ipc.handle("inspector_set_color", makeEditResponse(project));
    open(project);

    expect(screen.getByText("warm.cube")).toBeInTheDocument();

    drag(slider("LUT intensity"), 1, 0.5);

    expect(ipc.lastCall("inspector_set_color")).toEqual({
      segmentId: "segment-1",
      color: {
        brightness: 0,
        contrast: 1,
        saturation: 1,
        temperature: 0,
        lut: { path: "/looks/warm.cube", intensity: 0.5 },
      },
    });
  });

  it("removes the LUT while keeping the sliders' grade", () => {
    const project = projectWithSegments(makeSegment("segment-1", { extras: ["grade-1"] }));
    project.materials.color_adjusts = [
      {
        id: "grade-1",
        brightness: 0.2,
        contrast: 1,
        saturation: 1,
        temperature: 0,
        lut: { path: "/looks/warm.cube", intensity: 0.7 },
      },
    ];
    ipc.handle("inspector_set_color", makeEditResponse(project));
    open(project);

    fireEvent.click(screen.getByRole("button", { name: /remove lut/i }));

    expect(ipc.lastCall("inspector_set_color")).toEqual({
      segmentId: "segment-1",
      color: { brightness: 0.2, contrast: 1, saturation: 1, temperature: 0, lut: null },
    });
  });

  it("resets by sending null, and only offers reset once there is a grade", () => {
    const ungraded = projectWithSegments(makeSegment("segment-1"));
    const { unmount } = open(ungraded);
    expect(screen.getByRole("button", { name: /reset colour/i })).toBeDisabled();
    unmount();

    const graded = projectWithSegments(makeSegment("segment-1", { extras: ["grade-1"] }));
    graded.materials.color_adjusts = [
      { id: "grade-1", brightness: 0.2, contrast: 1, saturation: 1, temperature: 0, lut: null },
    ];
    ipc.handle("inspector_set_color", makeEditResponse(graded));
    open(graded);

    fireEvent.click(screen.getByRole("button", { name: /reset colour/i }));

    expect(ipc.lastCall("inspector_set_color")).toMatchObject({
      segmentId: "segment-1",
      color: null,
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
