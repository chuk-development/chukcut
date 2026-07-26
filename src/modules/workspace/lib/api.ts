/**
 * Typed wrappers around the `workspace_*` commands.
 *
 * Components call these, never `invoke` directly — one place to change when a
 * signature moves, one place to look when a call misbehaves.
 */

import { invoke } from "@tauri-apps/api/core";

import type { HardwareReport, RecentProject, Settings } from "@/modules/workspace/types";

export function workspaceSettingsGet(): Promise<Settings> {
  return invoke<Settings>("workspace_settings_get");
}

/** Whole-struct write: Rust has no per-field setter, and a partial save would drop fields. */
export function workspaceSettingsSet(settings: Settings): Promise<void> {
  return invoke<void>("workspace_settings_set", { settings });
}

/** Already pruned on the Rust side: entries whose file has gone are dropped before we see them. */
export function workspaceRecentList(): Promise<RecentProject[]> {
  return invoke<RecentProject[]>("workspace_recent_list");
}

/**
 * Note that a project was opened.
 *
 * `now` crosses explicitly rather than being read from the system clock in
 * Rust, so a test can pin it and so the timestamp matches the one the UI is
 * about to render.
 */
export function workspaceRecentRecord(path: string, name: string, now: number): Promise<void> {
  return invoke<void>("workspace_recent_record", { path, name, now });
}

/** Bytes currently held under the cache root. Walks the tree, so not free. */
export function workspaceCacheSize(): Promise<number> {
  return invoke<number>("workspace_cache_size");
}

export function workspaceCacheClear(): Promise<void> {
  return invoke<void>("workspace_cache_clear");
}

/**
 * What this machine can actually do, established by trying it.
 *
 * **This command does not exist in Rust yet.** See `lib/hardware.ts`, which
 * calls it and falls back to the encoder list `export_presets` already carries
 * when it is not there. The expected shape is `HardwareReport`.
 */
export function workspaceHardware(): Promise<HardwareReport> {
  return invoke<HardwareReport>("workspace_hardware");
}
