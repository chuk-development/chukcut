/**
 * One cut's worth of transition UI: the badge over the window, or the button
 * that makes one.
 *
 * Both live at the same place — the cut — because they are two states of one
 * affordance. A cut with no transition offers a button; a cut with one offers a
 * badge you can select, retime by dragging either edge, and delete.
 *
 * ## Why the drag is not a `moveSegment`
 *
 * Nothing moves. A transition is centred on the cut and its length is the only
 * thing a drag changes, so the pointer's distance from the cut *is* half the
 * duration and the clips either side stay exactly where they are. That is the
 * whole gesture, and it is why this component needs no snapping, no collision
 * check and no lane hit-testing.
 */

import { PlusIcon } from "lucide-react";
import { useCallback, useRef, useState } from "react";

import { IconTooltip } from "@/components/ui/tooltip";
import { formatDuration } from "@/lib/time";
import { cn } from "@/lib/utils";
import type { Micros } from "@/modules/project/types";
import { addTransition, removeTransition, retimeTransition } from "@/modules/transitions/lib/edits";
import {
  durationFromDrag,
  type Join,
  markerBox,
  windowFor,
} from "@/modules/transitions/lib/geometry";
import { useTransitionStore } from "@/modules/transitions/store";

export interface TransitionMarkerProps {
  join: Join;
  /** Pixels per microsecond. */
  zoom: number;
  /** Lane height, so the badge can centre itself in it. */
  laneHeight: number;
  /** Pixel to microsecond, for the drag. Supplied by the timeline, which owns
   * the scroll offset this depends on. */
  timeAtClientX: (clientX: number) => Micros;
}

/** The badge is a fixed height whatever the lane is. */
const BADGE_HEIGHT = 16;

export function TransitionMarker({ join, zoom, laneHeight, timeAtClientX }: TransitionMarkerProps) {
  const selected = useTransitionStore((state) => state.selectedSegmentId === join.to.id);
  const select = useTransitionStore((state) => state.select);
  const setDragDuration = useTransitionStore((state) => state.setDragDuration);
  const [dragging, setDragging] = useState<Micros | null>(null);
  const moved = useRef(false);

  const onDragStart = useCallback(
    (event: React.PointerEvent) => {
      if (event.button !== 0) return;
      event.stopPropagation();
      event.preventDefault();
      moved.current = false;
      const id = join.to.id;

      const onMove = (move: PointerEvent) => {
        const duration = durationFromDrag(join.cut, timeAtClientX(move.clientX), join.max);
        moved.current = true;
        setDragging(duration);
        setDragDuration({ segmentId: id, duration });
      };
      const onUp = (up: PointerEvent) => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        const duration = durationFromDrag(join.cut, timeAtClientX(up.clientX), join.max);
        setDragging(null);
        setDragDuration(null);
        // A press with no movement is a selection, not a zero-length drag —
        // otherwise clicking the badge would delete it.
        if (!moved.current) {
          select(id);
          return;
        }
        void retimeTransition(id, duration);
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [join.cut, join.max, join.to.id, select, setDragDuration, timeAtClientX],
  );

  // The cut with nothing on it: the button that makes a transition.
  if (!join.transition) {
    return (
      <IconTooltip label="Add a transition">
        <button
          type="button"
          data-slot="transition-add"
          aria-label={`Add a transition between ${join.from.id} and ${join.to.id}`}
          className={cn(
            "absolute z-20 grid place-items-center rounded-full border border-border/70",
            "bg-panel/90 text-muted-foreground opacity-0 transition-opacity",
            "hover:opacity-100 focus-visible:opacity-100 group-hover/lane:opacity-100",
          )}
          style={{
            left: join.cut * zoom - BADGE_HEIGHT / 2,
            top: (laneHeight - BADGE_HEIGHT) / 2,
            width: BADGE_HEIGHT,
            height: BADGE_HEIGHT,
          }}
          onClick={(event) => {
            event.stopPropagation();
            void addTransition(join.to.id);
          }}
        >
          <PlusIcon className="size-3" />
        </button>
      </IconTooltip>
    );
  }

  // While a drag is in flight the badge follows the pointer rather than the
  // document: the edit is only sent when the pointer is released, so without
  // this the handle would stick to the old length for the whole gesture.
  const shown =
    dragging === null
      ? join
      : {
          ...join,
          window: windowFor(join.cut, dragging, join.from, join.to),
        };
  const box = markerBox(shown, zoom);
  const duration = dragging ?? join.transition.duration;

  return (
    <IconTooltip label={`${join.transition.kind} · ${formatDuration(duration)}`}>
      {/* A button rather than a styled div: it is focusable, it announces
          itself, and pressing it selects the transition — which is exactly what
          the pointer gesture does when it does not turn into a drag. */}
      <button
        type="button"
        data-slot="transition-marker"
        data-kind={join.transition.kind}
        data-selected={selected}
        aria-label={`Transition into ${join.to.id}`}
        className={cn(
          "absolute z-20 cursor-ew-resize rounded-sm border",
          "bg-track-effect/60 border-track-effect",
          selected && "ring-1 ring-ring",
        )}
        style={{
          left: box.left,
          width: box.width,
          top: (laneHeight - BADGE_HEIGHT) / 2,
          height: BADGE_HEIGHT,
        }}
        onPointerDown={onDragStart}
        onClick={(event) => {
          // The pointer handler above already decided between select and
          // retime; this is only here so a keyboard activation selects too.
          event.stopPropagation();
          select(join.to.id);
        }}
        onDoubleClick={(event) => {
          event.stopPropagation();
          void removeTransition(join.to.id);
        }}
      >
        {/* The bow-tie: two triangles meeting at the cut, which is what every
            editor draws for a crossfade and what makes the direction of the
            effect legible at a glance. */}
        <svg
          className="h-full w-full text-track-effect-foreground/80"
          viewBox="0 0 100 100"
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          <path d="M0 0 L100 100 L100 0 L0 100 Z" fill="currentColor" opacity="0.35" />
        </svg>
      </button>
    </IconTooltip>
  );
}
