/**
 * The eight grips that give an undecorated window its frame back.
 *
 * This is the part of `decorations: false` that is silently missing until
 * someone tries to make the window bigger: on GTK the resize border *is* the
 * decoration, so with the decoration gone the window manager has nothing to
 * hit-test and `startResizeDragging` from inside the webview is the only way
 * back. Nothing on screen shows whether it is wired up — the grips are
 * transparent — so the test is the only thing standing between us and a window
 * that cannot be resized.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ResizeEdges } from "@/app/components/ResizeEdges";
import { type IpcHarness, installIpc } from "@/test/ipc";

const START_RESIZE = "plugin:window|start_resize_dragging";

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  ipc.handle(START_RESIZE, null);
});

afterEach(() => {
  ipc.restore();
});

function grip(edge: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(
    `[data-slot="resize-edge"][data-edge="${edge}"]`,
  );
  if (!found) throw new Error(`there is no ${edge} grip`);
  return found;
}

describe("the resize grips", () => {
  it("covers all four edges and all four corners", () => {
    render(<ResizeEdges />);

    expect(document.querySelectorAll('[data-slot="resize-edge"]')).toHaveLength(8);
  });

  it("starts a resize in the direction of the grip that was pressed", async () => {
    render(<ResizeEdges />);

    fireEvent.pointerDown(grip("SouthEast"), { button: 0 });

    await waitFor(() => expect(ipc.count(START_RESIZE)).toBe(1));
    expect(ipc.lastCall(START_RESIZE)).toEqual({ label: "main", value: "SouthEast" });
  });

  it("starts on the press, not on the click", async () => {
    // A resize is a drag. By the time a click has completed, the gesture it was
    // supposed to begin is over.
    render(<ResizeEdges />);

    fireEvent.click(grip("North"));
    expect(ipc.count(START_RESIZE)).toBe(0);

    fireEvent.pointerDown(grip("North"), { button: 0 });
    await waitFor(() => expect(ipc.count(START_RESIZE)).toBe(1));
  });

  it("leaves the other buttons to the window manager", async () => {
    render(<ResizeEdges />);

    // Right-click on a window frame is the window manager's own menu.
    fireEvent.pointerDown(grip("West"), { button: 2 });

    expect(ipc.count(START_RESIZE)).toBe(0);
  });

  it("is not something a keyboard or a screen reader has to walk past", () => {
    render(<ResizeEdges />);

    // Eight focusable, unlabelled elements between the user and the menu bar
    // would be a worse accessibility problem than the one they solve. Resizing
    // from the keyboard is the window manager's and still works.
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    for (const edge of document.querySelectorAll('[data-slot="resize-edge"]')) {
      expect(edge).toHaveAttribute("aria-hidden");
    }
  });
});
