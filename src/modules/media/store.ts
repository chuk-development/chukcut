/**
 * The media library.
 *
 * Importing is a two-step move that has to stay in this order: Rust adds the
 * material to the project pool, then the frontend re-reads the document. The
 * pool is what `InsertSegment` resolves `material_id` against, so inserting a
 * clip before the store has seen the new material would send Rust a reference
 * the UI cannot render.
 *
 * Imports are not undoable and do not touch the history — see
 * `project_import_media`.
 */

import { create } from "zustand";

import { useThumbnailStore } from "@/modules/media/lib/thumbnails";
import { projectGet, projectImportMedia } from "@/modules/project/lib/api";
import { describeError, useProjectStore } from "@/modules/project/store";
import type { ImportedMaterial } from "@/modules/project/types";

interface MediaState {
  items: ImportedMaterial[];
  importing: boolean;
  /** Why the last import produced nothing, in Rust's own words. */
  error: string | null;

  /**
   * Import every path into the project pool and return the materials, in the
   * order given. Failures are collected rather than thrown: three good files
   * and one corrupt one should import three files.
   */
  importPaths: (paths: string[]) => Promise<ImportedMaterial[]>;
  remove: (id: string) => void;
  clearError: () => void;
}

export const useMediaStore = create<MediaState>((set) => ({
  items: [],
  importing: false,
  error: null,

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
    // rather than patching a material in locally.
    if (imported.length > 0) {
      try {
        const project = await projectGet();
        if (project) useProjectStore.getState().refreshDocument(project);
      } catch (error) {
        failures.push(describeError(error));
      }
    }

    set((state) => {
      // The batch has to be deduplicated against itself as well as against the
      // library: one drop can carry a file and a symlink to it, and Rust answers
      // both with the same pool entry. Two rows sharing an id is a duplicate
      // React key and a tile that cannot be removed.
      const known = new Set(state.items.map((item) => item.id));
      const added: ImportedMaterial[] = [];
      for (const item of imported) {
        if (known.has(item.id)) continue;
        known.add(item.id);
        added.push(item);
      }
      return {
        items: [...state.items, ...added],
        importing: false,
        error: failures.length > 0 ? failures[0] : null,
      };
    });

    // Filmstrips are wanted the moment a tile exists to show one.
    const request = useThumbnailStore.getState().request;
    for (const item of imported) {
      if (item.kind !== "audio") request(item.path);
    }

    return imported;
  },

  /** Removes the library row only. The material stays in the pool, because a clip may reference it. */
  remove: (id) => set((state) => ({ items: state.items.filter((item) => item.id !== id) })),
  clearError: () => set({ error: null }),
}));
