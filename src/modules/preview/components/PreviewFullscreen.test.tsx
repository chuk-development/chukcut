/**
 * Fullscreen, through the mounted player.
 *
 * The point of the feature is judging image quality, so the assertion that
 * matters most is the one about the bitmap: the canvas element keeps the
 * proxy's own resolution and CSS scales it up. Setting the bitmap to the
 * on-screen size instead would resample the frame down to the panel and then
 * back up, throwing away exactly the detail fullscreen exists to show — and it
 * would look like proof that the preview is soft when the source is not.
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { makePreviewInfo, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

let ipc: IpcHarness;

/** A 4K project previewing at 1920×1080, so scaling is visible in the numbers. */
async function mountPreview() {
  vi.resetModules();
  const { Preview } = await import("@/modules/preview/components/Preview");
  const { useProjectStore } = await import("@/modules/project/store");
  const { usePreviewStore } = await import("@/modules/preview/store");
  const project = makeProject({
    canvas: { width: 3840, height: 2160, background: [0, 0, 0, 1] },
  });
  useProjectStore.getState().loadProject(project);

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

function panel(view: { container: HTMLElement }): HTMLElement {
  const element = view.container.querySelector('[data-slot="preview"]');
  if (!element) throw new Error("the player did not render");
  return element as HTMLElement;
}

beforeEach(() => {
  ipc = installIpc();
  ipc.handle("preview_start", makePreviewInfo({ session: 1, frame: 0, width: 1920, height: 1080 }));
  ipc.handle("plugin:window|set_fullscreen", null);
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({ status: 410, ok: false }) as unknown as Response),
  );
  vi.stubGlobal(
    "createImageBitmap",
    vi.fn(async () => ({ close: vi.fn() }) as unknown),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  ipc.restore();
});

// ---------------------------------------------------------------------------
// Getting there
// ---------------------------------------------------------------------------

describe("entering fullscreen", () => {
  it("asks the window manager when the transport button is pressed", async () => {
    const { view, usePreviewStore } = await mountPreview();

    fireEvent.click(screen.getByRole("button", { name: "Fullscreen" }));

    await waitFor(() => expect(usePreviewStore.getState().fullscreen).toBe(true));
    expect(ipc.lastCall("plugin:window|set_fullscreen")).toMatchObject({ value: true });
    expect(panel(view)).toHaveAttribute("data-fullscreen");
  });

  it("answers to F", async () => {
    const { usePreviewStore } = await mountPreview();

    fireEvent.keyDown(window, { key: "f" });

    await waitFor(() => expect(usePreviewStore.getState().fullscreen).toBe(true));
  });

  it("answers to a double-click on the picture", async () => {
    const { canvas, usePreviewStore } = await mountPreview();

    fireEvent.doubleClick(canvas);

    await waitFor(() => expect(usePreviewStore.getState().fullscreen).toBe(true));
  });

  it("does not fire F while a field has focus", async () => {
    await mountPreview();
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();

    fireEvent.keyDown(field, { key: "f" });

    expect(ipc.count("plugin:window|set_fullscreen")).toBe(0);
    field.remove();
  });
});

// ---------------------------------------------------------------------------
// What it looks like
// ---------------------------------------------------------------------------

describe("in fullscreen", () => {
  it("keeps the bitmap at the frame's own resolution and scales with CSS", async () => {
    const { canvas, usePreviewStore } = await mountPreview();
    usePreviewStore.getState().setFullscreen(true);

    await waitFor(() => expect(canvas.width).toBe(1920));
    // The element's intrinsic size is the proxy Rust rendered, whatever the
    // panel is doing. Anything else throws detail away.
    expect(canvas.height).toBe(1080);
    // The CSS box is what the letterboxing changes.
    expect(canvas.style.width).not.toBe("");
  });

  it("puts the picture on black with no panel chrome", async () => {
    const { view, canvas, usePreviewStore } = await mountPreview();
    usePreviewStore.getState().setFullscreen(true);

    await waitFor(() => expect(panel(view)).toHaveAttribute("data-fullscreen"));
    expect(panel(view).className).toContain("fixed inset-0");
    expect(panel(view).className).toContain("bg-preview-letterbox");
    // The header is the panel's, not the picture's.
    expect(screen.queryByRole("heading", { name: "Player" })).toBeNull();
    expect(canvas.className).not.toContain("shadow-");
  });

  it("shows a minimal transport with no zoom control", async () => {
    const { view, usePreviewStore } = await mountPreview();
    usePreviewStore.getState().setFullscreen(true);

    await waitFor(() =>
      expect(view.container.querySelector('[data-slot="fullscreen-transport"]')).not.toBeNull(),
    );
    expect(screen.getByRole("button", { name: "Leave fullscreen" })).toBeInTheDocument();
    // A fullscreen picture is already at the only scale that matters.
    expect(screen.queryByRole("combobox", { name: "Preview zoom" })).toBeNull();
  });

  it("fades the overlay out when the pointer stops, and brings it back on movement", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { view, usePreviewStore } = await mountPreview();
      usePreviewStore.getState().setFullscreen(true);
      await waitFor(() =>
        expect(view.container.querySelector('[data-slot="fullscreen-transport"]')).not.toBeNull(),
      );
      const overlay = () =>
        view.container.querySelector('[data-slot="fullscreen-transport"]') as HTMLElement;

      expect(overlay().className).toContain("opacity-100");

      await act(async () => {
        await vi.advanceTimersByTimeAsync(3_000);
      });
      // Hidden *and* out of the way: an invisible bar that still swallows
      // clicks is worse than one that stayed.
      expect(overlay().className).toContain("opacity-0");
      expect(overlay().className).toContain("pointer-events-none");

      fireEvent.pointerMove(window);
      expect(overlay().className).toContain("opacity-100");
    } finally {
      vi.useRealTimers();
    }
  });
});

// ---------------------------------------------------------------------------
// Coming back
// ---------------------------------------------------------------------------

describe("leaving fullscreen", () => {
  it("goes back to the panel on Escape", async () => {
    const { view, usePreviewStore } = await mountPreview();
    usePreviewStore.getState().setFullscreen(true);
    await waitFor(() => expect(panel(view)).toHaveAttribute("data-fullscreen"));

    fireEvent.keyDown(window, { key: "Escape" });

    await waitFor(() => expect(usePreviewStore.getState().fullscreen).toBe(false));
    expect(ipc.lastCall("plugin:window|set_fullscreen")).toMatchObject({ value: false });
    expect(panel(view)).not.toHaveAttribute("data-fullscreen");
    expect(screen.getByRole("heading", { name: "Player" })).toBeInTheDocument();
  });

  it("leaves the window alone when Escape is pressed in the panels", async () => {
    await mountPreview();

    fireEvent.keyDown(window, { key: "Escape" });

    // Escape belongs to the timeline there — it clears the selection.
    expect(ipc.count("plugin:window|set_fullscreen")).toBe(0);
  });

  it("goes back to the panel on the overlay's own button", async () => {
    const { usePreviewStore } = await mountPreview();
    usePreviewStore.getState().setFullscreen(true);

    fireEvent.click(await screen.findByRole("button", { name: "Leave fullscreen" }));

    await waitFor(() => expect(usePreviewStore.getState().fullscreen).toBe(false));
  });
});

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

describe("the keyboard while fullscreen", () => {
  it("still plays, steps and jumps", async () => {
    ipc.handle("preview_play", makePreviewInfo({ session: 1, playing: true }));
    ipc.handle("preview_seek", makePreviewInfo({ session: 1 }));
    const { usePreviewStore } = await mountPreview();
    usePreviewStore.getState().setFullscreen(true);

    fireEvent.keyDown(window, { key: " " });
    await waitFor(() => expect(ipc.count("preview_play")).toBe(1));

    fireEvent.keyDown(window, { key: "ArrowRight" });
    fireEvent.keyDown(window, { key: "End" });
    await waitFor(() => expect(ipc.count("preview_seek")).toBe(2));
  });
});
