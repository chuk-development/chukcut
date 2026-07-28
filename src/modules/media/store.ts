/**
 * The media library's session state.
 *
 * The rows themselves are NOT here: the library renders the project's material
 * pool (`libraryItems` over the document), so a saved and reopened project
 * shows its media without this store having seen an import. What lives here is
 * only what the document cannot know — import progress, the last import error,
 * and which pool files are currently gone from disk.
 *
 * Importing is a two-step move that has to stay in this order: Rust adds the
 * material to the project pool, then the frontend re-reads the document. The
 * pool is what `InsertSegment` resolves `material_id` against, so inserting a
 * clip before the store has seen the new material would send Rust a reference
 * the UI cannot render.
 *
 * Imports are not undoable and do not touch the history — see
 * `project_import_media`. Removing a material *is* undoable: it goes through
 * `timeline_apply` like any other document edit, and it never deletes the
 * clips that reference it — they go offline instead.
 */

import { create } from "zustand";

import { mediaMissingFiles } from "@/modules/media/lib/api";
import { libraryPaths, removeMaterialCommand } from "@/modules/media/lib/library";
import { useThumbnailStore } from "@/modules/media/lib/thumbnails";
import { projectGet, projectImportMedia } from "@/modules/project/lib/api";
import { describeError, runEdit, useProjectStore } from "@/modules/project/store";
import type { ImportedMaterial } from "@/modules/project/types";
import { timelineApply } from "@/modules/timeline/lib/api";

interface MediaState {
  importing: boolean;
  /** Why the last import produced nothing, in Rust's own words. */
  error: string | null;
  /**
   * Pool file paths that are gone from disk, sorted. Replaced only when the
   * set actually changes, so memos keyed on it stay stable across re-checks.
   */
  missingPaths: string[];

  /**
   * Import every path into the project pool and return the materials, in the
   * order given. Failures are collected rather than thrown: three good files
   * and one corrupt one should import three files.
   */
  importPaths: (paths: string[]) => Promise<ImportedMaterial[]>;
  /**
   * Remove a material from the project's pool, as one undoable edit. The
   * clips that reference it stay on the timeline and go offline; the file on
   * disk is never touched.
   */
  removeFromProject: (materialId: string) => Promise<void>;
  /**
   * Re-check which pool files exist on disk. Best-effort: a failed check
   * keeps the previous answer rather than flashing everything missing.
   */
  refreshMissing: () => Promise<void>;
  clearError: () => void;
}

export const useMediaStore = create<MediaState>((set, get) => ({
  importing: false,
  error: null,
  missingPaths: [],

  importPaths: async (paths) => {
    if (paths.length === 0) return [];
    set({ importing: true, error: null });

    const imported: ImportedMaterial[] = [];
    const failures: string[] = [];

    for (const path of paths) {
      try {
        imported.push(await projectImportMedia(path));
      } catch (error) {
        failures.push(describeError(error));
      }
    }

    // The pool changed under us; the document is server state, so re-read it
    // rather than patching a material in locally. The library renders the
    // pool, so this is also what makes the new tiles appear.
    if (imported.length > 0) {
      try {
        const project = await projectGet();
        if (project) useProjectStore.getState().refreshDocument(project);
      } catch (error) {
        failures.push(describeError(error));
      }
    }

    set({ importing: false, error: failures.length > 0 ? failures[0] : null });

    // Filmstrips are wanted the moment a tile exists to show one.
    const request = useThumbnailStore.getState().request;
    for (const item of imported) {
      if (item.kind !== "audio") request(item.path);
    }

    return imported;
  },

  removeFromProject: async (materialId) => {
    const project = useProjectStore.getState().project;
    if (!project) return;
    const command = removeMaterialCommand(project, materialId);
    if (!command) return;
    await runEdit(() => timelineApply(command));
  },

  refreshMissing: async () => {
    const paths = libraryPaths(useProjectStore.getState().project);
    let missing: string[];
    try {
      missing = paths.length > 0 ? await mediaMissingFiles(paths) : [];
    } catch {
      // The check is advisory; a failed IPC round must not mark the whole
      // library offline or clear a state that was right a second ago.
      return;
    }
    missing.sort();
    const current = get().missingPaths;
    const changed =
      missing.length !== current.length || missing.some((path, i) => path !== current[i]);
    if (changed) set({ missingPaths: missing });
  },

  clearError: () => set({ error: null }),
}));
