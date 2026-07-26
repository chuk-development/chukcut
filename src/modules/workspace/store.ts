/**
 * The workspace: settings, recent projects, the cache, the hardware report.
 *
 * Unlike the project document, none of this is server state that gets replaced
 * wholesale — settings are edited here and written through, one whole struct at
 * a time, because Rust has no per-field setter and a partial save would drop
 * the fields it did not know about.
 *
 * Writes are optimistic and reverted on failure. A settings panel where the
 * switch lags a round trip behind the finger feels broken, and the failure it
 * is protecting against — a read-only config directory — is rare enough to be
 * worth an occasional visible bounce.
 */

import { create } from "zustand";

import { describeError } from "@/modules/project/store";
import {
  workspaceCacheClear,
  workspaceCacheSize,
  workspaceRecentList,
  workspaceRecentRecord,
  workspaceSettingsGet,
  workspaceSettingsSet,
} from "@/modules/workspace/lib/api";
import { hardwareStatus } from "@/modules/workspace/lib/hardware";
import {
  DEFAULT_SETTINGS,
  EMPTY_HARDWARE,
  type HardwareStatus,
  type RecentProject,
  type Settings,
} from "@/modules/workspace/types";

export type LoadStatus = "idle" | "loading" | "ready" | "error";

/** What the user chose when told the document has unsaved changes. */
export type DiscardChoice = "save" | "discard" | "cancel";

/**
 * The other end of the open prompt.
 *
 * Outside the store on purpose: a resolver is not state, nothing renders from
 * it, and putting a live continuation in a Zustand slice invites a component to
 * read it. What the store carries is the *question* — which is what the dialog
 * needs to draw itself.
 */
let answerPrompt: ((choice: DiscardChoice) => void) | null = null;

interface WorkspaceState {
  /**
   * What the user is trying to do that would discard their changes, phrased to
   * finish "…before you ". `null` when nothing is being asked.
   */
  discardPrompt: string | null;
  /** Ask, and wait. Resolves with whatever the dialog answers. */
  askBeforeDiscarding: (intent: string) => Promise<DiscardChoice>;
  /** Answer the open prompt. Answering when nothing is open is a no-op. */
  answerDiscardPrompt: (choice: DiscardChoice) => void;

  settings: Settings;
  settingsStatus: LoadStatus;
  /** A write that did not land, in Rust's own words. */
  settingsError: string | null;

  recent: RecentProject[];
  recentStatus: LoadStatus;
  recentError: string | null;

  /** Bytes under the cache root. `null` until asked. */
  cacheSize: number | null;
  cacheBusy: boolean;

  hardware: HardwareStatus;
  hardwareStatus: LoadStatus;
  hardwareError: string | null;

  loadSettings: () => Promise<void>;
  /** Change some fields and write the whole struct back. Reverts if Rust refuses. */
  updateSettings: (patch: Partial<Settings>) => Promise<boolean>;

  refreshRecent: () => Promise<void>;
  recordRecent: (path: string, name: string) => Promise<void>;
  /**
   * Forget one entry locally.
   *
   * Rust prunes entries whose file has gone before it answers, but a project
   * can be moved between that answer and the click — and then the open fails
   * with "cannot read …". Dropping the row there keeps the list honest without
   * a second IPC round trip; the next `workspace_recent_list` agrees.
   */
  dropRecent: (path: string) => void;

  refreshCacheSize: () => Promise<void>;
  clearCache: () => Promise<void>;

  loadHardware: () => Promise<void>;
}

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  discardPrompt: null,

  askBeforeDiscarding: (intent) =>
    new Promise<DiscardChoice>((resolve) => {
      // A second question while one is open would strand the first resolver and
      // leave the caller awaiting forever. The one already on screen wins.
      if (answerPrompt) {
        resolve("cancel");
        return;
      }
      answerPrompt = resolve;
      set({ discardPrompt: intent });
    }),

  answerDiscardPrompt: (choice) => {
    const resolve = answerPrompt;
    answerPrompt = null;
    set({ discardPrompt: null });
    resolve?.(choice);
  },

  settings: DEFAULT_SETTINGS,
  settingsStatus: "idle",
  settingsError: null,

  recent: [],
  recentStatus: "idle",
  recentError: null,

  cacheSize: null,
  cacheBusy: false,

  hardware: EMPTY_HARDWARE,
  hardwareStatus: "idle",
  hardwareError: null,

  loadSettings: async () => {
    set({ settingsStatus: "loading" });
    try {
      set({ settings: await workspaceSettingsGet(), settingsStatus: "ready", settingsError: null });
    } catch (error) {
      // Defaults are already in the store, so the panel stays usable and the
      // user is told why nothing they change will survive a restart.
      set({ settingsStatus: "error", settingsError: describeError(error) });
    }
  },

  updateSettings: async (patch) => {
    const previous = get().settings;
    const next = { ...previous, ...patch };
    set({ settings: next, settingsError: null });
    try {
      await workspaceSettingsSet(next);
      set({ settingsStatus: "ready" });
      return true;
    } catch (error) {
      set({ settings: previous, settingsError: describeError(error) });
      return false;
    }
  },

  refreshRecent: async () => {
    set({ recentStatus: "loading" });
    try {
      set({ recent: await workspaceRecentList(), recentStatus: "ready", recentError: null });
    } catch (error) {
      set({ recentStatus: "error", recentError: describeError(error) });
    }
  },

  recordRecent: async (path, name) => {
    try {
      await workspaceRecentRecord(path, name, Date.now());
      await get().refreshRecent();
    } catch {
      // Losing a line in the recent list is not worth interrupting a save or an
      // open over. The list is a convenience; the document is the work.
    }
  },

  dropRecent: (path) => set({ recent: get().recent.filter((entry) => entry.path !== path) }),

  refreshCacheSize: async () => {
    try {
      set({ cacheSize: await workspaceCacheSize() });
    } catch {
      // Walking the cache tree can fail on a directory we cannot read. The
      // panel shows "unknown" rather than a number that is quietly wrong.
      set({ cacheSize: null });
    }
  },

  clearCache: async () => {
    set({ cacheBusy: true });
    try {
      await workspaceCacheClear();
      set({ cacheSize: 0, settingsError: null });
    } catch (error) {
      set({ settingsError: describeError(error) });
    } finally {
      set({ cacheBusy: false });
    }
  },

  loadHardware: async () => {
    set({ hardwareStatus: "loading" });
    try {
      set({ hardware: await hardwareStatus(), hardwareStatus: "ready", hardwareError: null });
    } catch (error) {
      set({ hardwareStatus: "error", hardwareError: describeError(error) });
    }
  },
}));
