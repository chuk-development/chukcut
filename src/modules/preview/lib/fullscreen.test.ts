/**
 * Fullscreen, and the one way it can go wrong quietly.
 *
 * `core:default` does not include `core:window:allow-set-fullscreen`, so the
 * call is rejected until the capability file grants it. The failure mode that
 * matters is the UI believing it succeeded: a black overlay filling a 1280px
 * window, with the panels gone and no explanation. Everything below exists to
 * pin "the store follows the window, never the other way about".
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setFullscreen, toggleFullscreen } from "@/modules/preview/lib/fullscreen";
import { usePreviewStore } from "@/modules/preview/store";
import { type IpcHarness, installIpc } from "@/test/ipc";

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  usePreviewStore.setState({ fullscreen: false, error: null });
});

afterEach(() => {
  ipc.restore();
  usePreviewStore.setState({ fullscreen: false, error: null });
});

describe("asking the window for fullscreen", () => {
  it("goes to the window manager, not to a CSS class", async () => {
    ipc.handle("plugin:window|set_fullscreen", null);

    await setFullscreen(true);

    // A `position: fixed` div in a 1280px window shows the same pixels it
    // always did, which is the opposite of what this feature is for.
    expect(ipc.lastCall("plugin:window|set_fullscreen")).toMatchObject({ value: true });
    expect(usePreviewStore.getState().fullscreen).toBe(true);
  });

  it("toggles from whatever the store currently believes", async () => {
    ipc.handle("plugin:window|set_fullscreen", null);

    await toggleFullscreen();
    expect(ipc.lastCall("plugin:window|set_fullscreen")).toMatchObject({ value: true });

    await toggleFullscreen();
    expect(ipc.lastCall("plugin:window|set_fullscreen")).toMatchObject({ value: false });
    expect(usePreviewStore.getState().fullscreen).toBe(false);
  });

  it("stays in the panels when the permission is missing, and says why", async () => {
    ipc.fail(
      "plugin:window|set_fullscreen",
      "window.set_fullscreen not allowed. Permissions associated with this command: window:allow-set-fullscreen",
    );

    const changed = await setFullscreen(true);

    expect(changed).toBe(false);
    // The one thing that must not happen: an overlay over a window that never
    // went fullscreen.
    expect(usePreviewStore.getState().fullscreen).toBe(false);
    expect(usePreviewStore.getState().error).toMatch(/allow-set-fullscreen/);
  });

  it("clears an earlier failure once it works", async () => {
    ipc.fail("plugin:window|set_fullscreen", "not allowed");
    await setFullscreen(true);

    ipc.handle("plugin:window|set_fullscreen", null);
    await setFullscreen(true);

    expect(usePreviewStore.getState().fullscreen).toBe(true);
    expect(usePreviewStore.getState().error).toBeNull();
  });
});

describe("leaving fullscreen", () => {
  it("survives a session restart", () => {
    // Every edit re-snapshots the document and calls `reset`. Being thrown back
    // into the panels halfway through reviewing a cut would be unusable.
    usePreviewStore.setState({ fullscreen: true });

    usePreviewStore.getState().reset();

    expect(usePreviewStore.getState().fullscreen).toBe(true);
  });
});
