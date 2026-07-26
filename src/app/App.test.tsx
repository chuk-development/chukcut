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
import { useProjectStore } from "@/modules/project/store";
import { DEFAULT_SETTINGS } from "@/modules/workspace/types";
import { makeMaterial, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

/** One menu, enough to tell a drawn bar from an empty one. */
const MENU_BAR = [
  {
    title: "File",
    entries: [
      {
        kind: "item",
        id: "file.new",
        label: "New Project",
        accelerator: "Ctrl+N",
        enabled: true,
        unavailable_reason: null,
      },
    ],
  },
];

/** Everything the editor asks for while it is coming up. */
function scriptStartup(ipc: IpcHarness) {
  ipc.handle("project_get", makeProject());
  ipc.handle("project_path", null);
  ipc.handle("project_new", makeProject());
  ipc.handle("preview_start", null);
  ipc.handle("media_thumbnails", []);
  ipc.handle("workspace_settings_get", DEFAULT_SETTINGS);
  ipc.handle("workspace_recent_list", []);
  // The menu bar's contents and the window's own state. Both belong to the
  // title strip, which is drawn whatever else is going on — the window has no
  // decorations, so it is the only way to move or close it.
  ipc.handle("workspace_menu_describe", MENU_BAR);
  ipc.handle("plugin:window|is_maximized", false);
  // Neither of these is registered in Rust yet; both are meant to degrade to
  // "nothing to say" rather than to an error on screen.
  ipc.fail("workspace_recovery_status", "Command workspace_recovery_status not found");
  ipc.fail("workspace_recovery_peek", "Command workspace_recovery_peek not found");
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
  // The document store is a module singleton, so a project left open by the
  // previous test would hide the start screen in the next one.
  useProjectStore.setState({
    project: null,
    path: null,
    status: "loading",
    dirty: false,
    error: null,
  });
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

  it("shows the start screen when Rust has nothing open", async () => {
    ipc.handle("project_get", null);
    render(<App />);

    // It used to create "Untitled" here, which meant the user never chose a
    // canvas and never found their previous work.
    expect(await screen.findByLabelText("Start")).toBeInTheDocument();
    expect(ipc.count("project_new")).toBe(0);
    expect(screen.queryByLabelText("Player")).toBeNull();
  });

  it("goes into the editor once the start screen has made something", async () => {
    ipc.handle("project_get", null);
    render(<App />);
    await screen.findByLabelText("Start");

    (await screen.findByText("Vertical")).click();

    await waitFor(() => expect(ipc.count("project_new")).toBe(1));
    expect(ipc.lastCall("project_new")).toMatchObject({ width: 1080, height: 1920 });
    await expectTheEditorIsUp();
  });
});

describe("when the Rust core does not answer at all", () => {
  beforeEach(() => {
    ipc = installIpc();
    ipc.fail("project_get", "the backend is not running");
    ipc.fail("workspace_menu_describe", "the backend is not running");
    ipc.fail("plugin:window|is_maximized", "the backend is not running");
    ipc.handle("media_thumbnails", []);
  });

  it("says so instead of showing three empty panels", async () => {
    render(<App />);

    expect(await screen.findByText("The Rust core did not answer")).toBeInTheDocument();
    expect(screen.queryByLabelText("Player")).toBeNull();
  });

  it("still leaves a way to close the window", async () => {
    // The window has no decorations. If the strip went with the editor, a
    // backend that failed to start would leave a window with no title bar, no
    // menus and no close button — and nothing to do but kill it.
    render(<App />);
    await screen.findByText("The Rust core did not answer");

    expect(screen.getByRole("button", { name: "Close" })).toBeInTheDocument();
    expect(screen.getByRole("menubar")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// The title strip
// ---------------------------------------------------------------------------

describe("the strip that replaces the window decoration", () => {
  beforeEach(() => {
    ipc = installIpc();
    scriptStartup(ipc);
  });

  it("is there before a document is, and stays there once one is open", async () => {
    ipc.handle("project_get", null);
    render(<App />);

    await screen.findByLabelText("Start");
    expect(screen.getByRole("menubar")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Minimise" })).toBeInTheDocument();
  });

  it("draws the menus Rust described, rather than a copy kept on this side", async () => {
    render(<App />);
    await expectTheEditorIsUp();

    // The bar's contents cross the boundary on every state change: `menu.rs`
    // holds the one table of items and gates, and a duplicate here would be
    // free to drift out of step with it.
    expect(await screen.findByRole("menuitem", { name: "File" })).toBeInTheDocument();
    expect(ipc.lastCall("workspace_menu_describe")).toMatchObject({
      state: { has_project: true },
    });
  });

  it("is somewhere to pick the window up by", async () => {
    render(<App />);
    await expectTheEditorIsUp();

    // Tauri's own drag region, matched against the exact element pressed. Take
    // the attribute off and the window stops moving, which looks like a CSS bug
    // for an hour.
    const strip = document.querySelector('[data-slot="title-bar"]');
    expect(strip).toHaveAttribute("data-tauri-drag-region");
  });
});
