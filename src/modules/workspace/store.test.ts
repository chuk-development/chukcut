/**
 * Settings, as they cross the boundary and come back.
 *
 * The thing worth testing here is not that a switch flips — it is that a change
 * to one field writes the *whole* struct, because Rust has no per-field setter
 * and a partial save would silently drop every preference this build does not
 * know about. And that a refused write puts the old value back, since the
 * control is updated optimistically and would otherwise be lying.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useWorkspaceStore } from "@/modules/workspace/store";
import { DEFAULT_SETTINGS, type Settings } from "@/modules/workspace/types";
import { type IpcHarness, installIpc } from "@/test/ipc";

/** What Rust has on disk in these tests: not the defaults, so adoption is visible. */
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

beforeEach(() => {
  ipc = installIpc();
  useWorkspaceStore.setState({
    settings: DEFAULT_SETTINGS,
    settingsStatus: "idle",
    settingsError: null,
    cacheSize: null,
    cacheBusy: false,
    recent: [],
    recentStatus: "idle",
    recentError: null,
  });
});

afterEach(() => {
  ipc.restore();
});

describe("loading settings", () => {
  it("adopts what Rust has rather than what the defaults say", async () => {
    ipc.handle("workspace_settings_get", STORED);

    await useWorkspaceStore.getState().loadSettings();

    expect(useWorkspaceStore.getState().settings).toEqual(STORED);
    expect(useWorkspaceStore.getState().settingsStatus).toBe("ready");
  });

  it("stays usable on defaults when the settings file cannot be read", async () => {
    ipc.fail("workspace_settings_get", "permission denied");

    await useWorkspaceStore.getState().loadSettings();

    const state = useWorkspaceStore.getState();
    expect(state.settings).toEqual(DEFAULT_SETTINGS);
    expect(state.settingsStatus).toBe("error");
    expect(state.settingsError).toBe("permission denied");
  });
});

describe("changing a setting", () => {
  beforeEach(async () => {
    ipc.handle("workspace_settings_get", STORED);
    ipc.handle("workspace_settings_set", null);
    await useWorkspaceStore.getState().loadSettings();
  });

  it("writes the whole struct back, not the field that changed", async () => {
    await useWorkspaceStore.getState().updateSettings({ snapping: false });

    // A partial write would drop every preference this build does not know
    // about — including ones a newer build wrote.
    expect(ipc.lastCall("workspace_settings_set")).toEqual({
      settings: { ...STORED, snapping: false },
    });
  });

  it("names the argument `settings`, which is what the command takes", async () => {
    await useWorkspaceStore.getState().updateSettings({ preview_quality: 95 });

    expect(Object.keys(ipc.lastCall("workspace_settings_set") ?? {})).toEqual(["settings"]);
  });

  it("keeps Rust's snake_case field names on the way out", async () => {
    await useWorkspaceStore.getState().updateSettings({ preview_max_edge: 1280 });

    const sent = ipc.lastCall("workspace_settings_set")?.settings as Record<string, unknown>;
    for (const key of Object.keys(sent)) expect(key).toBe(key.toLowerCase());
    expect(sent.preview_max_edge).toBe(1280);
  });

  it("sends the canvas as a two-element array, which is how a Rust tuple arrives", async () => {
    await useWorkspaceStore.getState().updateSettings({ default_canvas: [1080, 1080] });

    const sent = ipc.lastCall("workspace_settings_set")?.settings as Settings;
    expect(sent.default_canvas).toEqual([1080, 1080]);
  });

  it("shows the new value immediately rather than after the round trip", async () => {
    let land: (value: unknown) => void = () => {};
    ipc.handle("workspace_settings_set", () => new Promise((resolve) => (land = resolve)));

    const inFlight = useWorkspaceStore.getState().updateSettings({ snapping: false });
    expect(useWorkspaceStore.getState().settings.snapping).toBe(false);

    land(null);
    await inFlight;
    expect(useWorkspaceStore.getState().settings.snapping).toBe(false);
  });

  it("puts the old value back when the write is refused", async () => {
    ipc.fail("workspace_settings_set", "the config directory is read-only");

    const ok = await useWorkspaceStore.getState().updateSettings({ preview_quality: 30 });

    expect(ok).toBe(false);
    expect(useWorkspaceStore.getState().settings.preview_quality).toBe(STORED.preview_quality);
    expect(useWorkspaceStore.getState().settingsError).toBe("the config directory is read-only");
  });
});

describe("the cache", () => {
  it("reports what is on disk and empties it", async () => {
    ipc.handle("workspace_cache_size", 1_500_000_000);
    ipc.handle("workspace_cache_clear", null);

    await useWorkspaceStore.getState().refreshCacheSize();
    expect(useWorkspaceStore.getState().cacheSize).toBe(1_500_000_000);

    await useWorkspaceStore.getState().clearCache();
    expect(useWorkspaceStore.getState().cacheSize).toBe(0);
  });

  it("says the size is unknown rather than zero when the walk fails", async () => {
    ipc.fail("workspace_cache_size", "cannot read the cache directory");

    await useWorkspaceStore.getState().refreshCacheSize();

    // Zero would read as "nothing cached", which is a different and wrong claim.
    expect(useWorkspaceStore.getState().cacheSize).toBeNull();
  });

  it("surfaces a refused clear instead of pretending the cache is empty", async () => {
    ipc.handle("workspace_cache_size", 42);
    ipc.fail("workspace_cache_clear", "device or resource busy");
    await useWorkspaceStore.getState().refreshCacheSize();

    await useWorkspaceStore.getState().clearCache();

    expect(useWorkspaceStore.getState().cacheSize).toBe(42);
    expect(useWorkspaceStore.getState().settingsError).toBe("device or resource busy");
    expect(useWorkspaceStore.getState().cacheBusy).toBe(false);
  });
});

describe("the recent list", () => {
  it("records an open with the millisecond clock Rust sorts by", async () => {
    ipc.handle("workspace_recent_record", null);
    ipc.handle("workspace_recent_list", []);

    await useWorkspaceStore.getState().recordRecent("/home/me/cut.chukcut", "Cut");

    const payload = ipc.lastCall("workspace_recent_record");
    expect(payload?.path).toBe("/home/me/cut.chukcut");
    expect(payload?.name).toBe("Cut");
    expect(typeof payload?.now).toBe("number");
    expect(payload?.now).toBeGreaterThan(1_700_000_000_000);
  });

  it("does not interrupt a save when the recent list cannot be written", async () => {
    ipc.fail("workspace_recent_record", "disk full");

    // Losing a line in a convenience list is not a reason to throw.
    await expect(
      useWorkspaceStore.getState().recordRecent("/home/me/cut.chukcut", "Cut"),
    ).resolves.toBeUndefined();
  });
});
