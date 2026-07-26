/**
 * The screen the app opens onto when nothing is open.
 *
 * It exists because the editor used to create "Untitled" at launch, so the
 * three things worth pinning are: a preset actually creates a project with the
 * canvas it advertises and the frame rate from settings, the recent list is
 * reachable, and an entry whose file has gone stops being offered instead of
 * failing again on the next click.
 */

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useProjectStore } from "@/modules/project/store";
import { StartScreen } from "@/modules/workspace/components/StartScreen";
import { useWorkspaceStore } from "@/modules/workspace/store";
import { DEFAULT_SETTINGS } from "@/modules/workspace/types";
import { makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

const DAY = 24 * 60 * 60 * 1000;

const RECENT = [
  { path: "/home/me/reels/gone.chukcut", name: "Gone", opened_at: Date.now() - 2 * DAY },
  { path: "/home/me/reels/here.chukcut", name: "Here", opened_at: Date.now() - 3 * 60 * 1000 },
];

let ipc: IpcHarness;
const onMoreOptions = vi.fn();
const onOpenSettings = vi.fn();

function mount() {
  return render(<StartScreen onMoreOptions={onMoreOptions} onOpenSettings={onOpenSettings} />);
}

beforeEach(() => {
  onMoreOptions.mockReset();
  onOpenSettings.mockReset();
  ipc = installIpc();
  ipc.handle("workspace_recent_list", RECENT);
  ipc.handle("workspace_recent_record", null);
  // The recovery seam is not registered in Rust yet, which is exactly what a
  // machine running this build sees.
  ipc.fail("workspace_recovery_peek", "Command workspace_recovery_peek not found");
  useWorkspaceStore.setState({
    settings: { ...DEFAULT_SETTINGS, default_fps: 60 },
    recent: [],
    recentStatus: "idle",
    recentError: null,
  });
  useProjectStore.setState({ project: null, path: null, dirty: false, error: null });
});

afterEach(() => {
  ipc.restore();
});

describe("starting something", () => {
  it("offers the three shapes people deliver, at 1080p", async () => {
    mount();

    expect(await screen.findByText("Vertical")).toBeInTheDocument();
    expect(screen.getByText(/1080 × 1920/)).toBeInTheDocument();
    expect(screen.getByText(/1920 × 1080/)).toBeInTheDocument();
    expect(screen.getByText(/1080 × 1080/)).toBeInTheDocument();
  });

  it("creates the project with that canvas and the frame rate from settings", async () => {
    ipc.handle("project_new", makeProject({ name: "Kitchen" }));
    mount();

    await userEvent.clear(screen.getByLabelText("Project name"));
    await userEvent.type(screen.getByLabelText("Project name"), "Kitchen");
    await userEvent.click(screen.getByText("Landscape"));

    await waitFor(() => expect(ipc.count("project_new")).toBe(1));
    expect(ipc.lastCall("project_new")).toEqual({
      name: "Kitchen",
      width: 1920,
      height: 1080,
      fps: 60,
    });
    expect(useProjectStore.getState().project?.name).toBe("Kitchen");
  });

  it("keeps the full canvas list one click away rather than on the screen", async () => {
    mount();

    await userEvent.click(await screen.findByText(/Another canvas/));

    expect(onMoreOptions).toHaveBeenCalledTimes(1);
  });
});

describe("the recent list", () => {
  it("shows what was opened and when", async () => {
    mount();

    expect(await screen.findByText("Here")).toBeInTheDocument();
    expect(screen.getByText("3 minutes ago")).toBeInTheDocument();
    expect(screen.getByText("2 days ago")).toBeInTheDocument();
    // The directory, not the file name again.
    expect(screen.getAllByText("/home/me/reels").length).toBe(2);
  });

  it("says so plainly when there is nothing in it", async () => {
    ipc.handle("workspace_recent_list", []);
    mount();

    expect(await screen.findByText("No recent projects")).toBeInTheDocument();
  });

  it("offers a retry rather than an empty list when it cannot be read", async () => {
    ipc.fail("workspace_recent_list", "the recent file is unreadable");
    mount();

    expect(await screen.findByText("the recent file is unreadable")).toBeInTheDocument();

    ipc.handle("workspace_recent_list", RECENT);
    await userEvent.click(screen.getByRole("button", { name: /Try again/ }));

    expect(await screen.findByText("Here")).toBeInTheDocument();
  });

  it("stops offering an entry whose file has been moved away", async () => {
    ipc.fail("project_open", "cannot read /home/me/reels/gone.chukcut: No such file or directory");
    mount();
    await screen.findByText("Gone");

    await userEvent.click(screen.getByText("Gone"));

    // Rust prunes missing files before it answers, but a project can move
    // between that answer and this click — so the row goes now, not on the next
    // launch.
    await waitFor(() => expect(screen.queryByText("Gone")).toBeNull());
    expect(screen.getByText("Here")).toBeInTheDocument();
    expect(useProjectStore.getState().error).toMatch(/No such file/);
  });

  it("opens the one that is still there", async () => {
    ipc.handle("project_open", makeProject({ name: "Here" }));
    mount();
    await screen.findByText("Here");

    await userEvent.click(screen.getByText("Here"));

    await waitFor(() => expect(useProjectStore.getState().project?.name).toBe("Here"));
    expect(ipc.lastCall("project_open")).toEqual({ path: "/home/me/reels/here.chukcut" });
  });
});
