import { type PointerEvent as ReactPointerEvent, useRef, useState } from "react";

import { clamp, formatTimecode, frameDuration } from "@/lib/time";
import { easingApply, snapToFrame } from "@/modules/inspector/lib/keyframes";
import { type PropertyDef, roundToStep } from "@/modules/inspector/lib/properties";
import type { Keyframe, KeyframeTrack, Micros } from "@/modules/project/types";

/**
 * The curve.
 *
 * Time runs left to right across the segment — the clip's own time, since that
 * is what keyframe times are relative to — and the value axis auto-fits the
 * keyframes rather than showing the property's full range, because a rotation
 * animation of four degrees on a -180..180 axis is a flat line.
 *
 * The interpolation is drawn with the same easing function the renderer uses,
 * so a hold reads as a step and an ease-in-out visibly leans. A drag is local
 * state until the pointer comes up, exactly like the timeline's clip drags, and
 * then leaves as one command.
 */

/** Plot area, in the SVG's own units, which are percentages of the box. */
const X0 = 4;
const X1 = 96;
const Y0 = 10;
const Y1 = 90;

export interface KeyframeCurveProps {
  track: KeyframeTrack;
  def: PropertyDef;
  /** Length of the segment; the width of the plot. */
  duration: Micros;
  fps: number;
  /** Playhead, segment-relative. */
  playhead: Micros;
  selectedTime: Micros | null;
  onSelect: (time: Micros | null) => void;
  /** A finished drag. One command, on release. */
  onMove: (keyframe: Keyframe, toTime: Micros, toValue: number) => void;
  /** A click on the plot background, in segment-relative time. */
  onScrub: (time: Micros) => void;
}

interface DragState {
  keyframe: Keyframe;
  time: Micros;
  value: number;
  moved: boolean;
}

export function KeyframeCurve({
  track,
  def,
  duration,
  fps,
  playhead,
  selectedTime,
  onSelect,
  onMove,
  onScrub,
}: KeyframeCurveProps) {
  const plot = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<DragState | null>(null);

  // What the plot shows while a keyframe is being dragged: the document is
  // stale by design until the pointer comes up.
  const keyframes = drag
    ? track.keyframes
        .map((k) =>
          k.time === drag.keyframe.time ? { ...k, time: drag.time, value: drag.value } : k,
        )
        .sort((a, b) => a.time - b.time)
    : track.keyframes;

  // Fitted to the document, not to the drag: an axis that rescaled as a
  // keyframe was dragged would move the value under the pointer.
  const [low, high] = valueDomain(track.keyframes, def);
  const toX = (time: Micros) => X0 + (duration > 0 ? clamp(time / duration, 0, 1) : 0) * (X1 - X0);
  const toY = (value: number) => Y1 - ((value - low) / (high - low)) * (Y1 - Y0);

  const readPointer = (event: ReactPointerEvent, keyframe: Keyframe): DragState | null => {
    const box = plot.current?.getBoundingClientRect();
    if (!box || box.width <= 0) return null;

    const index = track.keyframes.findIndex((k) => k.time === keyframe.time);
    const frame = frameDuration(fps);
    // Neighbours are the bounds: keeping the order here is what keeps the
    // command Rust receives from ever having to reject an overlap.
    const before = index > 0 ? track.keyframes[index - 1].time + frame : 0;
    const after =
      index >= 0 && index < track.keyframes.length - 1
        ? track.keyframes[index + 1].time - frame
        : duration;

    const units = ((event.clientX - box.left) / box.width) * 100;
    const fraction = clamp((units - X0) / (X1 - X0), 0, 1);
    const time = clamp(snapToFrame(fraction * duration, fps), Math.min(before, after), after);

    // Deliberately unclamped vertically: the axis is fitted to the keyframes,
    // so dragging past the top of the box is the only way to take a value
    // beyond the range the track currently covers.
    const height = box.height || 1;
    const vertical = ((event.clientY - box.top) / height) * 100;
    const raw = high - ((vertical - Y0) / (Y1 - Y0)) * (high - low);
    const value = clamp(roundToStep(raw, def.step), def.min, def.max);

    return {
      keyframe,
      time,
      value,
      moved: time !== keyframe.time || value !== keyframe.value,
    };
  };

  const startDrag = (event: ReactPointerEvent, keyframe: Keyframe) => {
    event.stopPropagation();
    event.preventDefault();
    (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
    onSelect(keyframe.time);
    setDrag({ keyframe, time: keyframe.time, value: keyframe.value, moved: false });
  };

  const moveDrag = (event: ReactPointerEvent, keyframe: Keyframe) => {
    if (!drag) return;
    const next = readPointer(event, keyframe);
    if (next) setDrag({ ...next, moved: drag.moved || next.moved });
  };

  const endDrag = (event: ReactPointerEvent) => {
    if (!drag) return;
    (event.currentTarget as HTMLElement).releasePointerCapture?.(event.pointerId);
    if (drag.moved) onMove(drag.keyframe, drag.time, drag.value);
    setDrag(null);
  };

  const scrub = (event: ReactPointerEvent) => {
    const box = plot.current?.getBoundingClientRect();
    if (!box || box.width <= 0) return;
    const units = ((event.clientX - box.left) / box.width) * 100;
    const fraction = clamp((units - X0) / (X1 - X0), 0, 1);
    onScrub(Math.round(fraction * duration));
  };

  const playheadX = toX(clamp(playhead, 0, duration));
  const showPlayhead = playhead >= 0 && playhead <= duration;

  return (
    <div
      ref={plot}
      data-slot="keyframe-curve"
      className="relative h-[96px] w-full select-none overflow-hidden rounded-sm border border-border/60 bg-surface/30"
      onPointerDown={scrub}
    >
      <svg
        viewBox="0 0 100 100"
        preserveAspectRatio="none"
        className="absolute inset-0 size-full"
        role="presentation"
        aria-hidden="true"
      >
        <title>{def.label} curve</title>
        {[Y0, (Y0 + Y1) / 2, Y1].map((y) => (
          <line
            key={y}
            x1={X0}
            x2={X1}
            y1={y}
            y2={y}
            stroke="currentColor"
            strokeWidth={1}
            vectorEffect="non-scaling-stroke"
            className="text-border/70"
          />
        ))}
        {showPlayhead ? (
          <line
            x1={playheadX}
            x2={playheadX}
            y1={0}
            y2={100}
            stroke="currentColor"
            strokeWidth={1}
            vectorEffect="non-scaling-stroke"
            className="text-timeline-playhead"
          />
        ) : null}
        <path
          d={curvePath(keyframes, toX, toY)}
          fill="none"
          stroke="currentColor"
          strokeWidth={1.5}
          strokeLinecap="round"
          strokeLinejoin="round"
          vectorEffect="non-scaling-stroke"
          className="text-primary"
        />
      </svg>

      <span className="pointer-events-none absolute left-1 top-0.5 font-mono text-[9px] text-muted-foreground/70">
        {formatValue(def, high)}
      </span>
      <span className="pointer-events-none absolute bottom-0.5 left-1 font-mono text-[9px] text-muted-foreground/70">
        {formatValue(def, low)}
      </span>

      {track.keyframes.map((keyframe) => {
        const dragging = drag !== null && drag.keyframe.time === keyframe.time;
        const time = dragging ? drag.time : keyframe.time;
        const value = dragging ? drag.value : keyframe.value;
        return (
          <button
            key={keyframe.time}
            type="button"
            data-slot="keyframe-handle"
            data-selected={selectedTime === keyframe.time}
            aria-label={`${def.label} keyframe at ${formatTimecode(keyframe.time, fps)}`}
            className="absolute size-[9px] -translate-x-1/2 -translate-y-1/2 rotate-45 rounded-[1px] border border-panel bg-primary transition-colors hover:bg-primary/80 data-[selected=true]:bg-foreground"
            // Kept inside the box: a handle dragged past the fitted range would
            // otherwise be clipped away mid-gesture.
            style={{ left: `${toX(time)}%`, top: `${clamp(toY(value), 3, 97)}%` }}
            onPointerDown={(event) => startDrag(event, keyframe)}
            onPointerMove={(event) => moveDrag(event, keyframe)}
            onPointerUp={endDrag}
            onClick={(event) => event.stopPropagation()}
          />
        );
      })}
    </div>
  );
}

/**
 * The value range the plot covers.
 *
 * Fitted to the keyframes with a margin, not taken from the property's range: a
 * fade from 0.9 to 1.0 opacity drawn on a 0..1 axis is indistinguishable from
 * no animation at all. A track whose values are all equal gets an arbitrary but
 * stable band around them so the line lands in the middle rather than on an
 * edge.
 */
function valueDomain(keyframes: Keyframe[], def: PropertyDef): [number, number] {
  const values = keyframes.map((k) => k.value);
  const low = Math.min(...values);
  const high = Math.max(...values);
  const span = high - low;
  const pad = span > 0 ? span * 0.2 : Math.max(def.step * 5, (def.max - def.min) * 0.05);
  return [low - pad, high + pad];
}

/** The interpolation, drawn the way the renderer computes it. */
function curvePath(
  keyframes: Keyframe[],
  toX: (time: Micros) => number,
  toY: (value: number) => number,
): string {
  if (keyframes.length === 0) return "";
  const first = keyframes[0];
  const last = keyframes[keyframes.length - 1];
  const at = (x: number, y: number) => `${x.toFixed(2)} ${y.toFixed(2)}`;

  // Before the first keyframe and after the last one the value is held, which
  // is what `KeyframeTrack::sample` does at the ends.
  const parts = [`M ${at(0, toY(first.value))}`, `L ${at(toX(first.time), toY(first.value))}`];

  for (let i = 0; i < keyframes.length - 1; i++) {
    const a = keyframes[i];
    const b = keyframes[i + 1];
    if (a.easing === "hold") {
      parts.push(`L ${at(toX(b.time), toY(a.value))}`, `L ${at(toX(b.time), toY(b.value))}`);
      continue;
    }
    if (a.easing === "linear") {
      parts.push(`L ${at(toX(b.time), toY(b.value))}`);
      continue;
    }
    const steps = 12;
    for (let step = 1; step <= steps; step++) {
      const t = step / steps;
      const time = a.time + (b.time - a.time) * t;
      const value = a.value + (b.value - a.value) * easingApply(a.easing, t);
      parts.push(`L ${at(toX(time), toY(value))}`);
    }
  }

  parts.push(`L ${at(100, toY(last.value))}`);
  return parts.join(" ");
}

function formatValue(def: PropertyDef, value: number): string {
  return def.format ? def.format(value) : value.toFixed(2);
}
