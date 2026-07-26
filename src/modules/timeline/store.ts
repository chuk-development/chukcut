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

/** What the razor would cut, given where the pointer is. */
export interface RazorTarget {
  trackId: Id;
  segmentId: Id;
  at: Micros;
}

interface TimelineState {
  /** Pixels per microsecond. */
  zoom: number;
  /** Horizontal scroll offset of the lane viewport, in pixels. */
  scrollX: number;
  playhead: Micros;
  selectedSegmentId: Id | null;
  tool: TimelineTool;
  snapping: boolean;
  /**
   * Where the razor is hovering.
   *
   * In the store rather than in the timeline component for one reason, and it
   * is a performance one: this changes on every pointer move across the lanes,
   * and holding it as component state re-rendered the entire timeline — every
   * lane, every clip — to move a one-pixel line. Only the guides subscribe.
   */
  razorTarget: RazorTarget | null;

  setZoom: (zoom: number) => void;
  zoomBy: (factor: number) => void;
  setScrollX: (scrollX: number) => void;
  setPlayhead: (playhead: Micros) => void;
  select: (segmentId: Id | null) => void;
  setTool: (tool: TimelineTool) => void;
  toggleSnapping: () => void;
  setRazorTarget: (target: RazorTarget | null) => void;
}

export const useTimelineStore = create<TimelineState>((set, get) => ({
  zoom: DEFAULT_ZOOM,
  scrollX: 0,
  playhead: 0,
  selectedSegmentId: null,
  tool: "select",
  snapping: true,
  razorTarget: null,

  setZoom: (zoom) => set({ zoom: clamp(zoom, MIN_ZOOM, MAX_ZOOM) }),
  zoomBy: (factor) => set({ zoom: clamp(get().zoom * factor, MIN_ZOOM, MAX_ZOOM) }),
  setScrollX: (scrollX) => set({ scrollX }),
  setPlayhead: (playhead) => set({ playhead: Math.max(0, Math.round(playhead)) }),
  select: (selectedSegmentId) => set({ selectedSegmentId }),
  setTool: (tool) => set({ tool, razorTarget: tool === "razor" ? get().razorTarget : null }),
  toggleSnapping: () => set({ snapping: !get().snapping }),

  setRazorTarget: (target) => {
    // A pointer move that lands on the same microsecond of the same clip must
    // not publish a new object: subscribers compare by identity, and the razor
    // is driven by a stream of moves most of which change nothing.
    const current = get().razorTarget;
    if (current === target) return;
    if (
      current &&
      target &&
      current.trackId === target.trackId &&
      current.segmentId === target.segmentId &&
      current.at === target.at
    ) {
      return;
    }
    set({ razorTarget: target });
  },
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
