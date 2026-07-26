/**
 * The waveform drawn on a clip.
 *
 * A canvas rather than DOM nodes, for the obvious reason: a clip at high zoom
 * is two thousand columns wide, and two thousand `<span>`s per clip on a
 * timeline with fifty of them is tens of thousands of layout boxes to move
 * every time anything changes.
 *
 * Two things this component does *not* do, both on purpose:
 *
 * - **It does not re-request on every render.** The resolution it needs comes
 *   from the zoom, is quantised to a ladder, and the store drops everything
 *   that would not change the picture.
 * - **It does not repaint on every render.** The draw happens in an effect
 *   keyed on the numbers the picture is made of, so a pointer move that
 *   re-renders the clip for some other reason costs nothing at all.
 */

import { memo, useEffect, useRef } from "react";
import { cn } from "@/lib/utils";
import { useWaveformStore } from "@/modules/media/lib/waveform";
import type { Micros } from "@/modules/project/types";
import { bucketsForClip, waveformColumns } from "@/modules/timeline/lib/waveform";

/**
 * Colours, when the stylesheet is not there to answer.
 *
 * A canvas cannot take a class, so the tokens have to be resolved to strings
 * before they can be painted; these are the fallbacks for the one context where
 * that returns nothing, which is jsdom. They are not a second palette — if a
 * token changes and these do not, only a test looks different.
 */
const FALLBACK: Record<string, string> = {
  "--timeline-waveform": "#8fd6ad",
  "--timeline-waveform-rms": "#e8fbef",
  "--timeline-waveform-muted": "#8a8a8a",
  "--timeline-waveform-backdrop": "rgba(0, 0, 0, 0.45)",
};

/**
 * The tokens, resolved once.
 *
 * `getComputedStyle` is not cheap and a timeline at high zoom has one of these
 * canvases per clip; asking the engine for four custom properties per clip per
 * repaint showed up as the most expensive thing in the draw. The timeline
 * palette is defined on `:root` only and has no light-theme override, so one
 * resolution is the same answer every canvas would have got.
 */
const resolved = new Map<string, string>();

function token(name: string): string {
  const cached = resolved.get(name);
  if (cached !== undefined) return cached;
  const root = typeof document !== "undefined" ? document.documentElement : null;
  const value = root ? getComputedStyle(root).getPropertyValue(name).trim() : "";
  // Only a real answer is cached. An empty one means the stylesheet has not
  // been applied yet, and caching the fallback then would keep the waveform on
  // the wrong colour for the rest of the session.
  if (value) resolved.set(name, value);
  return value || FALLBACK[name];
}

export interface WaveformProps {
  /** The material's file, which is what the cache is keyed by. */
  path: string;
  /** Length of the whole source file. */
  materialDuration: Micros;
  /** Where in the file this clip starts. */
  sourceStart: Micros;
  speed: number;
  /** Pixels per timeline microsecond. */
  zoom: number;
  /** First pixel of the clip to draw, from its left edge. */
  fromPx: number;
  /** How many pixels to draw. Clamped to the visible window by the caller. */
  widthPx: number;
  height: number;
  /** Distance from the clip's top edge. */
  topPx: number;
  /**
   * `full` fills the clip, for an audio lane. `strip` is the slim band along
   * the bottom of a video clip, which is what makes it possible to cut on a
   * beat without moving the clip to its own lane first.
   */
  variant: "full" | "strip";
  /** The track or the clip is silent. The shape stays; the colour stops claiming to be audible. */
  muted: boolean;
}

function WaveformCanvas({
  path,
  materialDuration,
  sourceStart,
  speed,
  zoom,
  fromPx,
  widthPx,
  height,
  topPx,
  variant,
  muted,
}: WaveformProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const entry = useWaveformStore((state) => state.entries[path]);
  const request = useWaveformStore((state) => state.request);
  const data = entry?.data ?? null;

  const wanted = bucketsForClip({ materialDuration, zoom, speed });

  // Off the render path: a fetch is a side effect, never something a render
  // triggers synchronously. Cheap to run on every zoom change because the store
  // drops the ones that would not change the picture.
  useEffect(() => {
    request(path, wanted);
  }, [path, wanted, request]);

  const width = Math.max(0, Math.ceil(widthPx));

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || width <= 0 || height <= 0) return;

    // Backing store in device pixels, CSS box in layout pixels: a 1px column on
    // a HiDPI screen is otherwise drawn blurry across two.
    const ratio = Math.min(3, Math.max(1, window.devicePixelRatio || 1));
    const deviceWidth = Math.round(width * ratio);
    const deviceHeight = Math.round(height * ratio);
    if (canvas.width !== deviceWidth) canvas.width = deviceWidth;
    if (canvas.height !== deviceHeight) canvas.height = deviceHeight;

    const context = canvas.getContext("2d");
    if (!context) return;

    context.clearRect(0, 0, deviceWidth, deviceHeight);

    if (variant === "strip") {
      // The band sits on top of the filmstrip, so it needs something to sit on.
      context.fillStyle = token("--timeline-waveform-backdrop");
      context.fillRect(0, 0, deviceWidth, deviceHeight);
    }

    if (!data) return;

    const columns = waveformColumns({
      data,
      materialDuration,
      sourceStart,
      speed,
      zoom,
      fromPx,
      widthPx: width,
      step: 1,
    });
    if (columns.length === 0) return;

    const middle = deviceHeight / 2;
    const half = deviceHeight / 2;
    const envelope = token(muted ? "--timeline-waveform-muted" : "--timeline-waveform");

    // Envelope first, body over it. Both are drawn as one fillRect per column
    // rather than as a path: a path of four thousand points costs more to
    // rasterise than the rectangles do, and the rectangles cannot self-
    // intersect at a discontinuity.
    context.fillStyle = envelope;
    context.globalAlpha = muted ? 0.5 : 0.75;
    for (const column of columns) {
      const top = middle - column.max * half;
      const bottom = middle - column.min * half;
      const x = Math.round((column.x - fromPx) * ratio);
      // Silence still draws a hairline, so an empty passage reads as "there is
      // audio here and it is quiet" rather than as a gap in the data.
      context.fillRect(x, top, ratio, Math.max(ratio, bottom - top));
    }

    if (data.rms) {
      context.fillStyle = token(muted ? "--timeline-waveform-muted" : "--timeline-waveform-rms");
      context.globalAlpha = muted ? 0.65 : 1;
      for (const column of columns) {
        const top = middle - column.rms * half;
        const x = Math.round((column.x - fromPx) * ratio);
        context.fillRect(x, top, ratio, Math.max(ratio, column.rms * half * 2));
      }
    }

    context.globalAlpha = 1;
  }, [data, materialDuration, sourceStart, speed, zoom, fromPx, width, height, variant, muted]);

  if (width <= 0 || height <= 0) return null;

  return (
    <canvas
      ref={canvasRef}
      data-slot="waveform"
      data-status={entry?.status ?? "idle"}
      aria-hidden
      className={cn("pointer-events-none absolute", variant === "strip" && "rounded-b-[2px]")}
      style={{ left: fromPx, top: topPx, width, height }}
    />
  );
}

export const Waveform = memo(WaveformCanvas);
