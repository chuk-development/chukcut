/**
 * The open document.
 *
 * This store is not client state. The project comes from Rust and is replaced
 * wholesale after every edit — there is no local mutation and no patching, so
 * a desync between the two halves is structurally impossible. Anything the UI
 * derives from the document is computed at read time in `types.ts`.
 */

import { create } from "zustand";

import type { Project } from "@/modules/project/types";
import type { EditResponse } from "@/modules/timeline/lib/api";

export type ProjectStatus = "loading" | "ready" | "error";

interface ProjectState {
  project: Project | null;
  path: string | null;
  status: ProjectStatus;
  /** Last error from Rust, in its own words. Cleared when the user dismisses it. */
  error: string | null;
  dirty: boolean;

  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;

  /** Replace the document after an open/new, which also resets history. */
  loadProject: (project: Project, path?: string | null) => void;
  /** Replace the document after an edit, taking Rust's history state with it. */
  applyEditResponse: (response: EditResponse) => void;
  /**
   * Replace the document after a mutation that bypassed the history — importing
   * media is the only one. History state is left alone because the undo stack
   * genuinely did not move.
   */
  refreshDocument: (project: Project) => void;
  setPath: (path: string | null) => void;
  markSaved: (path: string) => void;
  setStatus: (status: ProjectStatus) => void;
  setError: (error: string | null) => void;
}

export const useProjectStore = create<ProjectState>((set) => ({
  project: null,
  path: null,
  status: "loading",
  error: null,
  dirty: false,
  canUndo: false,
  canRedo: false,
  undoLabel: null,
  redoLabel: null,

  loadProject: (project, path = null) =>
    set({
      project,
      path,
      status: "ready",
      dirty: false,
      canUndo: false,
      canRedo: false,
      undoLabel: null,
      redoLabel: null,
      error: null,
    }),

  applyEditResponse: (response) =>
    set({
      project: response.project,
      status: "ready",
      dirty: true,
      canUndo: response.can_undo,
      canRedo: response.can_redo,
      undoLabel: response.undo_label,
      redoLabel: response.redo_label,
      error: null,
    }),

  refreshDocument: (project) => set({ project, status: "ready", dirty: true }),

  setPath: (path) => set({ path }),
  markSaved: (path) => set({ path, dirty: false }),
  setStatus: (status) => set({ status }),
  setError: (error) => set({ error }),
}));

/**
 * Run an IPC call that returns a new document and fold the result into the
 * store, surfacing the failure instead of throwing.
 *
 * "Target range is occupied" is a normal outcome of dragging a clip onto
 * another one, so a rejected edit is a message, not an exception.
 */
export async function runEdit(call: () => Promise<EditResponse>): Promise<boolean> {
  try {
    const response = await call();
    useProjectStore.getState().applyEditResponse(response);
    return true;
  } catch (error) {
    useProjectStore.getState().setError(describeError(error));
    return false;
  }
}

export function describeError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
