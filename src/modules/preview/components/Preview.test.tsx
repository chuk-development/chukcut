/**
 * What the viewer does with the three answers the frame protocol gives.
 *
 * These go through the mounted panel rather than through `fetchFrame` alone,
 * because the interesting part is not the status mapping — that is tested next
 * door — but what the canvas and the error bar do afterwards.
 */

import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { contextOf } from "@/test/dom";
import { makePreviewInfo, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

function respondWith(status: number): Response {
  return {
    status,
    ok: status >= 200 && status < 300,
    blob: async () => new Blob(["jpeg bytes"]),
  } as unknown as Response;
}

let ipc: IpcHarness;
let fetchMock: ReturnType<typeof vi.fn>;

/** Mount the panel on a fresh module graph, with a session already answering. */
async function mountPreview() {
  vi.resetModules();
  const { Preview } = await import("@/modules/preview/components/Preview");
  const { useProjectStore } = await import("@/modules/project/store");
  const { usePreviewStore } = await import("@/modules/preview/store");
  useProjectStore.getState().loadProject(makeProject());

  const view = render(
    <TooltipProvider>
      <Preview />
    </TooltipProvider>,
  );
  const canvas = view.container.querySelector("canvas");
  if (!canvas) throw new Error("the player rendered no canvas");
  await waitFor(() => expect(usePreviewStore.getState().session).not.toBeNull());
  return { view, canvas, usePreviewStore };
}

beforeEach(() => {
  ipc = installIpc();
  ipc.handle("preview_start", makePreviewInfo({ session: 1, frame: 12 }));
  // The panel measures itself on every layout, so every mount reports a size.
  ipc.handle("preview_viewport", makePreviewInfo({ session: 1, frame: 12 }));
  fetchMock = vi.fn();
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal(
    "createImageBitmap",
    vi.fn(async () => ({ close: vi.fn() }) as unknown),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  ipc.restore();
});

describe("painting the frame the pacer says is due", () => {
  it("paints the placeholder, not a blank canvas, before a session exists", async () => {
    vi.resetModules();
    ipc.fail("preview_start", "still starting up");
    const { Preview } = await import("@/modules/preview/components/Preview");
    const { useProjectStore } = await import("@/modules/project/store");
    useProjectStore.getState().loadProject(makeProject());

    const view = render(
      <TooltipProvider>
        <Preview />
      </TooltipProvider>,
    );
    const canvas = view.container.querySelector("canvas");

    expect(contextOf(canvas as HTMLCanvasElement).calls.map((call) => call.op)).toContain(
      "fillRect",
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("draws the frame once it arrives", async () => {
    fetchMock.mockResolvedValue(respondWith(200));
    const { canvas } = await mountPreview();

    await waitFor(() => expect(contextOf(canvas).drawn).toHaveLength(1));
    expect(fetchMock).toHaveBeenCalledWith("chukcut-frame://preview/1/12");
  });
});

describe("the size the panel asks Rust for", () => {
  it("reports the picture as it is displayed, in device pixels rather than CSS ones", async () => {
    // The whole point of the change: Rust renders what the panel can show, not
    // what the project's canvas is. A 700 px canvas on a 2x display really does
    // show 1400 columns, so the CSS size is multiplied by `devicePixelRatio`
    // before it crosses.
    fetchMock.mockResolvedValue(respondWith(200));
    const { canvas } = await mountPreview();

    await waitFor(() => expect(ipc.count("preview_viewport")).toBeGreaterThan(0));
    const reported = ipc.lastCall("preview_viewport") as { width: number; height: number };
    const ratio = window.devicePixelRatio || 1;

    // Against the element's own style, which is what the user is looking at,
    // rather than against a number this test made up.
    expect(reported.width).toBe(Math.round(Number.parseFloat(canvas.style.width) * ratio));
    expect(reported.height).toBe(Math.round(Number.parseFloat(canvas.style.height) * ratio));
    expect(reported.width).toBeGreaterThan(0);
  });
});

describe("a superseded frame", () => {
  it("is dropped without painting anything", async () => {
    fetchMock.mockResolvedValue(respondWith(410));
    const { canvas } = await mountPreview();

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(contextOf(canvas).drawn).toEqual([]);
  });

  it("is never retried, because a newer frame is already on its way", async () => {
    fetchMock.mockResolvedValue(respondWith(410));
    await mountPreview();

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    // Give a second attempt time to happen if the code were to make one.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("says nothing to the user", async () => {
    fetchMock.mockResolvedValue(respondWith(410));
    const { usePreviewStore } = await mountPreview();

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(usePreviewStore.getState().error).toBeNull();
    // The error bar is the only thing in the panel with this icon-plus-text row.
    expect(screen.queryByText(/frame 12/)).toBeNull();
  });
});

describe("a frame that is not encoded yet", () => {
  /**
   * Three attempts, not one.
   *
   * Each one blocks in Rust for up to `FRAME_WAIT`, so this is about 180 ms of
   * patience — which is what a cold decoder seek costs, and what the
   * full-quality re-render after a pause costs. It is still a fixed number: a
   * loop would spin for as long as the encoder is behind.
   */
  const ATTEMPTS = 3;

  it("is retried a bounded number of times", async () => {
    fetchMock.mockResolvedValue(respondWith(404));
    await mountPreview();

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(ATTEMPTS));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fetchMock).toHaveBeenCalledTimes(ATTEMPTS);
  });

  it("treats a 204 as pending too, because that is a session with nothing in it yet", async () => {
    // `fetch` calls 204 a success and hands back an empty body, so without
    // this it fell through to `createImageBitmap` and came back as an error
    // nobody retried.
    fetchMock.mockResolvedValue(respondWith(204));
    const { canvas, usePreviewStore } = await mountPreview();

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(ATTEMPTS));
    expect(contextOf(canvas).drawn).toEqual([]);
    expect(usePreviewStore.getState().error).toBeNull();
  });

  it("leaves the previous picture up when the retries miss too", async () => {
    fetchMock.mockResolvedValue(respondWith(404));
    const { canvas } = await mountPreview();

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(ATTEMPTS));
    // Blanking the viewer while the encoder catches up reads as a crash.
    expect(contextOf(canvas).drawn).toEqual([]);
    expect(contextOf(canvas).calls.filter((call) => call.op === "clearRect")).toEqual([]);
  });

  it("paints the frame when the retry finds it, and stops asking", async () => {
    fetchMock.mockResolvedValueOnce(respondWith(404)).mockResolvedValueOnce(respondWith(200));
    const { canvas } = await mountPreview();

    await waitFor(() => expect(contextOf(canvas).drawn).toHaveLength(1));
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("says nothing to the user", async () => {
    fetchMock.mockResolvedValue(respondWith(404));
    const { usePreviewStore } = await mountPreview();

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(ATTEMPTS));
    expect(usePreviewStore.getState().error).toBeNull();
  });
});

describe("the error bar", () => {
  it("survives the panel reporting its size, which is not an answer to anything", async () => {
    // A resize lands on a debounce a fifth of a second after a layout the user
    // may not have caused. Clearing the message they are reading because the
    // window manager moved something is the bug this asserts against.
    fetchMock.mockResolvedValue(respondWith(200));
    const { usePreviewStore } = await mountPreview();
    ipc.channel("preview_start").emit({ type: "error", message: "decoder gave up on clip 3" });

    await waitFor(() => expect(ipc.count("preview_viewport")).toBeGreaterThan(0));
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(usePreviewStore.getState().error).toBe("decoder gave up on clip 3");
  });

  it("shows an error event from the render thread and nothing else", async () => {
    fetchMock.mockResolvedValue(respondWith(410));
    const { usePreviewStore } = await mountPreview();
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(screen.queryByText("decoder gave up on clip 3")).toBeNull();

    ipc.channel("preview_start").emit({ type: "error", message: "decoder gave up on clip 3" });

    expect(await screen.findByText("decoder gave up on clip 3")).toBeInTheDocument();
    expect(usePreviewStore.getState().error).toBe("decoder gave up on clip 3");
  });
});
