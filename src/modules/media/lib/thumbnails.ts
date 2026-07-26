/**
 * The thumbnail cache.
 *
 * One strip per *material*, not per segment: two clips cut from the same file
 * share a filmstrip, and the strip is indexed by source time, so it survives
 * trimming, splitting and every zoom level without another decode.
 *
 * Decoding a strip is slow on a cold cache, so jobs are queued and run one at a
 * time. Firing three of them at once because three clips mounted would put
 * three decoders on the machine at the same moment as playback.
 *
 * ## Tiles arrive one batch at a time
 *
 * A strip is an array of `STRIP_COUNT` slots, filled in as batches land. Three
 * consequences the components downstream depend on:
 *
 * - **Tiles arrive out of order.** Each carries its own index — workers finish
 *   in whatever order they finish, and a strip whose cached tiles are
 *   interleaved with freshly decoded ones is not a range. Position in the array
 *   *is* position in the file; nothing here reads a strip as "the tiles so far".
 * - **A partial strip is drawable.** A clip renders the tiles it has and a
 *   deliberate placeholder for the rest, which is the point of streaming: the
 *   alternative is a flat rectangle until the last tile lands.
 * - **Failure arrives as a message, not as a rejection.** `media_thumbnails`
 *   answers with a job id long before the decoder gets anywhere, so the only
 *   place a decode failure is ever reported is the terminal batch's `error`.
 *
 * ## Who wants a strip, and what happens when nobody does
 *
 * `retain`/`release` refcount the components that have a strip on screen. When
 * the last one goes — a clip deleted mid-decode, the project closed, a library
 * tab switched away from — the job is cancelled rather than left to finish for
 * a view that no longer exists. Cancellation is deferred by a turn because
 * React unmounts and immediately remounts under StrictMode, and cancelling a
 * decode that is about to be wanted again would make development slower than
 * production for no reason.
 */

import { create } from "zustand";

import { mediaThumbnails, mediaThumbnailsCancel } from "@/modules/media/lib/api";

/** Frames per strip. Twelve is enough to read a clip at a glance without a long decode. */
export const STRIP_COUNT = 12;
/** Tall enough for the video lane at its default height, small enough to decode quickly. */
export const STRIP_HEIGHT = 72;

/**
 * `pending` — asked for, nothing back yet.
 * `partial` — some tiles have landed and more are coming.
 * `ready` — the job finished, whether or not every slot was filled.
 * `failed` — nothing usable arrived, and nothing will. Sticky.
 */
export type StripStatus = "pending" | "partial" | "ready" | "failed";

export interface Strip {
  /**
   * Tile URLs by position across the whole material. A `null` is a tile that
   * has not been decoded *yet*, not a tile that is missing.
   */
  tiles: (string | null)[];
  /** How many slots are filled. */
  received: number;
  status: StripStatus;
  /** Why the decode stopped early, in Rust's own words. Null unless it did. */
  error: string | null;
}

interface ThumbnailState {
  strips: Record<string, Strip>;
  /**
   * Ask for a strip without claiming to be looking at it. For priming a cache
   * on import; a component that draws the strip should `retain` instead, so the
   * job can be cancelled when nothing is left to draw it into.
   */
  request: (path: string) => void;
  /** This component has the strip on screen. */
  retain: (path: string) => void;
  /** It no longer does. Cancels the decode if it was the last one. */
  release: (path: string) => void;
}

const queue: string[] = [];
let draining = false;

/** How many mounted components are drawing each strip. */
const interested = new Map<string, number>();
/** The running job per path, once Rust has told us its id. */
const jobs = new Map<string, string>();

function emptyStrip(): Strip {
  return {
    tiles: new Array<string | null>(STRIP_COUNT).fill(null),
    received: 0,
    status: "pending",
    error: null,
  };
}

export const useThumbnailStore = create<ThumbnailState>((set, get) => ({
  strips: {},

  request: (path) => {
    if (!path) return;
    // Every terminal state is sticky, not just failure: a file that cannot be
    // decoded must not be retried on every re-render for the rest of the
    // session, and one that can is already on its way.
    if (get().strips[path]) return;
    set((state) => ({ strips: { ...state.strips, [path]: emptyStrip() } }));
    queue.push(path);
    void drain();
  },

  retain: (path) => {
    if (!path) return;
    interested.set(path, (interested.get(path) ?? 0) + 1);
    get().request(path);
  },

  release: (path) => {
    if (!path) return;
    const remaining = (interested.get(path) ?? 0) - 1;
    if (remaining > 0) {
      interested.set(path, remaining);
      return;
    }
    interested.delete(path);
    // Deferred: React unmounts and remounts an effect immediately in
    // StrictMode, and a cancel-then-restart there would be a bug that only
    // exists in development.
    setTimeout(() => abandon(path), 0);
  },
}));

/**
 * Stop decoding a strip nobody is looking at any more.
 *
 * The entry is dropped rather than marked, so that a clip dragged back onto the
 * timeline starts a fresh job instead of inheriting a half-finished strip.
 * A strip that already finished is kept: it is a handful of URLs, and throwing
 * it away would mean decoding the file again.
 */
function abandon(path: string): void {
  if ((interested.get(path) ?? 0) > 0) return;

  const strip = useThumbnailStore.getState().strips[path];
  if (!strip || strip.status === "ready" || strip.status === "failed") return;

  const queued = queue.indexOf(path);
  if (queued !== -1) queue.splice(queued, 1);

  useThumbnailStore.setState((state) => {
    const { [path]: _dropped, ...rest } = state.strips;
    return { strips: rest };
  });

  const jobId = jobs.get(path);
  jobs.delete(path);
  // The job answers with a terminal `cancelled` batch, which is what lets the
  // queue move on to the next file.
  if (jobId) void mediaThumbnailsCancel(jobId).catch(() => {});
}

/**
 * Fold one batch's tiles into a strip.
 *
 * Written as a pure function because the cases worth being able to state —
 * tiles out of order, tiles that overlap, a tile past the end of the strip —
 * are all about this merge and nothing else.
 */
function merge(strip: Strip, tiles: { index: number; url: string }[]): Strip {
  const next = strip.tiles.slice();
  let received = strip.received;

  for (const tile of tiles) {
    if (tile.index < 0) continue;
    // A tile past the end means Rust settled on a different count. Grow rather
    // than drop it: the array's length *is* the strip's resolution, and every
    // reader indexes proportionally into it.
    while (next.length <= tile.index) next.push(null);
    if (next[tile.index] === null) received++;
    next[tile.index] = tile.url;
  }

  return { ...strip, tiles: next, received, status: "partial" };
}

async function drain(): Promise<void> {
  if (draining) return;
  draining = true;
  try {
    while (queue.length > 0) {
      const path = queue.shift();
      if (!path) continue;
      await decode(path);
    }
  } finally {
    draining = false;
  }
}

async function decode(path: string): Promise<void> {
  // The entry may have been dropped between being queued and being reached.
  if (!useThumbnailStore.getState().strips[path]) return;

  let settle: () => void = () => {};
  const finished = new Promise<void>((resolve) => {
    settle = resolve;
  });
  let failure: string | null = null;

  const done = () => {
    useThumbnailStore.setState((state) => {
      const strip = state.strips[path];
      if (!strip) return state;
      // Whether Rust filled every slot is not the frontend's business: a short
      // clip may honestly have fewer distinct frames than the strip has slots.
      // Anything at all is a usable filmstrip; nothing at all is a failure.
      // Tiles that arrived before an error are kept — half a strip beats none.
      return {
        strips: {
          ...state.strips,
          [path]: {
            ...strip,
            status: strip.received > 0 ? "ready" : "failed",
            error: failure,
          },
        },
      };
    });
  };

  try {
    const jobId = await mediaThumbnails(path, STRIP_COUNT, STRIP_HEIGHT, (batch) => {
      if (batch.tiles.length > 0) {
        useThumbnailStore.setState((state) => {
          const strip = state.strips[path];
          if (!strip) return state;
          // `at` is deliberately not kept: Rust spreads `count` frames evenly
          // across the file, so a tile's slot already says where it came from,
          // and a second index would be a second thing to keep in step.
          return { strips: { ...state.strips, [path]: merge(strip, batch.tiles) } };
        });
      }
      if (batch.complete) {
        // A cancellation is the user's own doing and is not reported as a
        // failure; the entry is gone by now in any case.
        failure = batch.cancelled ? null : batch.error;
        settle();
      }
    });

    if (jobId) {
      jobs.set(path, jobId);
      // Released while the command was in flight: there was no id to cancel
      // with at the time, so it falls to here.
      if ((interested.get(path) ?? 0) === 0 && !useThumbnailStore.getState().strips[path]) {
        jobs.delete(path);
        void mediaThumbnailsCancel(jobId).catch(() => {});
        return;
      }
      await finished;
    }
    // No job id means nothing was started and no terminal batch is coming.
    // Settling here rather than waiting is what keeps one bad answer from
    // stalling every file queued behind it for the rest of the session.
  } catch (error) {
    // The command itself refused — a bad argument, or the backend gone. The
    // decode failure path is the terminal batch, not this.
    failure = typeof error === "string" ? error : "the thumbnail job could not be started";
  } finally {
    jobs.delete(path);
    done();
  }
}

/**
 * Pick the frame nearest a position in the source file.
 *
 * The strip spans the whole material, so a segment showing seconds 8–12 of a
 * one-minute file indexes into the middle of it rather than the start.
 *
 * Returns `""` when the tile at that position has not been decoded yet —
 * deliberately distinct from a URL, so a caller can draw a placeholder in that
 * slot rather than a frame from somewhere else in the file. A caller that would
 * rather show *something* passes `nearest`, which walks outward to the closest
 * tile that exists.
 */
export function frameAt(
  strip: readonly (string | null)[],
  sourceTime: number,
  materialDuration: number,
  nearest = false,
): string {
  if (strip.length === 0) return "";
  const fraction =
    materialDuration > 0 ? Math.min(1, Math.max(0, sourceTime / materialDuration)) : 0;
  const index = Math.min(strip.length - 1, Math.round(fraction * (strip.length - 1)));

  const exact = strip[index];
  if (exact) return exact;
  if (!nearest) return "";

  for (let offset = 1; offset < strip.length; offset++) {
    const before = strip[index - offset];
    if (before) return before;
    const after = strip[index + offset];
    if (after) return after;
  }
  return "";
}

/** The one frame that stands for a whole file, for a library tile. Null until something decodes. */
export function stripPoster(strip: Strip | undefined): string | null {
  if (!strip) return null;
  // Middle of the strip: the first frame of a video is usually black or a slate.
  return frameAt(strip.tiles, 1, 2, true) || null;
}
