/**
 * What the player knows.
 *
 * Almost all of it comes from Rust: the session, the proxy size, the frame that
 * is due, whether it is playing. The frontend owns only the zoom, because that
 * is the one thing the frame server has no opinion about.
 *
 * The playhead lives in the timeline store and is written from the `position`
 * events here. Having two places that believe they own the current time is how
 * a player and a timeline end up disagreeing.
 */

import { create } from "zustand";
import type { PreviewInfo } from "@/modules/preview/lib/api";
import type { Micros } from "@/modules/project/types";

/** `"fit"` scales to the panel; a number is a fixed multiple of the canvas size. */
export type PreviewZoom = "fit" | number;

interface PreviewState {
  /** Null before the first session opens. */
  session: number | null;
  /** Base URL for the current session's frames. */
  frameUrl: string | null;
  width: number;
  height: number;
  fps: number;
  duration: Micros;
  /** The frame the pacer says is due. */
  frame: number;
  playing: boolean;
  /** Set only from `error` events — a dropped frame is not worth telling anyone about. */
  error: string | null;

  zoom: PreviewZoom;

  applyInfo: (info: PreviewInfo) => void;
  setFrame: (frame: number, playing: boolean) => void;
  setPlaying: (playing: boolean) => void;
  setError: (error: string | null) => void;
  setZoom: (zoom: PreviewZoom) => void;
  reset: () => void;
}

export const usePreviewStore = create<PreviewState>((set) => ({
  session: null,
  frameUrl: null,
  width: 0,
  height: 0,
  fps: 30,
  duration: 0,
  frame: 0,
  playing: false,
  error: null,
  zoom: "fit",

  applyInfo: (info) =>
    set({
      session: info.session,
      frameUrl: info.frameUrl,
      width: info.width,
      height: info.height,
      fps: info.fps,
      duration: info.duration,
      frame: info.frame,
      playing: info.playing,
      error: null,
    }),

  setFrame: (frame, playing) => set({ frame, playing }),
  setPlaying: (playing) => set({ playing }),
  setError: (error) => set({ error }),
  setZoom: (zoom) => set({ zoom }),
  reset: () => set({ session: null, frameUrl: null, playing: false, frame: 0 }),
}));
