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
import type { ClipboardEntry } from "@/modules/timeline/lib/clipboard";

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
  /**
   * The selected clips, in the order they were added.
   *
   * A list rather than a `Set` because the order carries meaning — the last
   * entry is what a shift-click extends from — and because an array compares by
   * identity in a `useStore` selector, which a rebuilt `Set` would too but
   * without giving anything back.
   *
   * Empty is the ordinary "nothing selected". Every consumer that used to read
   * one id now asks for the sole selection ([`soleSelection`]) and gets `null`
   * when the answer is ambiguous, which is what keeps the inspector from
   * silently editing one of four clips.
   */
  selection: Id[];
  /**
   * Where a shift-click measures from: the clip whose selection was the last
   * deliberate act. Null once the selection is emptied.
   */
  selectionAnchor: Id | null;
  /**
   * Clips that were cut or copied, detached from any document.
   *
   * Deliberately *not* part of the project: a clipboard is not something the
   * user is editing, it must survive closing the document it came from, and
   * putting it in the document would put it in the undo history and in the file
   * on disk. It sits here, with the zoom and the playhead, for the same reason
   * they do — it belongs to the session, and Rust never sends it back.
   *
   * The entries carry whole `Segment`s, so a paste keeps the source range, the
   * transform, the effects and the keyframes of what was copied.
   */
  clipboard: ClipboardEntry[];
  tool: TimelineTool;
  snapping: boolean;
  /**
   * The in and out marks, as the user set them with I and O.
   *
   * A viewing aid like the zoom, not part of the document: they are never
   * persisted, and setting them is not an edit. Either can exist without the
   * other — a lone mark is drawn but selects nothing yet.
   */
  markIn: Micros | null;
  markOut: Micros | null;
  /**
   * The range between the marks, for "export only this range".
   *
   * **Contract with the export module**: non-null exactly when both marks are
   * set and `markIn < markOut`, and then `{start: markIn, end: markOut}`. The
   * export side reads this field defensively by exactly this name and shape —
   * do not rename it, do not add fields, do not make it hold a degenerate
   * range. It is stored rather than derived at the call site so that reading
   * it needs no knowledge of the marks at all.
   */
  exportRange: { start: Micros; end: Micros } | null;
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
  /** Select exactly this clip, or clear the selection. The plain click. */
  select: (segmentId: Id | null) => void;
  /** Replace the selection wholesale: Select All, and a rubber band's release. */
  selectMany: (segmentIds: readonly Id[]) => void;
  /** Ctrl/Cmd+click: add the clip, or take it out if it was already in. */
  toggleSelection: (segmentId: Id) => void;
  /** Shift+click: add a run of clips without disturbing what is already there. */
  extendSelection: (segmentIds: readonly Id[]) => void;
  setClipboard: (entries: ClipboardEntry[]) => void;
  setTool: (tool: TimelineTool) => void;
  toggleSnapping: () => void;
  setRazorTarget: (target: RazorTarget | null) => void;
  /** I: mark in at this instant. */
  setMarkIn: (at: Micros) => void;
  /** O: mark out at this instant. */
  setMarkOut: (at: Micros) => void;
  /** X: clear both marks. */
  clearMarks: () => void;
}

/** The one place the exportRange invariant is written: both marks, in before out. */
function rangeOf(
  markIn: Micros | null,
  markOut: Micros | null,
): { start: Micros; end: Micros } | null {
  if (markIn === null || markOut === null || markIn >= markOut) return null;
  return { start: markIn, end: markOut };
}

export const useTimelineStore = create<TimelineState>((set, get) => ({
  zoom: DEFAULT_ZOOM,
  scrollX: 0,
  playhead: 0,
  selection: [],
  selectionAnchor: null,
  clipboard: [],
  tool: "select",
  snapping: true,
  razorTarget: null,
  markIn: null,
  markOut: null,
  exportRange: null,

  setZoom: (zoom) => set({ zoom: clamp(zoom, MIN_ZOOM, MAX_ZOOM) }),
  zoomBy: (factor) => set({ zoom: clamp(get().zoom * factor, MIN_ZOOM, MAX_ZOOM) }),
  setScrollX: (scrollX) => set({ scrollX }),
  setPlayhead: (playhead) => set({ playhead: Math.max(0, Math.round(playhead)) }),
  select: (segmentId) =>
    set(
      segmentId === null
        ? { selection: [], selectionAnchor: null }
        : { selection: [segmentId], selectionAnchor: segmentId },
    ),

  selectMany: (segmentIds) => {
    const selection = unique(segmentIds);
    // A click on empty space and a rubber band over empty space are the same
    // thing to everything downstream, so both leave the anchor cleared.
    set({ selection, selectionAnchor: selection[selection.length - 1] ?? null });
  },

  toggleSelection: (segmentId) => {
    const current = get().selection;
    if (current.includes(segmentId)) {
      const selection = current.filter((id) => id !== segmentId);
      // The anchor has to leave with the clip it named, or the next shift-click
      // extends from something that is no longer selected.
      const anchor = get().selectionAnchor;
      set({
        selection,
        selectionAnchor: anchor === segmentId ? (selection[selection.length - 1] ?? null) : anchor,
      });
      return;
    }
    set({ selection: [...current, segmentId], selectionAnchor: segmentId });
  },

  extendSelection: (segmentIds) => {
    const selection = unique([...get().selection, ...segmentIds]);
    // The anchor stays where it was: shift-clicking twice extends from the
    // original clip both times, which is what every list in every OS does.
    set({ selection });
  },

  setClipboard: (clipboard) => set({ clipboard }),

  setTool: (tool) => set({ tool, razorTarget: tool === "razor" ? get().razorTarget : null }),
  toggleSnapping: () => set({ snapping: !get().snapping }),

  setMarkIn: (at) => {
    const markIn = Math.max(0, Math.round(at));
    set({ markIn, exportRange: rangeOf(markIn, get().markOut) });
  },

  setMarkOut: (at) => {
    const markOut = Math.max(0, Math.round(at));
    set({ markOut, exportRange: rangeOf(get().markIn, markOut) });
  },

  clearMarks: () => set({ markIn: null, markOut: null, exportRange: null }),

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

function unique(ids: readonly Id[]): Id[] {
  return [...new Set(ids)];
}

/**
 * The one selected clip, or null when nothing is selected — or when several
 * are.
 *
 * Every panel that edits *a* clip asks for this rather than for the first entry
 * of the selection. With four clips selected there is no such thing as "the"
 * clip, and answering with one of them would let the inspector change a
 * property on a clip the user is not looking at.
 */
export function soleSelection(selection: readonly Id[]): Id | null {
  return selection.length === 1 ? selection[0] : null;
}

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
