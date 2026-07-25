/**
 * Timeline UI state.
 *
 * None of this is part of the document: zoom, scroll, the playhead and the
 * selection are what the user is looking at, not what they are editing. It
 * survives an edit unchanged because Rust never sends it back.
 */

import { create } from "zustand";

import { clamp } from "@/lib/time";
import type { Id, Micros } from "@/modules/project/types";

/** Pixels per microsecond. 1e-4 is 100 px per second — a comfortable default. */
export const DEFAULT_ZOOM = 1e-4;
/** 2 px per second: an hour fits on screen. */
export const MIN_ZOOM = 2e-6;
/** 4000 px per second: individual frames are ~130 px wide at 30 fps. */
export const MAX_ZOOM = 4e-3;

export type TimelineTool = "select" | "razor";

interface TimelineState {
  /** Pixels per microsecond. */
  zoom: number;
  /** Horizontal scroll offset of the lane viewport, in pixels. */
  scrollX: number;
  playhead: Micros;
  selectedSegmentId: Id | null;
  tool: TimelineTool;
  snapping: boolean;

  setZoom: (zoom: number) => void;
  zoomBy: (factor: number) => void;
  setScrollX: (scrollX: number) => void;
  setPlayhead: (playhead: Micros) => void;
  select: (segmentId: Id | null) => void;
  setTool: (tool: TimelineTool) => void;
  toggleSnapping: () => void;
}

export const useTimelineStore = create<TimelineState>((set, get) => ({
  zoom: DEFAULT_ZOOM,
  scrollX: 0,
  playhead: 0,
  selectedSegmentId: null,
  tool: "select",
  snapping: true,

  setZoom: (zoom) => set({ zoom: clamp(zoom, MIN_ZOOM, MAX_ZOOM) }),
  zoomBy: (factor) => set({ zoom: clamp(get().zoom * factor, MIN_ZOOM, MAX_ZOOM) }),
  setScrollX: (scrollX) => set({ scrollX }),
  setPlayhead: (playhead) => set({ playhead: Math.max(0, Math.round(playhead)) }),
  select: (selectedSegmentId) => set({ selectedSegmentId }),
  setTool: (tool) => set({ tool }),
  toggleSnapping: () => set({ snapping: !get().snapping }),
}));

/** Width of the track-header gutter. Shared by the ruler so ticks line up. */
export const TRACK_HEADER_WIDTH = 150;
export const RULER_HEIGHT = 26;
export const TRACK_HEIGHT: Record<string, number> = {
  video: 56,
  image: 56,
  audio: 44,
  text: 34,
  sticker: 34,
  effect: 28,
};

export function trackHeight(kind: string): number {
  return TRACK_HEIGHT[kind] ?? 44;
}
