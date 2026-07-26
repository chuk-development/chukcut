/**
 * The preview session controller.
 *
 * The controller keeps module-level state — the in-flight start, the restart
 * timer — so each test reloads the module graph and takes its stores from the
 * fresh copy. Sharing one graph across tests would let a pending restart from
 * the previous test decide the next one.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { makeEditResponse, makePreviewInfo, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

/** Comfortably past the restart debounce, without pinning its exact value. */
const PAST_THE_DEBOUNCE = 1000;

async function freshSession() {
  vi.resetModules();
  const session = await import("@/modules/preview/lib/session");
  const { usePreviewStore } = await import("@/modules/preview/store");
  const { useProjectStore } = await import("@/modules/project/store");
  const { useTimelineStore } = await import("@/modules/timeline/store");
  return {
    preview: session.preview,
    watchDocumentForPreview: session.watchDocumentForPreview,
    usePreviewStore,
    useProjectStore,
    useTimelineStore,
  };
}

type Session = Awaited<ReturnType<typeof freshSession>>;

/** A controller with a project open and one session already running. */
async function started(sessionId = 7): Promise<Session> {
  const app = await freshSession();
  ipc.handle("preview_start", (_payload, index) =>
    makePreviewInfo({
      session: sessionId + index,
      frameUrl: `chukcut-frame://preview/${sessionId}`,
    }),
  );
  app.useProjectStore.getState().loadProject(makeProject());
  await app.preview.ensureStarted();
  return app;
}

/** Let every already-resolved promise settle without advancing the clock meaningfully. */
async function settle(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

let ipc: IpcHarness;

beforeEach(() => {
  vi.useFakeTimers();
  ipc = installIpc();
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
  ipc.restore();
});

describe("who moves the playhead", () => {
  it("moves the playhead when Rust reports a position and does not seek back", async () => {
    const app = await started();

    ipc.channel("preview_start").emit({
      type: "position",
      session: 7,
      frame: 12,
      time: 400_000,
      playing: true,
    });

    expect(app.useTimelineStore.getState().playhead).toBe(400_000);
    expect(app.usePreviewStore.getState().frame).toBe(12);
    expect(app.usePreviewStore.getState().playing).toBe(true);
    // The feedback loop: seeking in response to a position event makes the
    // clock chase itself and the picture oscillate.
    expect(ipc.count("preview_seek")).toBe(0);
  });

  it("does not seek even after fifty position events in a row", async () => {
    const app = await started();
    const channel = ipc.channel("preview_start");

    for (let frame = 1; frame <= 50; frame++) {
      channel.emit({
        type: "position",
        session: 7,
        frame,
        time: frame * 33_333,
        playing: true,
      });
    }

    expect(app.useTimelineStore.getState().playhead).toBe(50 * 33_333);
    expect(ipc.count("preview_seek")).toBe(0);
  });

  it("ignores a position event from a session that has already been replaced", async () => {
    const app = await started();
    app.useTimelineStore.getState().setPlayhead(2_000_000);

    ipc.channel("preview_start").emit({
      type: "position",
      session: 3,
      frame: 99,
      time: 9_000_000,
      playing: true,
    });

    expect(app.useTimelineStore.getState().playhead).toBe(2_000_000);
    expect(app.usePreviewStore.getState().frame).toBe(0);
  });

  it("seeks when the user scrubs, at a whole non-negative microsecond", async () => {
    const app = await started();
    ipc.handle("preview_seek", makePreviewInfo({ session: 7 }));

    await app.preview.seek(1_234_567.4);
    expect(ipc.lastCall("preview_seek")).toEqual({ time: 1_234_567 });

    await app.preview.seek(-500);
    expect(ipc.lastCall("preview_seek")).toEqual({ time: 0 });

    expect(ipc.count("preview_seek")).toBe(2);
  });

  it("opens a session instead of seeking when none is running yet", async () => {
    const app = await freshSession();
    ipc.handle("preview_start", makePreviewInfo({ session: 1 }));
    app.useProjectStore.getState().loadProject(makeProject());

    await app.preview.seek(500_000);

    expect(ipc.count("preview_start")).toBe(1);
    expect(ipc.count("preview_seek")).toBe(0);
  });
});

describe("pressing play at the end", () => {
  /** A project with something in it, so `projectDuration` is not zero. */
  async function withClip(): Promise<{ app: Session; duration: number }> {
    const app = await freshSession();
    ipc.handle("preview_start", makePreviewInfo({ session: 7 }));
    ipc.handle("preview_seek", makePreviewInfo({ session: 7 }));
    ipc.handle("preview_play", makePreviewInfo({ session: 7, playing: true }));
    const { makeSegment, projectWithSegments, range } = await import("@/test/fixtures");
    const duration = 5_000_000;
    app.useProjectStore
      .getState()
      .loadProject(projectWithSegments(makeSegment("clip-1", { target_range: range(0, duration) })));
    await app.preview.ensureStarted();
    return { app, duration };
  }

  it("rewinds to the start when the playhead is already at the end", async () => {
    // The state an editor is in every time the user has watched to the end, and
    // the state a restored session comes back in. Without the rewind, Rust
    // plays the zero frames that remain and the picture never moves — which is
    // indistinguishable from a broken play button.
    const { app, duration } = await withClip();
    app.useTimelineStore.getState().setPlayhead(duration);

    await app.preview.play();

    expect(ipc.lastCall("preview_seek")).toEqual({ time: 0 });
    expect(app.useTimelineStore.getState().playhead).toBe(0);
    expect(ipc.count("preview_play")).toBe(1);
  });

  it("rewinds from within one frame of the end, and not from further back", async () => {
    const { app, duration } = await withClip();

    // A frame short of the end is the same intent.
    app.useTimelineStore.getState().setPlayhead(duration - 20_000);
    await app.preview.play();
    expect(ipc.count("preview_seek")).toBe(1);

    // Half a second of material left is a deliberate play of the tail.
    app.useTimelineStore.getState().setPlayhead(duration - 500_000);
    await app.preview.play();
    expect(ipc.count("preview_seek")).toBe(1);
    expect(app.useTimelineStore.getState().playhead).toBe(duration - 500_000);
  });

  it("plays from where it stands in the middle of the timeline", async () => {
    const { app, duration } = await withClip();
    app.useTimelineStore.getState().setPlayhead(Math.floor(duration / 2));

    await app.preview.play();

    expect(ipc.count("preview_seek")).toBe(0);
    expect(app.useTimelineStore.getState().playhead).toBe(Math.floor(duration / 2));
  });
});

describe("restarting after an edit", () => {
  it("opens exactly one new session when ten commands arrive in a burst", async () => {
    const app = await started();
    const stop = app.watchDocumentForPreview();
    expect(ipc.count("preview_start")).toBe(1);

    for (let i = 0; i < 10; i++) {
      app.useProjectStore.getState().applyEditResponse(makeEditResponse(makeProject()));
    }

    // Debounced: nothing has been asked for yet.
    await settle();
    expect(ipc.count("preview_start")).toBe(1);

    await vi.advanceTimersByTimeAsync(PAST_THE_DEBOUNCE);
    expect(ipc.count("preview_start")).toBe(2);

    // And it does not keep firing once the burst is over.
    await vi.advanceTimersByTimeAsync(PAST_THE_DEBOUNCE);
    expect(ipc.count("preview_start")).toBe(2);
    stop();
  });

  it("opens the new session at the current playhead", async () => {
    const app = await started();
    const stop = app.watchDocumentForPreview();
    app.useTimelineStore.getState().setPlayhead(3_500_000);

    app.useProjectStore.getState().applyEditResponse(makeEditResponse(makeProject()));
    await vi.advanceTimersByTimeAsync(PAST_THE_DEBOUNCE);

    expect(ipc.calls("preview_start")[1]).toMatchObject({ time: 3_500_000, options: null });
    stop();
  });

  it("resumes playback after a restart when the user was watching", async () => {
    const app = await started();
    ipc.handle("preview_play", makePreviewInfo({ session: 7, playing: true }));
    await app.preview.play();
    expect(app.usePreviewStore.getState().playing).toBe(true);
    expect(ipc.count("preview_play")).toBe(1);

    const stop = app.watchDocumentForPreview();
    app.useProjectStore.getState().applyEditResponse(makeEditResponse(makeProject()));
    await vi.advanceTimersByTimeAsync(PAST_THE_DEBOUNCE);

    // `preview_start` adopts paused, so a resume has to be asked for.
    expect(ipc.count("preview_play")).toBe(2);
    stop();
  });

  it("leaves a paused preview paused after a restart", async () => {
    const app = await started();
    ipc.handle("preview_play", makePreviewInfo({ session: 7, playing: true }));
    expect(app.usePreviewStore.getState().playing).toBe(false);

    const stop = app.watchDocumentForPreview();
    app.useProjectStore.getState().applyEditResponse(makeEditResponse(makeProject()));
    await vi.advanceTimersByTimeAsync(PAST_THE_DEBOUNCE);

    expect(ipc.count("preview_start")).toBe(2);
    expect(ipc.count("preview_play")).toBe(0);
    stop();
  });

  it("hands each session its own channel, and ignores the retired one", async () => {
    const app = await started();
    const stop = app.watchDocumentForPreview();
    const first = ipc.channel("preview_start", 0);

    app.useProjectStore.getState().applyEditResponse(makeEditResponse(makeProject()));
    await vi.advanceTimersByTimeAsync(PAST_THE_DEBOUNCE);

    const second = ipc.channel("preview_start", 1);
    expect(second.id).not.toBe(first.id);
    expect(app.usePreviewStore.getState().session).toBe(8);

    // The render thread of the retired session can still be draining.
    first.emit({ type: "position", session: 7, frame: 400, time: 8_000_000, playing: true });
    expect(app.useTimelineStore.getState().playhead).toBe(0);

    second.emit({ type: "position", session: 8, frame: 3, time: 100_000, playing: false });
    expect(app.useTimelineStore.getState().playhead).toBe(100_000);
    stop();
  });

  it("opens the first session rather than restarting when a project is loaded", async () => {
    const app = await freshSession();
    ipc.handle("preview_start", makePreviewInfo({ session: 1 }));
    const stop = app.watchDocumentForPreview();

    app.useProjectStore.getState().loadProject(makeProject());
    await settle();

    // No debounce for the first one: the viewer is blank until it exists.
    expect(ipc.count("preview_start")).toBe(1);
    stop();
  });

  it("does not open a session when there is no document to render", async () => {
    const app = await freshSession();
    ipc.handle("preview_start", makePreviewInfo());

    await app.preview.ensureStarted();

    expect(ipc.count("preview_start")).toBe(0);
  });

  it("folds overlapping first starts into one session", async () => {
    const app = await freshSession();
    ipc.handle("preview_start", makePreviewInfo({ session: 1 }));
    app.useProjectStore.getState().loadProject(makeProject());

    await Promise.all([
      app.preview.ensureStarted(),
      app.preview.ensureStarted(),
      app.preview.ensureStarted(),
    ]);

    expect(ipc.count("preview_start")).toBe(1);
  });
});

describe("what reaches the user", () => {
  it("shows the message from an error event", async () => {
    const app = await started();

    ipc.channel("preview_start").emit({ type: "error", message: "decoder gave up on clip 3" });

    expect(app.usePreviewStore.getState().error).toBe("decoder gave up on clip 3");
  });

  it("says nothing about position, ready or ended events", async () => {
    const app = await started();
    const channel = ipc.channel("preview_start");

    channel.emit({ type: "position", session: 7, frame: 1, time: 33_333, playing: true });
    channel.emit({ type: "ready", session: 7, width: 540, height: 960, fps: 30, duration: 1 });
    channel.emit({ type: "ended", session: 7 });

    expect(app.usePreviewStore.getState().error).toBeNull();
    // `ended` is how the transport learns playback finished.
    expect(app.usePreviewStore.getState().playing).toBe(false);
  });

  it("clears a stale error when the new session reports itself ready", async () => {
    const app = await started();
    const channel = ipc.channel("preview_start");
    channel.emit({ type: "error", message: "decoder gave up" });
    expect(app.usePreviewStore.getState().error).toBe("decoder gave up");

    channel.emit({ type: "ready", session: 7, width: 540, height: 960, fps: 30, duration: 1 });

    expect(app.usePreviewStore.getState().error).toBeNull();
  });

  it("surfaces Rust's sentence when the session cannot be opened at all", async () => {
    const app = await freshSession();
    ipc.fail("preview_start", "no GPU adapter available");
    app.useProjectStore.getState().loadProject(makeProject());

    await app.preview.ensureStarted();

    expect(app.usePreviewStore.getState().error).toBe("no GPU adapter available");
  });

  it("stays quiet when a seek lands on a session Rust has already closed", async () => {
    const app = await started();
    ipc.fail("preview_seek", "no session");

    await app.preview.seek(1_000_000);

    // A superseded seek is expected during a scrub; it is not news.
    expect(app.usePreviewStore.getState().error).toBeNull();
  });

  it("stays quiet when pausing something that was not playing", async () => {
    const app = await started();
    ipc.fail("preview_pause", "not playing");

    await app.preview.pause();

    expect(app.usePreviewStore.getState().error).toBeNull();
  });

  it("reports a failed play, because the user pressed a button and nothing happened", async () => {
    const app = await started();
    ipc.fail("preview_play", "audio device is busy");

    await app.preview.play();

    expect(app.usePreviewStore.getState().error).toBe("audio device is busy");
  });
});

describe("the transport toggle", () => {
  it("plays when paused and pauses when playing", async () => {
    const app = await started();
    ipc.handle("preview_play", makePreviewInfo({ session: 7, playing: true }));
    ipc.handle("preview_pause", makePreviewInfo({ session: 7, playing: false }));

    await app.preview.toggle();
    expect(ipc.count("preview_play")).toBe(1);
    expect(app.usePreviewStore.getState().playing).toBe(true);

    await app.preview.toggle();
    expect(ipc.count("preview_pause")).toBe(1);
    expect(app.usePreviewStore.getState().playing).toBe(false);
  });
});
