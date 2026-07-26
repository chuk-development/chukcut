/**
 * The waveform cache.
 *
 * One reduction per *material*, like the filmstrip and for the same reason: two
 * clips cut from the same file share it, and it is indexed by source time, so
 * it survives trimming, splitting and scrolling.
 *
 * Unlike a filmstrip it has a *resolution*, and the resolution the timeline
 * needs changes with the zoom. Two rules follow, and between them they are the
 * whole design of this file:
 *
 * - **Never fetch what would be thrown away.** A waveform is only ever drawn
 *   one column per pixel. Asking Rust for sixteen thousand buckets to draw four
 *   hundred pixels costs a decode and shows the user nothing.
 * - **Never refetch what is already finer than the pixels.** Zooming *in* by a
 *   factor of two on data that was fetched for a deeper zoom must not touch
 *   Rust at all, which is what `needsFetch` is for. Zooming out never fetches:
 *   the data on hand is already too fine, and downsampling it is arithmetic.
 *
 * The ladder is what makes the second rule bite. Quantising the request to
 * powers of two means a drag on the zoom slider produces at most a handful of
 * distinct resolutions instead of one per frame.
 */

import { create } from "zustand";

import { mediaWaveform, type WaveformData } from "@/modules/media/lib/api";

/**
 * The coarsest strip worth having. Below this, a clip zoomed out to a thumb's
 * width still costs a decode, so there is no point asking for less.
 */
export const MIN_WAVEFORM_BUCKETS = 512;
/**
 * The finest. A 4K-wide screen at maximum zoom shows about eight thousand
 * pixels of a clip; sixteen thousand buckets across the whole material is past
 * anything that can be drawn, and every bucket past it is decode time spent on
 * nothing.
 */
export const MAX_WAVEFORM_BUCKETS = 16_384;

/**
 * Round a pixel count up to the ladder.
 *
 * Powers of two, so that a continuous zoom gesture asks for at most five
 * distinct resolutions across the whole range rather than one per frame.
 */
export function waveformResolution(pixels: number): number {
  const wanted = Math.ceil(Math.max(0, pixels));
  let rung = MIN_WAVEFORM_BUCKETS;
  while (rung < wanted && rung < MAX_WAVEFORM_BUCKETS) rung *= 2;
  return Math.min(rung, MAX_WAVEFORM_BUCKETS);
}

export type WaveformStatus = "pending" | "ready" | "failed";

export interface WaveformEntry {
  /** Null until the first answer lands. A finer fetch never blanks it meanwhile. */
  data: WaveformData | null;
  /** The finest resolution asked for so far, in flight or not. */
  requested: number;
  status: WaveformStatus;
}

/**
 * Is a fetch at `wanted` buckets worth making?
 *
 * The short-circuit that matters is the third clause: data already on hand — or
 * already asked for — at this resolution or better is finer than the pixels it
 * is about to be drawn into, and refetching it would produce an identical
 * picture at the cost of a decode.
 */
export function needsFetch(entry: WaveformEntry | undefined, wanted: number): boolean {
  if (!entry) return true;
  // Sticky, like a failed filmstrip: a file with no decodable audio should not
  // be re-decoded every time the user zooms.
  if (entry.status === "failed") return false;
  return entry.requested < wanted;
}

interface WaveformState {
  entries: Record<string, WaveformEntry>;
  /**
   * Ask for a waveform fine enough to fill `pixels` columns across the whole
   * material. Cheap, idempotent and safe to call from an effect on every zoom
   * change: everything that would not change the picture is dropped here.
   */
  request: (path: string, pixels: number) => void;
}

/**
 * Path to the finest resolution still wanted for it. A Map rather than a list
 * so that three zoom steps while one decode is in flight collapse into one
 * fetch at the finest of them, instead of three fetches of which two are stale
 * before they start.
 */
const queue = new Map<string, number>();
let draining = false;

export const useWaveformStore = create<WaveformState>((set, get) => ({
  entries: {},

  request: (path, pixels) => {
    if (!path) return;
    const wanted = waveformResolution(pixels);
    const entry = get().entries[path];
    if (!needsFetch(entry, wanted)) return;

    set((state) => ({
      entries: {
        ...state.entries,
        // The coarse data stays on screen while the finer fetch runs. Blanking
        // the clip on every zoom step would be a worse picture than a slightly
        // soft one.
        [path]: { data: entry?.data ?? null, requested: wanted, status: "pending" },
      },
    }));
    queue.set(path, Math.max(queue.get(path) ?? 0, wanted));
    void drain();
  },
}));

async function drain(): Promise<void> {
  if (draining) return;
  draining = true;
  try {
    while (queue.size > 0) {
      const [path, buckets] = queue.entries().next().value as [string, number];
      queue.delete(path);
      await reduce(path, buckets);
    }
  } finally {
    draining = false;
  }
}

async function reduce(path: string, buckets: number): Promise<void> {
  try {
    const data = await mediaWaveform(path, buckets);
    useWaveformStore.setState((state) => {
      const previous = state.entries[path];
      if (data.buckets <= 0) {
        return {
          entries: {
            ...state.entries,
            [path]: {
              data: previous?.data ?? null,
              requested: previous?.requested ?? buckets,
              status: previous?.data ? "ready" : "failed",
            },
          },
        };
      }
      // A late coarse answer must not replace a finer one that has already
      // landed — with a queue that collapses requests this is unlikely, but the
      // consequence would be a waveform that got worse while the user zoomed in.
      const keep = previous?.data && previous.data.buckets > data.buckets ? previous.data : data;
      return {
        entries: {
          ...state.entries,
          [path]: {
            data: keep,
            requested: Math.max(previous?.requested ?? 0, buckets, keep.buckets),
            status: "ready",
          },
        },
      };
    });
  } catch {
    // No audio, or a file FFmpeg cannot open. Cosmetic: the clip keeps whatever
    // it had and stops asking.
    useWaveformStore.setState((state) => {
      const previous = state.entries[path];
      return {
        entries: {
          ...state.entries,
          [path]: {
            data: previous?.data ?? null,
            requested: previous?.requested ?? buckets,
            status: "failed",
          },
        },
      };
    });
  }
}
