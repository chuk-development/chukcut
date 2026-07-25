import { useMemo } from "react";

import { formatRulerLabel, formatTimecode, frameDuration, MICROS_PER_SECOND } from "@/lib/time";
import type { Micros } from "@/modules/project/types";
import { RULER_HEIGHT } from "@/modules/timeline/store";

/** A major tick needs this much room before its label starts colliding. */
const MIN_MAJOR_SPACING_PX = 76;
const MIN_MINOR_SPACING_PX = 7;

const FRAME_MULTIPLES = [1, 2, 5, 10, 15, 30, 60];
const SECOND_MULTIPLES = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200];

export interface TickScale {
  major: Micros;
  minor: Micros;
  /** Below one second the ruler counts frames, and the labels grow a frame field. */
  frameScale: boolean;
}

/**
 * Pick a tick interval whose labels are legible at the current zoom.
 *
 * The ladder is frames first, then seconds: zoomed in far enough that a second
 * is wider than the panel, counting seconds tells you nothing, and the only
 * useful unit left is the one the renderer actually produces.
 */
export function chooseTickScale(zoom: number, fps: number): TickScale {
  const frame = frameDuration(fps);

  for (const multiple of FRAME_MULTIPLES) {
    const step = frame * multiple;
    // Stop well short of a second. A whole second's worth of frames is not the
    // same number of microseconds as a second at 30 fps, and labelling ticks
    // 999990 µs apart makes the frame field read 29, 30, 30 instead of rolling.
    if (step * 2 > MICROS_PER_SECOND) break;
    if (step * zoom >= MIN_MAJOR_SPACING_PX) {
      return { major: step, minor: frame, frameScale: true };
    }
  }

  for (const multiple of SECOND_MULTIPLES) {
    const step = MICROS_PER_SECOND * multiple;
    if (step * zoom >= MIN_MAJOR_SPACING_PX) {
      return { major: step, minor: step / 5, frameScale: false };
    }
  }

  const fallback = MICROS_PER_SECOND * 7200;
  return { major: fallback, minor: fallback / 5, frameScale: false };
}

interface TimeRulerProps {
  zoom: number;
  fps: number;
  /** Total width of the scrollable content, in pixels. */
  width: number;
  /** Current horizontal scroll, used to render only the ticks in view. */
  scrollX: number;
  viewportWidth: number;
}

export function TimeRuler({ zoom, fps, width, scrollX, viewportWidth }: TimeRulerProps) {
  const scale = useMemo(() => chooseTickScale(zoom, fps), [zoom, fps]);

  const ticks = useMemo(() => {
    const from = Math.max(0, (scrollX - 200) / zoom);
    const to = (scrollX + Math.max(viewportWidth, 400) + 200) / zoom;
    const drawMinor = scale.minor * zoom >= MIN_MINOR_SPACING_PX;
    const step = drawMinor ? scale.minor : scale.major;

    const out: { time: Micros; major: boolean }[] = [];
    const first = Math.floor(from / step) * step;
    // A pathological zoom/viewport combination must not lock up the UI.
    for (let time = first, guard = 0; time <= to && guard < 2000; time += step, guard++) {
      if (time < 0) continue;
      out.push({ time, major: Math.abs(time % scale.major) < 1 });
    }
    return out;
  }, [zoom, scrollX, viewportWidth, scale]);

  return (
    <div
      className="sticky top-0 z-20 border-b border-border bg-timeline-ruler"
      style={{ width, height: RULER_HEIGHT }}
      data-slot="time-ruler"
    >
      {ticks.map((tick) => (
        <div
          key={tick.time}
          className={
            tick.major
              ? "absolute bottom-0 w-px bg-muted-foreground/50"
              : "absolute bottom-0 w-px bg-muted-foreground/25"
          }
          style={{ left: tick.time * zoom, height: tick.major ? 9 : 5 }}
        >
          {tick.major ? (
            <span className="absolute -top-[15px] left-1 whitespace-nowrap text-[10px] leading-none tracking-tight text-muted-foreground">
              {scale.frameScale ? formatTimecode(tick.time, fps) : formatRulerLabel(tick.time)}
            </span>
          ) : null}
        </div>
      ))}
    </div>
  );
}
