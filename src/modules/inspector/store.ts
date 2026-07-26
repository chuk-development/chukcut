/**
 * Inspector UI state.
 *
 * Which property the curve view is showing and which keyframe is selected are
 * things the user is looking at, not things they are editing, so they live here
 * and not in the document. Both are held loosely on purpose: the selection is
 * validated against the segment at render time rather than being cleared on
 * every selection change, because an edit replaces the whole document and a
 * store that tried to stay in step with it would be one more thing to desync.
 */

import { create } from "zustand";

import type { Micros } from "@/modules/project/types";

interface InspectorState {
  /** `PropertyDef.id`, not an `AnimatableProperty` — the scale row drives two. */
  curveProperty: string | null;
  /** Segment-relative time of the selected keyframe. */
  selectedKeyframeTime: Micros | null;

  showCurve: (propertyId: string | null) => void;
  selectKeyframe: (time: Micros | null) => void;
}

export const useInspectorStore = create<InspectorState>((set) => ({
  curveProperty: null,
  selectedKeyframeTime: null,

  showCurve: (curveProperty) => set({ curveProperty, selectedKeyframeTime: null }),
  selectKeyframe: (selectedKeyframeTime) => set({ selectedKeyframeTime }),
}));
