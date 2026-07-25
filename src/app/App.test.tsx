/**
 * The white-screen regression.
 *
 * `getCurrentWebview()` throws when the webview metadata is absent. The
 * subscription to desktop file drops is set up in an effect at the top of the
 * tree, so that throw used to take the whole editor with it — three panels and
 * an open document gone, because a convenience on top of the Import button
 * could not be wired up.
 */

import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "@/app/App";
import { makeMaterial, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

/** Everything the editor asks for while it is coming up. */
function scriptStartup(ipc: IpcHarness) {
  ipc.handle("project_get", makeProject());
  ipc.handle("project_path", null);
  ipc.handle("project_new", makeProject());
  ipc.handle("preview_start", null);
  ipc.handle("media_thumbnails", []);
}

async function expectTheEditorIsUp() {
  expect(await screen.findByLabelText("Player")).toBeInTheDocument();
  expect(screen.getByLabelText("Media library")).toBeInTheDocument();
  expect(screen.getByLabelText("Inspector")).toBeInTheDocument();
  expect(screen.queryByText("The Rust core did not answer")).toBeNull();
}

let ipc: IpcHarness;

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({ status: 410, ok: false }) as unknown as Response),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  ipc.restore();
});

describe("with no webview metadata", () => {
  beforeEach(() => {
    ipc = installIpc({ webviewLabel: null });
    scriptStartup(ipc);
  });

  it("still shows the editor", async () => {
    render(<App />);

    await expectTheEditorIsUp();
  });

  it("keeps the editor mounted after the failed subscription settles", async () => {
    render(<App />);
    await expectTheEditorIsUp();

    // The rejection lands a tick later than the first paint; the panels have to
    // survive it, not merely appear before it.
    await new Promise((resolve) => setTimeout(resolve, 20));

    await expectTheEditorIsUp();
  });

  it("still opens the document that Rust already had", async () => {
    render(<App />);

    expect(await screen.findByLabelText("Player")).toBeInTheDocument();
    await waitFor(() => expect(ipc.count("project_get")).toBe(1));
    // A project that was already open must not be replaced with a blank one.
    expect(ipc.count("project_new")).toBe(0);
  });
});

describe("with a working webview", () => {
  beforeEach(() => {
    ipc = installIpc();
    scriptStartup(ipc);
  });

  it("shows the editor and subscribes to desktop drops", async () => {
    render(<App />);

    await expectTheEditorIsUp();
  });

  it("puts a file dropped from the desktop into the library", async () => {
    ipc.handle("project_import_media", makeMaterial("v1", { name: "clip.mp4" }));
    render(<App />);
    await expectTheEditorIsUp();
    expect(screen.getByText("No media imported")).toBeInTheDocument();

    await ipc.emitWindowEvent("tauri://drag-drop", {
      paths: ["/media/clip.mp4"],
      position: { x: 40, y: 40 },
    });

    expect(await screen.findByText("clip.mp4")).toBeInTheDocument();
    expect(ipc.lastCall("project_import_media")).toEqual({ path: "/media/clip.mp4" });
  });

  it("creates a project when Rust has none open", async () => {
    ipc.handle("project_get", null);
    render(<App />);

    await waitFor(() => expect(ipc.count("project_new")).toBe(1));
    // An editor that opens onto an empty shell is hostile.
    expect(ipc.lastCall("project_new")).toEqual({
      name: "Untitled",
      width: 1080,
      height: 1920,
      fps: 30,
    });
    await expectTheEditorIsUp();
  });
});

describe("when the Rust core does not answer at all", () => {
  beforeEach(() => {
    ipc = installIpc();
    ipc.fail("project_get", "the backend is not running");
    ipc.handle("media_thumbnails", []);
  });

  it("says so instead of showing three empty panels", async () => {
    render(<App />);

    expect(await screen.findByText("The Rust core did not answer")).toBeInTheDocument();
    expect(screen.queryByLabelText("Player")).toBeNull();
  });
});
