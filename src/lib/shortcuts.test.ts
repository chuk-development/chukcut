/**
 * J/K/L, the transport keys with no panel to live in.
 *
 * What matters here: they drive the session through the same functions the
 * transport buttons use, they keep out of text fields and chords (Ctrl+L is
 * the browser's), and J does what it *says* — pause and one frame back —
 * because the engine cannot play in reverse and pretending otherwise would be
 * a stutter, not a feature.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { installTransportKeys } from "@/lib/shortcuts";
import { usePreviewStore } from "@/modules/preview/store";
import { useProjectStore } from "@/modules/project/store";
import { useTimelineStore } from "@/modules/timeline/store";
import { makePreviewInfo, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

let ipc: IpcHarness;
let teardown: () => void;

beforeEach(() => {
  ipc = installIpc();
  ipc.handle("preview_play", makePreviewInfo());
  ipc.handle("preview_pause", makePreviewInfo());
  ipc.handle("preview_seek", makePreviewInfo());
  useProjectStore.setState({ project: makeProject() });
  // A live session, so a seek goes to `preview_seek` rather than opening one.
  usePreviewStore.setState({ session: 1 });
  teardown = installTransportKeys();
});

afterEach(() => {
  teardown();
  ipc.restore();
  useProjectStore.setState({ project: null });
  usePreviewStore.setState({ session: null, playing: false });
  useTimelineStore.setState({ playhead: 0 });
});

function press(key: string, init: Partial<KeyboardEventInit> = {}) {
  window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, ...init }));
}

function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe("the transport keys", () => {
  it("L plays and K pauses, through the session's own functions", async () => {
    press("l");
    await settle();
    expect(ipc.count("preview_play")).toBe(1);

    press("k");
    await settle();
    expect(ipc.count("preview_pause")).toBe(1);
  });

  it("J pauses and lands exactly one frame earlier", async () => {
    useTimelineStore.setState({ playhead: 1_000_000 });

    press("j");
    await settle();

    expect(ipc.count("preview_pause")).toBe(1);
    // One frame at the project's 30 fps, rounded the way frameDuration rounds.
    expect(useTimelineStore.getState().playhead).toBe(1_000_000 - 33_333);
    expect(ipc.lastCall("preview_seek")).toEqual({ time: 1_000_000 - 33_333 });
  });

  it("J does not step past the start", async () => {
    useTimelineStore.setState({ playhead: 10_000 });

    press("j");
    await settle();

    expect(useTimelineStore.getState().playhead).toBe(0);
  });

  it("leaves a text field's letters alone", async () => {
    const input = document.createElement("input");
    document.body.appendChild(input);
    input.focus();

    input.dispatchEvent(new KeyboardEvent("keydown", { key: "l", bubbles: true }));
    await settle();

    expect(ipc.count("preview_play")).toBe(0);
    input.remove();
  });

  it("leaves chords alone — Ctrl+L is the browser's", async () => {
    press("l", { ctrlKey: true });
    press("l", { metaKey: true });
    press("l", { altKey: true });
    press("L", { shiftKey: true });
    await settle();

    expect(ipc.count("preview_play")).toBe(0);
  });

  it("does nothing on the start screen, where there is no session to drive", async () => {
    useProjectStore.setState({ project: null });

    press("l");
    press("k");
    press("j");
    await settle();

    expect(ipc.log).toEqual([]);
  });
});
