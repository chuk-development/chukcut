/**
 * Settings, from the control to the boundary and back.
 *
 * The store test pins the wire shape; this pins that the controls are actually
 * wired to it — that the panel shows what Rust has rather than the defaults,
 * that changing something writes immediately with no OK button anywhere, and
 * that a refused write visibly bounces instead of leaving a control showing a
 * value the engine never accepted.
 */

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SettingsDialog } from "@/modules/workspace/components/SettingsDialog";
import { useWorkspaceStore } from "@/modules/workspace/store";
import { DEFAULT_SETTINGS, type Settings } from "@/modules/workspace/types";
import { type IpcHarness, installIpc } from "@/test/ipc";

const STORED: Settings = {
  preview_max_edge: 720,
  preview_full_quality: false,
  preview_quality: 88,
  snapping: true,
  default_canvas: [1920, 1080],
  default_fps: 60,
  cache_limit: 4 * 1024 * 1024 * 1024,
};

let ipc: IpcHarness;

function mount() {
  return render(<SettingsDialog open onOpenChange={vi.fn()} />);
}

/** Drive a Radix select to the option named `option`. */
async function choose(trigger: string, option: string | RegExp) {
  await userEvent.click(screen.getByRole("combobox", { name: trigger }));
  await userEvent.click(await screen.findByRole("option", { name: option }));
}

beforeEach(() => {
  ipc = installIpc();
  ipc.handle("workspace_settings_set", null);
  ipc.handle("workspace_cache_size", 1_500_000_000);
  ipc.handle("workspace_cache_clear", null);
  ipc.handle("workspace_log_path", {
    directory: "/home/u/.local/state/chukcut/logs",
    file: "/home/u/.local/state/chukcut/logs/chukcut-2026-07-26.log",
  });
  useWorkspaceStore.setState({
    settings: STORED,
    settingsStatus: "ready",
    settingsError: null,
    cacheSize: null,
    cacheBusy: false,
  });
});

afterEach(() => {
  ipc.restore();
});

describe("the preview section", () => {
  it("shows what is stored, not what the defaults say", async () => {
    mount();

    expect(screen.getByRole("combobox", { name: "Resolution cap" })).toHaveTextContent("720 px");
    expect(screen.getByText("88")).toBeInTheDocument();
    expect(DEFAULT_SETTINGS.preview_max_edge).not.toBe(STORED.preview_max_edge);
  });

  it("writes the whole struct when the cap changes", async () => {
    mount();

    await choose("Resolution cap", "1280 px");

    await waitFor(() => expect(ipc.count("workspace_settings_set")).toBe(1));
    expect(ipc.lastCall("workspace_settings_set")).toEqual({
      settings: { ...STORED, preview_max_edge: 1280 },
    });
  });

  it("stops offering a cap once the preview is pinned to full resolution", async () => {
    mount();

    await userEvent.click(screen.getByRole("switch", { name: "Full resolution" }));

    await waitFor(() =>
      expect(ipc.lastCall("workspace_settings_set")).toEqual({
        settings: { ...STORED, preview_full_quality: true },
      }),
    );
    expect(screen.getByRole("combobox", { name: "Resolution cap" })).toBeDisabled();
  });

  it("says these take effect on the next frame rather than the next launch", () => {
    mount();

    expect(screen.getByText(/preview session restarts/)).toBeInTheDocument();
  });
});

describe("the editing section", () => {
  it("round-trips snapping", async () => {
    mount();
    await userEvent.click(screen.getByRole("tab", { name: "Editing" }));

    await userEvent.click(screen.getByRole("switch", { name: "Snapping" }));

    await waitFor(() =>
      expect(ipc.lastCall("workspace_settings_set")).toEqual({
        settings: { ...STORED, snapping: false },
      }),
    );
    expect(useWorkspaceStore.getState().settings.snapping).toBe(false);
  });

  it("round-trips the canvas that new projects start from", async () => {
    mount();
    await userEvent.click(screen.getByRole("tab", { name: "Editing" }));

    await choose("Canvas", "1080 × 1920");

    await waitFor(() =>
      expect(ipc.lastCall("workspace_settings_set")).toEqual({
        settings: { ...STORED, default_canvas: [1080, 1920] },
      }),
    );
  });

  it("puts the control back when Rust refuses the write", async () => {
    ipc.fail("workspace_settings_set", "the config directory is read-only");
    mount();
    await userEvent.click(screen.getByRole("tab", { name: "Editing" }));

    await userEvent.click(screen.getByRole("switch", { name: "Snapping" }));

    expect(await screen.findByText("the config directory is read-only")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole("switch", { name: "Snapping" })).toHaveAttribute(
        "data-state",
        "checked",
      ),
    );
  });
});

describe("the storage section", () => {
  it("shows what the cache is costing and empties it", async () => {
    mount();
    await userEvent.click(screen.getByRole("tab", { name: "Storage" }));

    expect(await screen.findByText("1.4 GB on disk")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Clear cache/ }));

    await waitFor(() => expect(ipc.count("workspace_cache_clear")).toBe(1));
    expect(await screen.findByText("0 B on disk")).toBeInTheDocument();
  });

  it("round-trips the cache limit, including having none", async () => {
    mount();
    await userEvent.click(screen.getByRole("tab", { name: "Storage" }));

    await choose("Cache limit", "No limit");

    await waitFor(() =>
      expect(ipc.lastCall("workspace_settings_set")).toEqual({
        settings: { ...STORED, cache_limit: 0 },
      }),
    );
  });

  it("names the log file and reveals it", async () => {
    ipc.handle("plugin:opener|reveal_item_in_dir", null);
    mount();
    await userEvent.click(screen.getByRole("tab", { name: "Storage" }));

    expect(
      await screen.findByText("/home/u/.local/state/chukcut/logs/chukcut-2026-07-26.log"),
    ).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Show log/ }));

    await waitFor(() => expect(ipc.count("plugin:opener|reveal_item_in_dir")).toBe(1));
    expect(ipc.lastCall("plugin:opener|reveal_item_in_dir")).toEqual({
      paths: ["/home/u/.local/state/chukcut/logs/chukcut-2026-07-26.log"],
    });
  });

  /**
   * A machine where the log file could not be opened still gets an answer, and
   * the button is not offered — revealing a file that is not there does
   * nothing and reads as broken.
   */
  it("says where the log would be when there is no file to reveal", async () => {
    ipc.handle("workspace_log_path", {
      directory: "/home/u/.local/state/chukcut/logs",
      file: null,
    });
    mount();
    await userEvent.click(screen.getByRole("tab", { name: "Storage" }));

    expect(await screen.findByText(/could not be opened this run/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Show log/ })).toBeDisabled();
  });
});
