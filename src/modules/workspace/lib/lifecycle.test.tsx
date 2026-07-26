/**
 * The guard in front of unsaved work.
 *
 * Four outcomes and they are all load-bearing: a clean document is never asked
 * about, cancelling stops the thing that asked, discarding lets it through, and
 * saving lets it through *only if the save landed*. The last one is the bug
 * this guard exists to prevent — a cancelled file dialog that still throws the
 * work away is worse than no prompt at all.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useProjectStore } from "@/modules/project/store";
import { UnsavedChangesDialog } from "@/modules/workspace/components/UnsavedChangesDialog";
import { guardUnsaved, openProjectAt, saveProject } from "@/modules/workspace/lib/lifecycle";
import { useWorkspaceStore } from "@/modules/workspace/store";
import { makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

const PROJECT = makeProject({ name: "Kitchen cut" });

let ipc: IpcHarness;

function openDocument({ dirty, path }: { dirty: boolean; path: string | null }) {
  useProjectStore.setState({ project: PROJECT, path, dirty, status: "ready", error: null });
}

beforeEach(() => {
  ipc = installIpc();
  ipc.handle("workspace_recent_record", null);
  ipc.handle("workspace_recent_list", []);
  useWorkspaceStore.setState({ discardPrompt: null, recent: [] });
});

afterEach(() => {
  ipc.restore();
  useProjectStore.setState({ project: null, path: null, dirty: false, error: null });
});

describe("a document with nothing unsaved", () => {
  it("is never asked about", async () => {
    openDocument({ dirty: false, path: "/home/me/cut.chukcut" });

    await expect(guardUnsaved("opening another project")).resolves.toBe(true);
    expect(useWorkspaceStore.getState().discardPrompt).toBeNull();
  });
});

describe("a document with unsaved changes", () => {
  beforeEach(() => {
    openDocument({ dirty: true, path: "/home/me/cut.chukcut" });
  });

  it("raises the prompt and waits, rather than resolving on its own", async () => {
    let settled = false;
    const decision = guardUnsaved("opening another project").then((value) => {
      settled = true;
      return value;
    });

    // The store carries the *question*, phrased for the sentence it lands in.
    await Promise.resolve();
    expect(useWorkspaceStore.getState().discardPrompt).toBe("opening another project");
    expect(settled).toBe(false);

    useWorkspaceStore.getState().answerDiscardPrompt("discard");
    await expect(decision).resolves.toBe(true);
  });

  it("stops the caller when the answer is cancel", async () => {
    const decision = guardUnsaved("starting a new project");
    await Promise.resolve();
    useWorkspaceStore.getState().answerDiscardPrompt("cancel");

    await expect(decision).resolves.toBe(false);
    expect(useWorkspaceStore.getState().discardPrompt).toBeNull();
  });

  it("saves first when asked to, and only then lets the caller through", async () => {
    ipc.handle("project_save", "/home/me/cut.chukcut");

    const decision = guardUnsaved("opening another project");
    await Promise.resolve();
    useWorkspaceStore.getState().answerDiscardPrompt("save");

    await expect(decision).resolves.toBe(true);
    expect(ipc.lastCall("project_save")).toEqual({ path: "/home/me/cut.chukcut" });
    expect(useProjectStore.getState().dirty).toBe(false);
  });

  it("does not let the caller through when the save it asked for failed", async () => {
    ipc.fail("project_save", "no space left on device");

    const decision = guardUnsaved("opening another project");
    await Promise.resolve();
    useWorkspaceStore.getState().answerDiscardPrompt("save");

    // Continuing here would discard the work the user just asked us to keep.
    await expect(decision).resolves.toBe(false);
    expect(useProjectStore.getState().dirty).toBe(true);
    expect(useProjectStore.getState().error).toBe("no space left on device");
  });

  it("does not let the caller through when the Save-as dialog was cancelled", async () => {
    // Never saved, so "save" has to ask where — and the user closed the dialog.
    openDocument({ dirty: true, path: null });
    ipc.handle("plugin:dialog|save", null);

    const decision = guardUnsaved("starting a new project");
    await Promise.resolve();
    useWorkspaceStore.getState().answerDiscardPrompt("save");

    await expect(decision).resolves.toBe(false);
    expect(ipc.count("project_save")).toBe(0);
  });
});

describe("the prompt itself", () => {
  beforeEach(() => {
    openDocument({ dirty: true, path: "/home/me/cut.chukcut" });
  });

  it("offers save, cancel and discard, and names the project", async () => {
    render(<UnsavedChangesDialog />);
    const decision = guardUnsaved("opening another project");
    expect(await screen.findByText("Save your changes?")).toBeInTheDocument();
    expect(screen.getByText(/Kitchen cut/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Discard" }));

    await expect(decision).resolves.toBe(true);
  });

  it("treats dismissing it as cancel, never as discard", async () => {
    render(<UnsavedChangesDialog />);
    const decision = guardUnsaved("starting a new project");
    await screen.findByText("Save your changes?");

    await userEvent.keyboard("{Escape}");

    await expect(decision).resolves.toBe(false);
  });
});

describe("opening a project that has moved", () => {
  it("drops it from the recent list rather than leaving it to fail again", async () => {
    useWorkspaceStore.setState({
      recent: [
        { path: "/home/me/gone.chukcut", name: "Gone", opened_at: 1 },
        { path: "/home/me/here.chukcut", name: "Here", opened_at: 2 },
      ],
      recentStatus: "ready",
    });
    ipc.fail("project_open", "cannot read /home/me/gone.chukcut: No such file or directory");

    const opened = await openProjectAt("/home/me/gone.chukcut");

    expect(opened).toBe(false);
    expect(useWorkspaceStore.getState().recent.map((entry) => entry.path)).toEqual([
      "/home/me/here.chukcut",
    ]);
    expect(useProjectStore.getState().error).toMatch(/No such file/);
  });

  it("records an open that worked, so it moves to the front next time", async () => {
    ipc.handle("project_open", PROJECT);

    await openProjectAt("/home/me/here.chukcut");

    expect(useProjectStore.getState().path).toBe("/home/me/here.chukcut");
    expect(ipc.lastCall("workspace_recent_record")).toMatchObject({
      path: "/home/me/here.chukcut",
      name: "Kitchen cut",
    });
  });
});

describe("save as", () => {
  it("asks for a path even when the project already has one", async () => {
    openDocument({ dirty: true, path: "/home/me/cut.chukcut" });
    ipc.handle("plugin:dialog|save", "/home/me/copy.chukcut");
    ipc.handle("project_save", "/home/me/copy.chukcut");

    await expect(saveProject({ promptForPath: true })).resolves.toBe(true);

    expect(ipc.lastCall("project_save")).toEqual({ path: "/home/me/copy.chukcut" });
    expect(useProjectStore.getState().path).toBe("/home/me/copy.chukcut");
  });
});
