/**
 * Transition UI state.
 *
 * None of this is the document. What is selected, what is being dragged and
 * what the catalogue said are all things the user is looking at; the
 * transitions themselves come from Rust with the project and are replaced
 * wholesale after every edit.
 *
 * A transition is identified by **the segment it is the entrance to**, not by
 * its own id. That is the same handle every command takes, and it is the one
 * that survives an edit: `transitions_set` replaces the material and Rust mints
 * whatever id it likes, so a selection held by material id would go stale the
 * first time the user changed the kind.
 */

import { create } from "zustand";

import type { Id, Micros } from "@/modules/project/types";
import { type TransitionDescriptor, transitionsCatalog } from "@/modules/transitions/lib/api";

interface TransitionState {
  /** The segment whose incoming transition is selected, if any. */
  selectedSegmentId: Id | null;
  /**
   * The duration a drag is currently proposing, so the marker can follow the
   * pointer without a round trip per frame. Cleared when the pointer is
   * released and Rust answers with the real document.
   */
  dragDuration: { segmentId: Id; duration: Micros } | null;
  catalog: TransitionDescriptor[];
  /** Set when a transition edit is refused, for the timeline's error strip. */
  error: string | null;

  select: (segmentId: Id | null) => void;
  setDragDuration: (drag: { segmentId: Id; duration: Micros } | null) => void;
  setError: (error: string | null) => void;
  loadCatalog: () => Promise<void>;
}

export const useTransitionStore = create<TransitionState>((set, get) => ({
  selectedSegmentId: null,
  dragDuration: null,
  catalog: [],
  error: null,

  select: (selectedSegmentId) => set({ selectedSegmentId }),
  setDragDuration: (dragDuration) => set({ dragDuration }),
  setError: (error) => set({ error }),

  /**
   * Fetch the catalogue once.
   *
   * It cannot change while the app is running — it is a table compiled into
   * Rust — so a second call is a no-op rather than a second round trip.
   */
  loadCatalog: async () => {
    if (get().catalog.length > 0) return;
    try {
      set({ catalog: await transitionsCatalog() });
    } catch (error) {
      set({ error: String(error) });
    }
  },
}));

/** The duration to draw for `segmentId`: the drag's, if one is in flight. */
export function pendingDuration(segmentId: Id, committed: Micros): Micros {
  const drag = useTransitionStore.getState().dragDuration;
  return drag && drag.segmentId === segmentId ? drag.duration : committed;
}
