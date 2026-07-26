/**
 * The text module's UI state, and the two gestures that need more than one
 * store to complete.
 *
 * The only real state here is the font list, which is server state that cannot
 * change while the app runs — the system's font collection is scanned once per
 * process in Rust — so it is fetched once and held. Everything else about a
 * title lives in the document.
 */

import { create } from "zustand";

import { describeError, useProjectStore } from "@/modules/project/store";
import type { Micros } from "@/modules/project/types";
import { textAdd, textFonts } from "@/modules/text/lib/api";
import { useTimelineStore } from "@/modules/timeline/store";

interface TextState {
  /** Every family the machine can draw. Empty until `loadFonts` has answered. */
  fonts: string[];
  fontsLoading: boolean;
  /** Set when the font list could not be fetched; the picker falls back to a free-text field. */
  fontsError: string | null;
  /** True while a title is being added, so the button can refuse a second press. */
  adding: boolean;

  loadFonts: () => Promise<void>;
}

export const useTextStore = create<TextState>((set, get) => ({
  fonts: [],
  fontsLoading: false,
  fontsError: null,
  adding: false,

  async loadFonts() {
    if (get().fonts.length > 0 || get().fontsLoading) return;
    set({ fontsLoading: true, fontsError: null });
    try {
      set({ fonts: await textFonts(), fontsLoading: false });
    } catch (error) {
      // Not fatal. A title still renders — `font::family_stack` appends a
      // generic family precisely so an unresolvable name degrades — so the
      // picker degrades to a text field rather than the panel disappearing.
      set({ fontsLoading: false, fontsError: describeError(error) });
    }
  },
}));

/**
 * Add a title at the playhead and select it.
 *
 * Selecting it is the point: the inspector is where a title is written, and a
 * button that adds a clip the user then has to find is a button that gets
 * pressed once. The lane and the instant are Rust's to choose, so the answer
 * carries them back rather than this guessing.
 *
 * Returns the new segment id, or null if the edit was refused.
 */
export async function addTitle(content?: string, at?: Micros): Promise<string | null> {
  if (useTextStore.getState().adding) return null;
  useTextStore.setState({ adding: true });
  try {
    const where = at ?? useTimelineStore.getState().playhead;
    const added = await textAdd(where, content);
    useProjectStore.getState().applyEditResponse(added);
    useTimelineStore.getState().select(added.segment_id);
    return added.segment_id;
  } catch (error) {
    useProjectStore.getState().setError(describeError(error));
    return null;
  } finally {
    useTextStore.setState({ adding: false });
  }
}
