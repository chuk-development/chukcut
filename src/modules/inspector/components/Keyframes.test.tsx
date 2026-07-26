/**
 * Keyframing, from the inspector.
 *
 * What is worth defending here is not the arithmetic — `keyframes.test.ts` has
 * that — but what a gesture *means* once a property is animated. Dragging a
 * slider on a static property writes the document; dragging the same slider on
 * an animated one writes a keyframe, because writing the document would appear
 * to do nothing: animation overrides the static value on the very next frame.
 *
 * And, as everywhere else in this app, one gesture is one command. A keyframe
 * dragged across the curve leaves as a single `move_keyframe` on release, not
 * as a command per pointer move.
 */

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { Inspector } from "@/modules/inspector/components/Inspector";
import { useInspectorStore } from "@/modules/inspector/store";
import { useProjectStore } from "@/modules/project/store";
import type {
  AnimatableProperty,
  Easing,
  Keyframe,
  KeyframeTrack,
  Project,
  Segment,
} from "@/modules/project/types";
import { useTimelineStore } from "@/modules/timeline/store";
import { stubRect } from "@/test/dom";
import { makeEditResponse, makeSegment, projectWithSegments } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

/** The clip starts a second in, so nothing can pass by treating clip time as timeline time. */
const CLIP_START = 1_000_000;
const CLIP_DURATION = 4_000_000;

const SLIDER_WIDTH = 200;

function key(time: number, value: number, easing: Easing = "linear"): Keyframe {
  return { time, value, easing };
}

function track(property: AnimatableProperty, ...keyframes: Keyframe[]): KeyframeTrack {
  return { property, keyframes };
}

function clip(...keyframes: KeyframeTrack[]): Segment {
  return makeSegment("segment-1", {
    target_range: { start: CLIP_START, duration: CLIP_DURATION },
    source_range: { start: 0, duration: CLIP_DURATION },
    keyframes,
  });
}

function open(project: Project, playhead: number) {
  useProjectStore.getState().loadProject(project);
  useTimelineStore.getState().select("segment-1");
  useTimelineStore.getState().setPlayhead(playhead);
  return render(<Inspector />);
}

/** Grab a slider by its label and give it a real box to measure against. */
function slider(label: string): HTMLElement {
  const element = document.querySelector<HTMLElement>(`[aria-label="${label}"]`);
  if (!element) throw new Error(`no slider labelled "${label}"`);
  stubRect(element, { left: 0, width: SLIDER_WIDTH });
  return element;
}

function drag(element: HTMLElement, from: number, to: number): void {
  fireEvent.pointerDown(element, { pointerId: 1, button: 0, clientX: from * SLIDER_WIDTH });
  fireEvent.pointerMove(element, { pointerId: 1, clientX: to * SLIDER_WIDTH });
  fireEvent.pointerUp(element, { pointerId: 1, clientX: to * SLIDER_WIDTH });
}

function readout(label: string): string {
  const element = document.querySelector(`[aria-label="${label}"]`);
  return element?.closest('[data-slot="property-slider"]')?.lastElementChild?.textContent ?? "";
}

function lastCommand(): Record<string, unknown> {
  return ipc.lastCall("timeline_apply")?.command as Record<string, unknown>;
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  useTimelineStore.getState().select(null);
  useTimelineStore.getState().setPlayhead(0);
  useProjectStore.setState({ project: null, error: null });
  useInspectorStore.setState({ curveProperty: null, selectedKeyframeTime: null });
});

afterEach(() => {
  ipc.restore();
});

describe("the value shown for an animated property", () => {
  it("is the value at the playhead, not the one in the document", () => {
    const project = projectWithSegments(clip(track("opacity", key(0, 0), key(2_000_000, 1))));
    ipc.handle("timeline_apply", makeEditResponse(project));

    // One second into a clip that fades in over two: half way.
    open(project, CLIP_START + 1_000_000);

    expect(readout("Opacity")).toBe("50%");
  });

  it("follows the playhead as it scrubs", () => {
    const project = projectWithSegments(clip(track("opacity", key(0, 0), key(2_000_000, 1))));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);
    expect(readout("Opacity")).toBe("0%");

    act(() => useTimelineStore.getState().setPlayhead(CLIP_START + 1_500_000));
    expect(readout("Opacity")).toBe("75%");

    // Past the last keyframe the value is held, never extrapolated.
    act(() => useTimelineStore.getState().setPlayhead(CLIP_START + 3_000_000));
    expect(readout("Opacity")).toBe("100%");
  });
});

describe("the keyframe toggle", () => {
  it("starts animating a property at the playhead with the value it has now", () => {
    const segment = clip();
    segment.transform = { ...segment.transform, opacity: 0.4 };
    const project = projectWithSegments(segment);
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START + 1_000_000);

    fireEvent.click(screen.getByRole("button", { name: "Animate Opacity" }));

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(lastCommand()).toEqual({
      type: "add_keyframe",
      segment_id: "segment-1",
      property: "opacity",
      // Segment-relative: the clip starts a second into the timeline.
      keyframe: { time: 1_000_000, value: 0.4, easing: "linear" },
    });
  });

  it("removes the keyframe under the playhead, carrying it whole so undo can restore it", () => {
    const project = projectWithSegments(
      clip(track("opacity", key(0, 0.25, "ease_in"), key(2_000_000, 1))),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);

    fireEvent.click(screen.getByRole("button", { name: "Remove Opacity keyframe" }));

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(lastCommand()).toEqual({
      type: "remove_keyframe",
      segment_id: "segment-1",
      property: "opacity",
      keyframe: { time: 0, value: 0.25, easing: "ease_in" },
    });
  });

  it("adds one at the sampled value when the playhead is between keyframes", () => {
    const project = projectWithSegments(
      clip(track("opacity", key(0, 0, "ease_in_out"), key(2_000_000, 1))),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START + 1_000_000);

    fireEvent.click(screen.getByRole("button", { name: "Add Opacity keyframe" }));

    // Half way along an ease-in-out is half way up, and the new keyframe
    // inherits the easing of the span it landed in rather than straightening it.
    expect(lastCommand()).toEqual({
      type: "add_keyframe",
      segment_id: "segment-1",
      property: "opacity",
      keyframe: { time: 1_000_000, value: 0.5, easing: "ease_in_out" },
    });
  });

  it("refuses to keyframe while the playhead is off the clip", () => {
    const project = projectWithSegments(clip());
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, 0);

    const toggle = screen.getByRole("button", { name: "Animate Opacity" });
    expect(toggle).toBeDisabled();
    fireEvent.click(toggle);

    expect(ipc.count("timeline_apply")).toBe(0);
  });

  it("keyframes both scale axes in one command, because it is one control", () => {
    const project = projectWithSegments(clip());
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START + 2_000_000);

    fireEvent.click(screen.getByRole("button", { name: "Animate Scale" }));

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(lastCommand()).toEqual({
      type: "composite",
      label: "Add keyframe",
      commands: [
        {
          type: "add_keyframe",
          segment_id: "segment-1",
          property: "scale_x",
          keyframe: { time: 2_000_000, value: 1, easing: "linear" },
        },
        {
          type: "add_keyframe",
          segment_id: "segment-1",
          property: "scale_y",
          keyframe: { time: 2_000_000, value: 1, easing: "linear" },
        },
      ],
    });
  });
});

describe("editing a value while the property is animated", () => {
  it("creates a keyframe at the playhead when it sits between two", () => {
    const project = projectWithSegments(clip(track("opacity", key(0, 0), key(2_000_000, 1))));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START + 1_000_000);

    drag(slider("Opacity"), 0.5, 0.75);

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(lastCommand()).toEqual({
      type: "add_keyframe",
      segment_id: "segment-1",
      property: "opacity",
      keyframe: { time: 1_000_000, value: 0.75, easing: "linear" },
    });
  });

  it("edits the keyframe in place when the playhead is on one", () => {
    const project = projectWithSegments(
      clip(track("opacity", key(0, 0.2, "ease_in"), key(2_000_000, 1))),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);

    drag(slider("Opacity"), 0.2, 0.6);

    expect(ipc.count("timeline_apply")).toBe(1);
    // Same time on both sides: a value edit, not a retime. The easing is not
    // part of the command, so shaping is not lost by changing a value.
    expect(lastCommand()).toEqual({
      type: "move_keyframe",
      segment_id: "segment-1",
      property: "opacity",
      from_time: 0,
      to_time: 0,
      before_value: 0.2,
      after_value: 0.6,
    });
  });

  it("still writes the document when nothing is animated", () => {
    const project = projectWithSegments(clip());
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);

    drag(slider("Opacity"), 1, 0.5);

    expect(lastCommand()).toMatchObject({ type: "set_transform", segment_id: "segment-1" });
  });

  it("sends one command for the drag, not one per pointer move", () => {
    const project = projectWithSegments(clip(track("opacity", key(0, 0), key(2_000_000, 1))));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START + 1_000_000);
    const opacity = slider("Opacity");

    fireEvent.pointerDown(opacity, { pointerId: 1, button: 0, clientX: 0.5 * SLIDER_WIDTH });
    for (let x = 100; x <= 180; x += 5) {
      fireEvent.pointerMove(opacity, { pointerId: 1, clientX: x });
    }
    expect(ipc.count("timeline_apply")).toBe(0);

    fireEvent.pointerUp(opacity, { pointerId: 1, clientX: 180 });
    expect(ipc.count("timeline_apply")).toBe(1);
  });
});

describe("navigating between keyframes", () => {
  it("moves the playhead onto the next keyframe, in timeline time", () => {
    const project = projectWithSegments(
      clip(track("opacity", key(0, 0), key(2_000_000, 1), key(3_000_000, 0))),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);

    fireEvent.click(screen.getByRole("button", { name: "Next Opacity keyframe" }));

    expect(useTimelineStore.getState().playhead).toBe(CLIP_START + 2_000_000);
    // And the toggle now says it is standing on one.
    expect(screen.getByRole("button", { name: "Remove Opacity keyframe" })).toHaveAttribute(
      "data-on-keyframe",
      "true",
    );
  });

  it("goes back, and stops at the ends", () => {
    const project = projectWithSegments(clip(track("opacity", key(0, 0), key(2_000_000, 1))));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START + 2_000_000);

    expect(screen.getByRole("button", { name: "Next Opacity keyframe" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Previous Opacity keyframe" }));

    expect(useTimelineStore.getState().playhead).toBe(CLIP_START);
    expect(screen.getByRole("button", { name: "Previous Opacity keyframe" })).toBeDisabled();
    expect(ipc.count("timeline_apply")).toBe(0);
  });

  it("says nothing about keyframes for a property that has none", () => {
    const project = projectWithSegments(clip(track("opacity", key(0, 0))));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);

    expect(screen.queryByRole("button", { name: "Next Rotation keyframe" })).toBeNull();
    expect(screen.getByRole("button", { name: "Animate Rotation" })).toBeInTheDocument();
  });
});

describe("the curve", () => {
  /** The curve measures itself; jsdom reports zeros without this. */
  function curve(): HTMLElement {
    const element = document.querySelector<HTMLElement>('[data-slot="keyframe-curve"]');
    if (!element) throw new Error("the curve view is not showing");
    element.getBoundingClientRect = () =>
      ({
        x: 0,
        y: 0,
        left: 0,
        top: 0,
        right: 200,
        bottom: 100,
        width: 200,
        height: 100,
        toJSON: () => ({}),
      }) as DOMRect;
    return element;
  }

  /** Plot x for a fraction of the segment, in the box `curve()` stubs. */
  function plotX(fraction: number): number {
    return (4 + fraction * 92) * 2;
  }

  it("sends one command when a keyframe is dragged, on release", () => {
    const project = projectWithSegments(clip(track("opacity", key(0, 0), key(2_000_000, 1))));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);
    curve();

    const handle = screen.getByRole("button", { name: "Opacity keyframe at 00:00:00" });
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: plotX(0), clientY: 90 });
    for (let x = plotX(0); x <= plotX(0.25); x += 4) {
      fireEvent.pointerMove(handle, { pointerId: 1, clientX: x, clientY: 70 });
    }
    expect(ipc.count("timeline_apply")).toBe(0);

    fireEvent.pointerMove(handle, { pointerId: 1, clientX: plotX(0.25), clientY: 50 });
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: plotX(0.25), clientY: 50 });

    expect(ipc.count("timeline_apply")).toBe(1);
    // A quarter along a four-second clip, and half way up a 0..1 curve whose
    // axis is padded by a fifth either side.
    expect(lastCommand()).toEqual({
      type: "move_keyframe",
      segment_id: "segment-1",
      property: "opacity",
      from_time: 0,
      to_time: 1_000_000,
      before_value: 0,
      after_value: 0.5,
    });
  });

  it("sends nothing for a click that does not move a keyframe", () => {
    const project = projectWithSegments(clip(track("opacity", key(0, 0), key(2_000_000, 1))));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);
    curve();

    const handle = screen.getByRole("button", { name: "Opacity keyframe at 00:00:00" });
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: plotX(0), clientY: 90 });
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: plotX(0), clientY: 90 });

    expect(ipc.count("timeline_apply")).toBe(0);
    expect(handle).toHaveAttribute("data-selected", "true");
  });

  it("keeps a dragged keyframe between its neighbours", () => {
    const project = projectWithSegments(
      clip(track("opacity", key(0, 0), key(1_000_000, 0.5), key(2_000_000, 1))),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);
    curve();

    // Drag the middle keyframe past the last one; it stops a frame short of it,
    // so the track Rust receives is still sorted.
    const handle = screen.getByRole("button", { name: "Opacity keyframe at 00:01:00" });
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: plotX(0.25), clientY: 50 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: plotX(0.95), clientY: 50 });
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: plotX(0.95), clientY: 50 });

    expect(lastCommand()).toMatchObject({ from_time: 1_000_000, to_time: 2_000_000 - 33_333 });
  });
});

describe("the easing picker", () => {
  it("shows the easing of the keyframe under the playhead", () => {
    const project = projectWithSegments(
      clip(track("opacity", key(0, 0, "ease_in_out"), key(2_000_000, 1))),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);

    expect(screen.getByRole("button", { name: "Easing In-out" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "Easing Linear" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("changes it in one command, carrying the easing it replaces", () => {
    const project = projectWithSegments(clip(track("opacity", key(0, 0), key(2_000_000, 1))));
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);

    fireEvent.click(screen.getByRole("button", { name: "Easing Hold" }));

    expect(ipc.count("timeline_apply")).toBe(1);
    expect(lastCommand()).toEqual({
      type: "set_keyframe_easing",
      segment_id: "segment-1",
      property: "opacity",
      time: 0,
      before: "linear",
      after: "hold",
    });
  });

  it("round-trips: the document comes back and the picker reflects it", () => {
    const before = projectWithSegments(clip(track("opacity", key(0, 0), key(2_000_000, 1))));
    const after = projectWithSegments(
      clip(track("opacity", key(0, 0, "ease_out"), key(2_000_000, 1))),
    );
    ipc.handle("timeline_apply", makeEditResponse(after));
    open(before, CLIP_START);

    fireEvent.click(screen.getByRole("button", { name: "Easing Out" }));
    // The store is replaced by Rust's answer, which is where the UI reads from.
    act(() => useProjectStore.getState().applyEditResponse(makeEditResponse(after)));

    expect(screen.getByRole("button", { name: "Easing Out" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });
});

describe("removing animation", () => {
  it("deletes every keyframe of the property in one undoable step", () => {
    const project = projectWithSegments(
      clip(track("opacity", key(0, 0), key(1_000_000, 1), key(2_000_000, 0))),
    );
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);

    fireEvent.click(screen.getByRole("button", { name: "Remove Opacity animation" }));

    expect(ipc.count("timeline_apply")).toBe(1);
    const command = lastCommand() as { type: string; label: string; commands: unknown[] };
    expect(command.type).toBe("composite");
    expect(command.label).toBe("Remove animation");
    expect(command.commands).toHaveLength(3);
  });

  it("offers nothing to edit when the clip has no animation", () => {
    const project = projectWithSegments(clip());
    ipc.handle("timeline_apply", makeEditResponse(project));
    open(project, CLIP_START);

    expect(document.querySelector('[data-slot="keyframe-curve"]')).toBeNull();
    expect(screen.getByText(/Nothing on this clip is animated/)).toBeInTheDocument();
  });
});
