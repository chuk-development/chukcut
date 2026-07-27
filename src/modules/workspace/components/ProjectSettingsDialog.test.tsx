/**
 * The project-settings dialog, from the controls to `project_configure`.
 *
 * The Rust side pins that a configure is one undoable step; this side pins
 * that the dialog reads the *document* rather than remembering its last visit,
 * commits everything in one call, and speaks the document's colour space —
 * linear RGBA — while the colour input speaks sRGB hex.
 */

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useProjectStore } from "@/modules/project/store";
import {
  backgroundOfHex,
  hexOfBackground,
  ProjectSettingsDialog,
} from "@/modules/workspace/components/ProjectSettingsDialog";
import { makeEditResponse, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

const PROJECT = makeProject({ name: "Reel" });

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  useProjectStore.setState({ project: PROJECT, dirty: false });
});

afterEach(() => {
  ipc.restore();
  useProjectStore.setState({ project: null, dirty: false, error: null });
});

function mount(onOpenChange = vi.fn()) {
  render(<ProjectSettingsDialog open onOpenChange={onOpenChange} />);
  return onOpenChange;
}

describe("the dialog", () => {
  it("shows the open document's values, not defaults and not its last visit", () => {
    mount();

    expect(screen.getByRole("textbox", { name: "Project name" })).toHaveValue("Reel");
    expect(screen.getByLabelText("Canvas width")).toHaveValue("1080");
    expect(screen.getByLabelText("Canvas height")).toHaveValue("1920");
  });

  it("commits everything as one project_configure call and closes", async () => {
    ipc.handle(
      "project_configure",
      makeEditResponse(makeProject({ name: "Renamed" }), { undo_label: "Project settings" }),
    );
    const onOpenChange = mount();

    const name = screen.getByRole("textbox", { name: "Project name" });
    await userEvent.clear(name);
    await userEvent.type(name, "Renamed");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    await waitFor(() => expect(ipc.count("project_configure")).toBe(1));
    expect(ipc.lastCall("project_configure")).toEqual({
      config: {
        name: "Renamed",
        width: 1080,
        height: 1920,
        fps: 30,
        background: [0, 0, 0, 1],
      },
    });
    // The response replaced the document, exactly like a timeline edit.
    expect(useProjectStore.getState().project?.name).toBe("Renamed");
    expect(useProjectStore.getState().dirty).toBe(true);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("refuses an odd canvas before it ever reaches the boundary", async () => {
    mount();

    const width = screen.getByLabelText("Canvas width");
    await userEvent.clear(width);
    await userEvent.type(width, "1235");

    expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
    expect(screen.getByText(/even/i)).toBeInTheDocument();
    expect(ipc.count("project_configure")).toBe(0);
  });

  it("warns that changing the frame rate re-times nothing", () => {
    mount();
    expect(screen.getByText(/re-times nothing/i)).toBeInTheDocument();
  });

  it("stays open and surfaces the message when Rust refuses", async () => {
    ipc.fail("project_configure", "the canvas cannot be larger than 8192 pixels on an edge");
    const onOpenChange = mount();

    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    await waitFor(() => expect(useProjectStore.getState().error).toMatch(/8192/));
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });
});

describe("the colour round trip", () => {
  it("converts the document's linear black and white to the obvious hexes", () => {
    expect(hexOfBackground([0, 0, 0, 1])).toBe("#000000");
    expect(hexOfBackground([1, 1, 1, 1])).toBe("#ffffff");
  });

  it("round-trips any hex through linear and back unchanged", () => {
    // The sRGB transfer function is its own inverse composed — a plain 2.2
    // gamma would drift on mid greys, which is exactly where a background
    // colour lives.
    for (const hex of ["#000000", "#ffffff", "#808080", "#123456", "#0a141e"]) {
      expect(hexOfBackground(backgroundOfHex(hex))).toBe(hex);
    }
  });

  it("speaks linear on the wire: mid grey is not 0.5", () => {
    const [r] = backgroundOfHex("#808080");
    // sRGB 128 is linear ~0.216. If this were 0.5, the dialog would be
    // writing sRGB into a linear field and every background would render
    // too bright.
    expect(r).toBeGreaterThan(0.2);
    expect(r).toBeLessThan(0.23);
  });
});
