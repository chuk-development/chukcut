/**
 * The thumbnail cache.
 *
 * One strip per *material*, not per segment: two clips cut from the same file
 * share a filmstrip, and the strip is indexed by source time, so it survives
 * trimming, splitting and every zoom level without another decode.
 *
 * `media_thumbnails` is synchronous in Rust and blocks on a cold cache, so
 * requests are queued and run one at a time. Firing three of them at once
 * because three clips mounted would freeze the backend for as long as it takes
 * to decode all three.
 */

import { create } from "zustand";

import { mediaThumbnails } from "@/modules/media/lib/api";

/** Frames per strip. Twelve is enough to read a clip at a glance without a long decode. */
export const STRIP_COUNT = 12;
/** Tall enough for the video lane at its default height, small enough to decode quickly. */
export const STRIP_HEIGHT = 72;

type Status = "idle" | "pending" | "ready" | "failed";

interface ThumbnailState {
  /** Asset URLs per media path, in timeline order across the whole file. */
  strips: Record<string, string[]>;
  status: Record<string, Status>;
  /** Ask for a strip. Cheap and idempotent: repeat calls while one is in flight do nothing. */
  request: (path: string) => void;
}

const queue: string[] = [];
let draining = false;

export const useThumbnailStore = create<ThumbnailState>((set, get) => ({
  strips: {},
  status: {},

  request: (path) => {
    if (!path) return;
    const status = get().status[path];
    // "failed" is sticky on purpose: a file that cannot be decoded should not
    // be retried on every re-render for the rest of the session.
    if (status) return;
    set((state) => ({ status: { ...state.status, [path]: "pending" } }));
    queue.push(path);
    void drain();
  },
}));

async function drain(): Promise<void> {
  if (draining) return;
  draining = true;
  try {
    while (queue.length > 0) {
      const path = queue.shift();
      if (!path) continue;
      try {
        const urls = await mediaThumbnails(path, STRIP_COUNT, STRIP_HEIGHT);
        useThumbnailStore.setState((state) => ({
          strips: { ...state.strips, [path]: urls },
          status: { ...state.status, [path]: urls.length > 0 ? "ready" : "failed" },
        }));
      } catch {
        // A missing strip is cosmetic. The clip falls back to its flat colour
        // and the user can still edit, which is the part that matters.
        useThumbnailStore.setState((state) => ({
          status: { ...state.status, [path]: "failed" },
        }));
      }
    }
  } finally {
    draining = false;
  }
}

/**
 * Pick the frame nearest a position in the source file.
 *
 * The strip spans the whole material, so a segment showing seconds 8–12 of a
 * one-minute file indexes into the middle of it rather than the start.
 */
export function frameAt(strip: string[], sourceTime: number, materialDuration: number): string {
  if (strip.length === 0) return "";
  if (materialDuration <= 0) return strip[0];
  const fraction = Math.min(1, Math.max(0, sourceTime / materialDuration));
  const index = Math.min(strip.length - 1, Math.round(fraction * (strip.length - 1)));
  return strip[index];
}
