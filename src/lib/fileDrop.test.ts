/**
 * Desktop file drops, and the crash they once caused.
 *
 * `getCurrentWebview()` reads `window.__TAURI_INTERNALS__.metadata.currentWebview`
 * and throws a `TypeError` the moment that metadata is not there — which is the
 * state the webview is in early enough in startup, and the state it is in for
 * anything rendering outside a Tauri window. Thrown from inside an effect, that
 * took the whole editor down to a white screen.
 *
 * The fix is that `listenForFileDrop` is `async`, so the throw becomes a
 * rejection the caller can shrug off. These tests pin the promise, not the
 * `async` keyword: a refactor to `function listenForFileDrop() { return … }`
 * would reintroduce the crash while still returning a promise on the happy path.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { type FileDropEvent, listenForFileDrop } from "@/lib/fileDrop";
import { type IpcHarness, installIpc } from "@/test/ipc";

let ipc: IpcHarness | null = null;

afterEach(() => {
  ipc?.restore();
  ipc = null;
  vi.unstubAllGlobals();
});

describe("when the webview metadata is missing", () => {
  beforeEach(() => {
    ipc = installIpc({ webviewLabel: null });
  });

  it("does not throw synchronously", () => {
    // A synchronous throw here happens during the effect, and React unmounts
    // the tree it was in.
    let pending: Promise<unknown> | undefined;
    expect(() => {
      pending = listenForFileDrop({ onDrop: () => {} });
    }).not.toThrow();
    pending?.catch(() => {});
  });

  it("hands back a rejected promise the caller can catch", async () => {
    await expect(listenForFileDrop({ onDrop: () => {} })).rejects.toBeInstanceOf(TypeError);
  });

  it("never calls the handlers", async () => {
    const onDrop = vi.fn();
    const onEnter = vi.fn();
    await listenForFileDrop({ onDrop, onEnter }).catch(() => {});

    expect(onDrop).not.toHaveBeenCalled();
    expect(onEnter).not.toHaveBeenCalled();
  });
});

describe("when the webview is there", () => {
  beforeEach(() => {
    ipc = installIpc();
  });

  it("reports a drop with its paths and the pointer in CSS pixels", async () => {
    vi.stubGlobal("devicePixelRatio", 2);
    const dropped: FileDropEvent[] = [];
    await listenForFileDrop({ onDrop: (event) => dropped.push(event) });

    await ipc?.emitWindowEvent("tauri://drag-drop", {
      paths: ["/media/a.mp4", "/media/b.mp3"],
      position: { x: 600, y: 400 },
    });

    // The OS reports physical pixels; every rect in the UI is in CSS pixels, so
    // a HiDPI screen would drop the file at twice the intended position.
    expect(dropped).toEqual([
      { paths: ["/media/a.mp4", "/media/b.mp3"], clientX: 300, clientY: 200 },
    ]);
  });

  it("reports a hover as an enter, and does not call it a drop", async () => {
    const onDrop = vi.fn();
    const onEnter = vi.fn();
    await listenForFileDrop({ onDrop, onEnter });

    await ipc?.emitWindowEvent("tauri://drag-over", { position: { x: 120, y: 80 } });

    expect(onEnter).toHaveBeenCalledWith({ clientX: 120, clientY: 80 });
    expect(onDrop).not.toHaveBeenCalled();
  });

  it("clears the hint when the drag leaves, and again once a drop lands", async () => {
    const onLeave = vi.fn();
    await listenForFileDrop({ onDrop: () => {}, onLeave });

    await ipc?.emitWindowEvent("tauri://drag-leave", {});
    expect(onLeave).toHaveBeenCalledTimes(1);

    await ipc?.emitWindowEvent("tauri://drag-drop", {
      paths: ["/a.mp4"],
      position: { x: 0, y: 0 },
    });
    // Otherwise the "drop over the timeline" banner stays up after the drop.
    expect(onLeave).toHaveBeenCalledTimes(2);
  });
});
