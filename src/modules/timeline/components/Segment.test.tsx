/**
 * What a clip looks like, in the states that are easy to get wrong.
 *
 * The states worth pinning are the ones with no picture in them — a clip whose
 * thumbnails have not arrived, one whose never will, one that has run out of
 * material to show. Each of those used to be the same flat rectangle, which is
 * indistinguishable from the editor being broken.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Micros } from "@/modules/project/types";
import type { ClipMaterial } from "@/modules/timeline/components/Segment";
import { Segment } from "@/modules/timeline/components/Segment";
import { makeSegment, range } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

const SECOND = 1_000_000;

const VIDEO: ClipMaterial = {
  path: "/media/a.mp4",
  aspect: 16 / 9,
  duration: 10 * SECOND,
  hasAudio: true,
  audioOnly: false,
  missing: false,
};

const noop = () => {};

function renderClip(over: Partial<Parameters<typeof Segment>[0]> = {}) {
  const segment = makeSegment("clip-1", {
    target_range: range(0, 4 * SECOND),
    source_range: range(2 * SECOND, 4 * SECOND),
  });
  return render(
    <Segment
      segment={segment}
      kind="video"
      label="a.mp4"
      selected={false}
      locked={false}
      muted={false}
      razor={false}
      linked={false}
      soundOnPartnerLane={false}
      linkable={false}
      zoom={1e-4}
      preview={null}
      ghosted={false}
      laneHeight={56}
      material={VIDEO}
      viewport={{ from: 0, to: 60 * SECOND } as { from: Micros; to: Micros }}
      onGesture={noop}
      onSelect={noop}
      onSplit={noop}
      onDuplicate={noop}
      onDelete={noop}
      onRippleDelete={noop}
      onUnlink={noop}
      onLink={noop}
      onFade={noop}
      name={null}
      canDetachAudio={false}
      canPasteAttributes={false}
      renaming={false}
      onSetSpeed={noop}
      onCustomSpeed={noop}
      onToggleMute={noop}
      onRenameStart={noop}
      onRenameCommit={noop}
      onRenameCancel={noop}
      onDetachAudio={noop}
      onReattachAudio={noop}
      onPasteAttributes={noop}
      {...over}
    />,
  );
}

function filmstrip() {
  return document.querySelector('[data-slot="filmstrip"]');
}

function placeholders() {
  return document.querySelectorAll('[data-slot="filmstrip-placeholder"]');
}

let ipc: IpcHarness;

/**
 * Jobs left running on purpose, ended when the test ends.
 *
 * A job only finishes when its terminal batch arrives, and the queue runs one
 * file at a time from a module-level singleton — so a job left open would stall
 * every test after this one.
 */
const running: (() => void)[] = [];

function heldJob(payload: { path?: unknown }): string {
  // The call this responder is answering, so the terminal batch later goes to
  // this job's own channel rather than to whichever one happens to be last.
  const call = ipc.count("media_thumbnails") - 1;
  running.push(() => ipc.channel("media_thumbnails", call).emit(terminal));
  return `job-${String(payload.path)}`;
}

/** The message every job ends with, whatever happened. */
const terminal = {
  job_id: "job-1",
  total: 12,
  produced: 0,
  tiles: [],
  complete: true,
  cancelled: false,
  error: null,
};

beforeEach(() => {
  ipc = installIpc();
  // Every clip retains its strip on mount, so every `renderClip` starts a job.
  // They are all held open and ended below, which keeps one test's decode from
  // sitting at the head of the queue during the next one.
  ipc.handle("media_thumbnails", heldJob);
  ipc.handle("media_waveform", { buckets: 0, duration: 0, min: [], max: [], rms: [] });
});

afterEach(async () => {
  for (const finish of running) finish();
  running.length = 0;
  // A macrotask, not a microtask: the queue advances to the next file on the
  // terminal batch, and the next test's answers must not be scripted until it
  // has. `vi.waitFor` would be satisfied on its first synchronous attempt.
  await new Promise((resolve) => setTimeout(resolve, 1));
  ipc.restore();
});

describe("a clip whose thumbnails have not arrived", () => {
  /**
   * The thumbnail store is a module-level singleton and a request for a path is
   * sticky by design, so each of these uses a file of its own rather than
   * resetting the module the component is bound to.
   */
  async function store() {
    return (await import("@/modules/media/lib/thumbnails")).useThumbnailStore;
  }

  it("lays out placeholders the moment it mounts", () => {
    renderClip();

    // A clip that has just been dropped has no tiles yet. It still has to look
    // like a clip waiting for frames, not like one that failed to get any.
    expect(filmstrip()).toHaveAttribute("data-status", "pending");
    expect(placeholders().length).toBeGreaterThan(0);
  });

  it("keeps pulsing while the decode is running", async () => {
    ipc.handle("media_thumbnails", heldJob);
    (await store()).getState().request("/media/pending.mp4");

    renderClip({ material: { ...VIDEO, path: "/media/pending.mp4" } });

    expect(filmstrip()).toHaveAttribute("data-status", "pending");
    for (const tile of placeholders()) expect(tile).toHaveClass("animate-pulse");
  });

  it("shows the tiles that have landed and placeholders for the rest", async () => {
    const useThumbnailStore = await store();
    ipc.handle("media_thumbnails", heldJob);
    useThumbnailStore.getState().request("/media/partial.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    // Index 2 of the strip, which is where this clip's first tile falls: it
    // starts two seconds into a ten-second file, so the file's own first frame
    // is not one of the frames it shows.
    ipc.channel("media_thumbnails").emit({
      job_id: "job-1",
      total: 12,
      produced: 1,
      tiles: [{ index: 2, at: 2_000_000, path: "/cache/2.jpg" }],
      complete: false,
      cancelled: false,
      error: null,
    });
    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/partial.mp4"].received).toBe(1),
    );

    renderClip({ material: { ...VIDEO, path: "/media/partial.mp4" } });

    const strip = filmstrip();
    expect(strip).toHaveAttribute("data-status", "partial");
    // Progressive, not all-or-nothing: one real frame is on screen while the
    // rest are still decoding.
    expect(strip?.querySelectorAll('[style*="background-image"]')).toHaveLength(1);
    expect(placeholders().length).toBeGreaterThan(0);
  });

  it("settles into a static texture once nothing more is coming", async () => {
    const useThumbnailStore = await store();
    // The failure is a field on the terminal batch: the command itself answered
    // with a job id long before the decoder got there.
    ipc.handle("media_thumbnails", "job-broken");
    useThumbnailStore.getState().request("/media/broken.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    ipc.channel("media_thumbnails").emit({ ...terminal, error: "no video stream" });
    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/broken.mp4"].status).toBe("failed"),
    );

    renderClip({ material: { ...VIDEO, path: "/media/broken.mp4" } });

    expect(filmstrip()).toHaveAttribute("data-status", "failed");
    // "Coming" pulses, "never" does not: the two must not be the same picture.
    for (const tile of placeholders()) expect(tile).not.toHaveClass("animate-pulse");
  });
});

describe("a clip trimmed to the limits of its material", () => {
  it("says nothing when there is material left on both sides", () => {
    renderClip();

    expect(document.querySelectorAll('[data-slot="source-limit"]')).toHaveLength(0);
  });

  it("marks the head when the clip starts at the first frame of the file", () => {
    renderClip({
      segment: makeSegment("clip-1", {
        target_range: range(0, 4 * SECOND),
        source_range: range(0, 4 * SECOND),
      }),
    });

    const limits = document.querySelectorAll('[data-slot="source-limit"]');
    expect(limits).toHaveLength(1);
    expect(limits[0]).toHaveAttribute("data-edge", "start");
  });

  it("marks both edges when the clip is the whole file", () => {
    renderClip({
      segment: makeSegment("clip-1", {
        target_range: range(0, 10 * SECOND),
        source_range: range(0, 10 * SECOND),
      }),
    });

    const edges = [...document.querySelectorAll('[data-slot="source-limit"]')].map((node) =>
      node.getAttribute("data-edge"),
    );
    expect(edges).toEqual(["start", "end"]);
  });

  it("says nothing for a still, which has no limits to run into", () => {
    renderClip({
      material: { ...VIDEO, duration: 0, hasAudio: false, path: "/media/a.png" },
      segment: makeSegment("clip-1", {
        target_range: range(0, 4 * SECOND),
        source_range: range(0, 4 * SECOND),
      }),
    });

    expect(document.querySelectorAll('[data-slot="source-limit"]')).toHaveLength(0);
  });
});

describe("the states a glance has to tell apart", () => {
  it("marks selection on the clip itself, not only in a class name", () => {
    renderClip({ selected: true });

    expect(document.querySelector('[data-slot="segment"]')).toHaveAttribute("data-selected");
  });

  it("says a locked clip is locked, in words as well as in hatching", () => {
    renderClip({ locked: true });

    expect(document.querySelector('[data-slot="segment"]')).toHaveAttribute("data-locked");
    expect(screen.getByLabelText("Locked")).toBeInTheDocument();
    // A locked clip cannot be trimmed, so it must not offer handles.
    expect(screen.queryByRole("button", { name: "Trim start" })).toBeNull();
  });

  it("says a silent clip is silent", () => {
    renderClip({ muted: true });

    expect(document.querySelector('[data-slot="segment"]')).toHaveAttribute("data-muted");
    expect(screen.getByLabelText("Silent")).toBeInTheDocument();
  });

  it("reaches the name through the title when the clip is too narrow to print it", () => {
    // 0.2 s at this zoom is twenty pixels: no room for a filename, and a
    // truncated one letter is worse than none.
    renderClip({
      segment: makeSegment("clip-1", {
        target_range: range(0, 200_000),
        source_range: range(2 * SECOND, 200_000),
      }),
    });

    expect(screen.queryByText("a.mp4")).toBeNull();
    expect(screen.getByRole("button", { name: "a.mp4" })).toHaveAttribute("title", "a.mp4");
  });
});

describe("a clip whose media is missing", () => {
  /**
   * The two causes draw the same state on purpose: a material removed from
   * the pool (`material` is null) and a file gone from disk (`missing` on the
   * material). To the user both mean "this clip has no media right now", and
   * either way the clip itself must stay — deleting the cut is exactly what
   * removing a library entry must never do.
   */
  it("marks a clip whose material was removed from the pool, keeping its label", () => {
    renderClip({ material: null });

    const clip = document.querySelector('[data-slot="segment"]');
    expect(clip).toHaveAttribute("data-missing");
    expect(screen.getByLabelText("Media offline")).toBeInTheDocument();
    // The label survives: the user has to be able to tell *which* file to
    // bring back.
    expect(screen.getByText("a.mp4")).toBeInTheDocument();
    // Nothing pretends there are frames to show.
    expect(filmstrip()).toBeNull();
  });

  it("marks a clip whose file is gone from disk the same way", () => {
    renderClip({ material: { ...VIDEO, missing: true } });

    expect(document.querySelector('[data-slot="segment"]')).toHaveAttribute("data-missing");
    expect(screen.getByLabelText("Media offline")).toBeInTheDocument();
    expect(filmstrip()).toBeNull();
    // No waveform and no fade handles either: there is no sound to shape.
    expect(document.querySelector('[data-slot="waveform"]')).toBeNull();
    expect(screen.queryByRole("button", { name: "Fade in" })).toBeNull();
  });

  it("does not mark a healthy clip", () => {
    renderClip();

    expect(document.querySelector('[data-slot="segment"]')).not.toHaveAttribute("data-missing");
    expect(screen.queryByLabelText("Media offline")).toBeNull();
  });
});

describe("audio on a clip", () => {
  it("draws a waveform on a video clip that carries sound", () => {
    renderClip();

    // The slim band along the bottom edge is what makes it possible to cut on a
    // beat without moving the clip to its own lane first.
    expect(document.querySelector('[data-slot="waveform"]')).toBeInTheDocument();
  });

  it("draws none on a video clip whose sound was split onto a linked lane", () => {
    // The import now puts the sound on its own audio lane, and that clip draws
    // the full waveform. Drawing the band here as well shows one waveform
    // twice, which reads as two pieces of audio — reported as "in der Videospur
    // ist immer noch die Audiospur zusätzlich zu sehen, also ist sie
    // dupliziert".
    renderClip({ soundOnPartnerLane: true });

    expect(document.querySelector('[data-slot="waveform"]')).toBeNull();
    // The picture is still a picture: only the waveform goes.
    expect(filmstrip()).toBeInTheDocument();
  });

  it("draws none on a video clip with no audio stream", () => {
    renderClip({ material: { ...VIDEO, hasAudio: false } });

    expect(document.querySelector('[data-slot="waveform"]')).toBeNull();
  });

  it("draws none for a clip scrolled off screen", () => {
    renderClip({ viewport: { from: 60 * SECOND, to: 120 * SECOND } });

    expect(document.querySelector('[data-slot="waveform"]')).toBeNull();
    expect(filmstrip()).toBeNull();
  });

  it("gives a clip on an audio lane the full waveform, not a band", () => {
    // What the sound of an imported file looks like once it has a lane of its
    // own: the material is still the video, and the lane is what decides.
    renderClip({ kind: "audio" });

    const waveform = document.querySelector('[data-slot="waveform"]');
    expect(waveform).toBeInTheDocument();
    expect(filmstrip()).toBeNull();
  });
});

describe("audio fades", () => {
  const SECOND_PX = SECOND * 1e-4; // 100 px per second at the fixture zoom.

  function fadedSegment() {
    return makeSegment("clip-1", {
      target_range: range(0, 4 * SECOND),
      source_range: range(0, 4 * SECOND),
      keyframes: [
        {
          property: "volume",
          keyframes: [
            { time: 0, value: 0, easing: "linear" },
            { time: SECOND, value: 1, easing: "linear" },
          ],
        },
      ],
    });
  }

  it("offers a handle at each top corner of a clip that carries sound", () => {
    renderClip();
    expect(screen.getByRole("button", { name: "Fade in" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Fade out" })).toBeInTheDocument();
  });

  it("offers none where the sound is not: no audio, a linked lane, a lock", () => {
    renderClip({ material: { ...VIDEO, hasAudio: false } });
    expect(screen.queryByRole("button", { name: "Fade in" })).toBeNull();
  });

  it("offers none on a clip whose sound lives on its linked audio lane", () => {
    // The fade belongs where the waveform is — on the audio clip below.
    renderClip({ soundOnPartnerLane: true });
    expect(screen.queryByRole("button", { name: "Fade in" })).toBeNull();
  });

  it("offers none on a locked clip", () => {
    renderClip({ locked: true });
    expect(screen.queryByRole("button", { name: "Fade in" })).toBeNull();
  });

  it("draws the ramp its volume keyframes spell", () => {
    renderClip({ segment: fadedSegment() });

    const ramp = document.querySelector('[data-slot="fade"][data-edge="in"]');
    expect(ramp).toBeInTheDocument();
    // One second of fade at 100 px per second.
    expect(ramp).toHaveStyle({ width: `${SECOND_PX}px` });
    expect(document.querySelector('[data-slot="fade"][data-edge="out"]')).toBeNull();
  });

  it("reports the fades a completed handle drag drew", () => {
    const onFade = vi.fn();
    renderClip({ onFade });

    // 100 px at zoom 1e-4 is one second of fade-in.
    fireEvent.pointerDown(screen.getByRole("button", { name: "Fade in" }), {
      button: 0,
      clientX: 0,
    });
    fireEvent.pointerMove(window, { clientX: 100 });
    fireEvent.pointerUp(window);

    expect(onFade).toHaveBeenCalledWith("clip-1", { fadeIn: SECOND, fadeOut: 0 });
  });

  it("reports a fade-out drag measured from the clip's end, leftwards", () => {
    const onFade = vi.fn();
    renderClip({ onFade });

    fireEvent.pointerDown(screen.getByRole("button", { name: "Fade out" }), {
      button: 0,
      clientX: 400,
    });
    fireEvent.pointerMove(window, { clientX: 350 });
    fireEvent.pointerUp(window);

    expect(onFade).toHaveBeenCalledWith("clip-1", { fadeIn: 0, fadeOut: SECOND / 2 });
  });

  it("dragging a fade back to nothing reports zero, which removes the keyframes", () => {
    const onFade = vi.fn();
    renderClip({ segment: fadedSegment(), onFade });

    fireEvent.pointerDown(screen.getByRole("button", { name: "Fade in" }), {
      button: 0,
      clientX: 100,
    });
    fireEvent.pointerMove(window, { clientX: -50 });
    fireEvent.pointerUp(window);

    expect(onFade).toHaveBeenCalledWith("clip-1", { fadeIn: 0, fadeOut: 0 });
  });

  it("says nothing when the handle never moved", () => {
    const onFade = vi.fn();
    renderClip({ onFade });

    fireEvent.pointerDown(screen.getByRole("button", { name: "Fade in" }), {
      button: 0,
      clientX: 0,
    });
    fireEvent.pointerUp(window);

    expect(onFade).not.toHaveBeenCalled();
  });
});

describe("a linked clip", () => {
  it("says so, because it is about to behave differently from how it looks", () => {
    // A drag that moves two clips when the user grabbed one is otherwise
    // indistinguishable from a bug.
    renderClip({ linked: true });
    expect(screen.getByLabelText("Linked to another clip")).toBeInTheDocument();
  });

  it("says nothing when it is on its own", () => {
    renderClip();
    expect(screen.queryByLabelText("Linked to another clip")).toBeNull();
  });
});
