/**
 * The repaint budget, as a test.
 *
 * A timeline is a component tree that receives a stream of pointer events, and
 * the naive version of that is quadratic in the worst way: every pointer move
 * during a drag sets state on the timeline, the timeline re-renders, and every
 * clip on it recomputes its filmstrip tiles and its waveform columns. At fifty
 * clips and sixty events a second that is three thousand tile computations a
 * second to move one rectangle.
 *
 * Three things stop it, and each has a test here because each is one careless
 * edit away from being undone:
 *
 * - `Segment` is wrapped in `React.memo`, and every prop it receives is
 *   reference-stable between renders. An arrow function in the timeline's JSX,
 *   or a descriptor object built inside the render, silently defeats it.
 * - The razor's position lives in the store, not in timeline state, so the
 *   pointer moves that drive it never re-render the lanes at all.
 * - The visible window handed to the clips is snapped to a pixel grid, so
 *   scrolling changes it once per grid step rather than once per wheel tick.
 *
 * `clipPaintCount` is the instrument: it counts paints from *inside* the memo,
 * which is the only place the difference is observable.
 */

import { act, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { useProjectStore } from "@/modules/project/store";
import type { Project, Segment } from "@/modules/project/types";
import { clipPaintCount } from "@/modules/timeline/components/Segment";
import { bodyPaintCount, Timeline } from "@/modules/timeline/components/Timeline";
import { useTimelineStore } from "@/modules/timeline/store";
import { makeProject, makeSegment, makeTrack, range } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

const SECOND = 1_000_000;
const CLIPS = 50;

// Fifty clips is a lot of DOM for jsdom to build, and this file mounts it six
// times. The default five seconds is a limit on the machine, not on the code
// under test.
vi.setConfig({ testTimeout: 30_000 });

/** Clips of two seconds each, back to back, all from real video material. */
function crowdedProject(count = CLIPS): Project {
  const segments: Segment[] = [];
  for (let i = 0; i < count; i++) {
    segments.push(
      makeSegment(`clip-${i}`, {
        material_id: `material-${i}`,
        target_range: range(i * 2 * SECOND, 2 * SECOND),
        source_range: range(0, 2 * SECOND),
      }),
    );
  }
  return makeProject({
    materials: {
      videos: segments.map((segment) => ({
        id: segment.material_id,
        path: `/media/${segment.material_id}.mp4`,
        width: 1920,
        height: 1080,
        duration: 20 * SECOND,
        fps: 30,
        has_audio: true,
        rotation: 0,
      })),
      audios: [],
      images: [],
      texts: [],
      links: [],
      transitions: [],
      extras: {},
    },
    tracks: [makeTrack("track-video", { segments })],
  });
}

/** Paints since the last call, per clip. */
function paintsSince(before: Map<string, number>): Map<string, number> {
  const delta = new Map<string, number>();
  for (const [id, count] of clipPaintCount) {
    const previous = before.get(id) ?? 0;
    if (count > previous) delta.set(id, count - previous);
  }
  return delta;
}

function snapshot(): Map<string, number> {
  return new Map(clipPaintCount);
}

/** The toolbar's buttons carry tooltips, which Radix refuses to render unprovided. */
function mount() {
  return render(
    <TooltipProvider>
      <Timeline />
    </TooltipProvider>,
  );
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  // Both caches answer with nothing: this test is about how often the timeline
  // repaints, not about what ends up drawn.
  ipc.handle("media_thumbnails", "job-1");
  ipc.handle("media_waveform", { buckets: 0, duration: 0, min: [], max: [], rms: [] });
  ipc.handle("preview_seek", null);
  ipc.handle("timeline_apply", () => Promise.reject("not part of this test"));
  useProjectStore.setState({ project: crowdedProject(), status: "ready" });
  useTimelineStore.setState({ zoom: 1e-4, scrollX: 0, tool: "select", razorTarget: null });
  clipPaintCount.clear();
});

afterEach(() => {
  ipc.restore();
  useProjectStore.setState({ project: null });
  vi.clearAllMocks();
});

describe("dragging one clip on a crowded timeline", () => {
  it("repaints that clip and no others", () => {
    mount();
    expect(clipPaintCount.size).toBe(CLIPS);

    const before = snapshot();
    // Queried directly rather than by role: computing an accessible name for
    // every button on fifty clips costs more than the drag being measured.
    const clip = document
      .querySelectorAll('[data-slot="segment"]')[0]
      ?.querySelector<HTMLElement>('button[aria-label="material-0.mp4"]');
    if (!clip) throw new Error("the first clip has no drag surface");
    fireEvent.pointerDown(clip, { button: 0, clientX: 10, clientY: 0 });
    for (let step = 1; step <= 20; step++) {
      fireEvent.pointerMove(window, { clientX: 10 + step * 4, clientY: 0 });
    }
    fireEvent.pointerUp(window);

    const painted = paintsSince(before);
    // Twenty pointer moves, a selection and a release. The clip under the
    // pointer repaints for each of them because something about it genuinely
    // changed; the other forty-nine have no reason to, and before the memo they
    // all did — a thousand repaints for one drag.
    expect([...painted.keys()]).toEqual(["clip-0"]);
    expect(painted.get("clip-0")).toBeLessThanOrEqual(22);
  });

  it("repaints nothing at all while the razor tracks the pointer", () => {
    useTimelineStore.setState({ tool: "razor" });
    mount();

    const before = snapshot();
    const viewport = document.querySelector('[data-slot="timeline-viewport"]');
    if (!viewport) throw new Error("the timeline has no viewport");
    for (let step = 0; step < 20; step++) {
      fireEvent.pointerMove(viewport, { clientX: 40 + step * 3, clientY: 0 });
    }

    // The cut line is one pixel of DOM. Moving it used to re-render every lane
    // and every clip on the timeline, which is why it lives in the store.
    expect([...paintsSince(before).keys()]).toEqual([]);
  });

  it("moves the razor line without touching a single clip's props", () => {
    // Three clips: this one checks the guide is still driven, not how much it
    // costs, and mounting fifty of anything is the slowest thing in this file.
    useProjectStore.setState({ project: crowdedProject(3) });
    useTimelineStore.setState({ tool: "razor" });
    mount();

    const viewport = document.querySelector('[data-slot="timeline-viewport"]');
    if (!viewport) throw new Error("the timeline has no viewport");
    fireEvent.pointerMove(viewport, { clientX: 40, clientY: 0 });

    // The guide is still driven — this is a performance test, not a licence to
    // stop drawing the thing.
    expect(useTimelineStore.getState().razorTarget?.segmentId).toBe("clip-0");
    expect(document.querySelector('[data-slot="razor-guide"]')).toBeInTheDocument();
  });

  it("publishes nothing when a pointer move lands on the same microsecond", () => {
    useProjectStore.setState({ project: crowdedProject(3) });
    useTimelineStore.setState({ tool: "razor" });
    mount();
    const viewport = document.querySelector('[data-slot="timeline-viewport"]');
    if (!viewport) throw new Error("the timeline has no viewport");

    fireEvent.pointerMove(viewport, { clientX: 40, clientY: 0 });
    const target = useTimelineStore.getState().razorTarget;
    fireEvent.pointerMove(viewport, { clientX: 40, clientY: 0 });

    // Identity, not equality: subscribers compare by reference, and most of a
    // pointer stream changes nothing.
    expect(useTimelineStore.getState().razorTarget).toBe(target);
  });
});

describe("playback moving the playhead", () => {
  it("moves the line and the timecode without rendering the timeline body", () => {
    mount();

    const bodyBefore = bodyPaintCount.renders;
    const clipsBefore = snapshot();

    // What Rust does during playback: one position event per frame, each
    // writing the playhead. A second of 24 fps playback, delivered as fast as
    // the store can take it.
    act(() => {
      for (let frame = 1; frame <= 24; frame++) {
        useTimelineStore.getState().setPlayhead(frame * 41_667);
      }
    });

    // The line moved — the isolated subscription is alive, not optimised away.
    const line = document.querySelector<HTMLElement>('[data-slot="playhead"]');
    if (!line) throw new Error("the timeline has no playhead");
    expect(line.style.left).not.toBe("0px");

    // And nothing else did. Before the playhead became its own subscriber,
    // every one of those 24 writes re-rendered this entire component — the
    // whole lane list reconciled 24 times a second to move one pixel of DOM,
    // while the same thread decoded and painted the preview's JPEGs. That is
    // what the owner reported as the timeline "lagging insanely".
    expect(bodyPaintCount.renders).toBe(bodyBefore);
    expect([...paintsSince(clipsBefore).keys()]).toEqual([]);
  });
});

describe("escape backs out in order", () => {
  it("returns the razor to select first, and clears the selection second", () => {
    mount();
    useTimelineStore.setState({ tool: "razor" });
    const firstClip = useProjectStore.getState().project?.tracks[0]?.segments[0]?.id;
    if (!firstClip) throw new Error("the crowded project has no clips");
    act(() => {
      useTimelineStore.getState().select(firstClip);
    });

    // First press: done cutting, back on Select — the selection survives,
    // because throwing it away for leaving a tool punishes the wrong action.
    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(useTimelineStore.getState().tool).toBe("select");
    expect(useTimelineStore.getState().selection).toContain(firstClip);

    // Second press: now it means "nothing selected".
    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(useTimelineStore.getState().selection).toHaveLength(0);
  });
});

describe("following the playhead during playback", () => {
  function viewport(): HTMLElement {
    const element = document.querySelector<HTMLElement>('[data-slot="timeline-viewport"]');
    if (!element) throw new Error("the timeline has no viewport");
    // jsdom lays nothing out, so the follower's guard would bail on width 0.
    Object.defineProperty(element, "clientWidth", { value: 1000, configurable: true });
    return element;
  }

  it("pages forward when the playhead crosses the right edge, and only then", async () => {
    mount();
    const element = viewport();
    const { usePreviewStore } = await import("@/modules/preview/store");
    usePreviewStore.setState({ playing: true });
    const zoom = useTimelineStore.getState().zoom;
    const bodyBefore = bodyPaintCount.renders;

    // Inside the page: playback position events must not move the scroll.
    act(() => {
      useTimelineStore.getState().setPlayhead(Math.round((500 / zoom) * 0.5) * 2);
    });
    expect(element.scrollLeft).toBe(0);

    // Past 95% of the viewport: one page turn, playhead re-placed at 10%.
    const past = Math.ceil(960 / zoom);
    act(() => {
      useTimelineStore.getState().setPlayhead(past);
    });
    expect(element.scrollLeft).toBeCloseTo(past * zoom - 100, 0);
    // The page turn costs a couple of body renders (scrollX, then the snapped
    // viewport window), once every page — versus a render per position event,
    // which is what continuous following would cost. The bound guards against
    // the 24-a-second storm coming back, not against the second render.
    expect(bodyPaintCount.renders - bodyBefore).toBeLessThanOrEqual(2);
  });

  it("stays put when paused, whoever moves the playhead", async () => {
    mount();
    const element = viewport();
    const { usePreviewStore } = await import("@/modules/preview/store");
    usePreviewStore.setState({ playing: false });

    act(() => {
      useTimelineStore.getState().setPlayhead(10_000_000);
    });
    // A paused timeline belongs to the user's own scroll position.
    expect(element.scrollLeft).toBe(0);
  });
});

describe("scrolling a crowded timeline", () => {
  it("leaves the clips alone until the visible window has actually moved", () => {
    mount();
    const viewport = document.querySelector('[data-slot="timeline-viewport"]');
    if (!viewport) throw new Error("the timeline has no viewport");

    const before = snapshot();
    // A trackpad emits a stream of these. Every one of them used to hand every
    // clip a brand-new window object.
    for (let step = 1; step <= 8; step++) {
      Object.defineProperty(viewport, "scrollLeft", { value: step * 4, configurable: true });
      fireEvent.scroll(viewport);
    }

    expect([...paintsSince(before).keys()]).toEqual([]);
  });

  it("does repaint once the window crosses a grid step", () => {
    mount();
    const viewport = document.querySelector('[data-slot="timeline-viewport"]');
    if (!viewport) throw new Error("the timeline has no viewport");

    const before = snapshot();
    Object.defineProperty(viewport, "scrollLeft", { value: 4000, configurable: true });
    fireEvent.scroll(viewport);

    // Quantised, not frozen: scrolling a long way must still bring the tiles
    // that scrolled into view.
    expect(paintsSince(before).size).toBe(CLIPS);
  });
});
