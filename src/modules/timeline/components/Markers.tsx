/**
 * The ruler's markers, and the in/out range shading.
 *
 * Markers live in the document; the in/out marks live in the timeline store
 * (a viewing aid, like the zoom). Both are drawn into the ruler strip, which
 * is otherwise one big scrub target — so the marker flags sit *above* the
 * scrub button in z-order and stop their pointer events, while the range
 * shading sits below it and takes no events at all.
 */

import { PencilIcon, Trash2Icon } from "lucide-react";
import { useState } from "react";

import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { preview } from "@/modules/preview/lib/session";
import { useProjectStore } from "@/modules/project/store";
import type { Marker, MarkerColor, Micros } from "@/modules/project/types";
import { markersOf } from "@/modules/project/types";
import {
  deleteMarker,
  MARKER_COLOR_ORDER,
  MARKER_COLORS,
  moveMarker,
  recolorMarker,
  renameMarker,
} from "@/modules/timeline/lib/markers";
import { buildSnapContext, snapInstant, snapRadius } from "@/modules/timeline/lib/snapping";
import { RULER_HEIGHT, useTimelineStore } from "@/modules/timeline/store";

/** How far a press may wobble and still be a click that jumps the playhead. */
const CLICK_SLOP_PX = 3;

function jumpTo(time: Micros) {
  useTimelineStore.getState().setPlayhead(time);
  void preview.seek(time);
}

function MarkerFlag({
  marker,
  zoom,
  onRename,
}: {
  marker: Marker;
  zoom: number;
  onRename: (marker: Marker) => void;
}) {
  /** Live position while dragging, in micros. Null when at rest. */
  const [dragTime, setDragTime] = useState<Micros | null>(null);
  const time = dragTime ?? marker.time;

  const beginDrag = (event: React.PointerEvent) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const startX = event.clientX;
    // Markers snap to what everything else snaps to — clip edges, the
    // playhead, the other markers — because lining a marker up with a cut is
    // most of what dragging one is for.
    const project = useProjectStore.getState().project;
    const state = useTimelineStore.getState();
    const others = markersOf(project)
      .filter((other) => other.id !== marker.id)
      .map((other) => other.time);
    const context = buildSnapContext(project, state.playhead, null, others);
    const radius = snapRadius(zoom);
    let live: Micros | null = null;

    const onMove = (moveEvent: PointerEvent) => {
      const wanted = Math.max(0, marker.time + Math.round((moveEvent.clientX - startX) / zoom));
      const hit = state.snapping && !moveEvent.altKey ? snapInstant(wanted, context, radius) : null;
      live = hit?.value ?? wanted;
      setDragTime(live);
    };
    const onUp = (upEvent: PointerEvent) => {
      window.removeEventListener("pointermove", onMove);
      setDragTime(null);
      const moved = Math.abs(upEvent.clientX - startX) > CLICK_SLOP_PX && live !== null;
      if (moved && live !== null && live !== marker.time) {
        void moveMarker(marker, live);
      } else {
        // A click: jump the playhead to the marker.
        jumpTo(marker.time);
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp, { once: true });
  };

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <button
          type="button"
          data-slot="timeline-marker"
          data-marker-id={marker.id}
          title={marker.label || "Marker"}
          aria-label={marker.label ? `Marker: ${marker.label}` : "Marker"}
          onPointerDown={beginDrag}
          className="absolute z-30 flex -translate-x-1/2 cursor-ew-resize flex-col items-center border-none bg-transparent p-0 outline-none"
          style={{ left: time * zoom, top: 2, height: RULER_HEIGHT - 2 }}
        >
          <span
            aria-hidden
            className="block size-[9px] rounded-[2px] border border-background/60"
            style={{ background: MARKER_COLORS[marker.color] }}
          />
          <span
            aria-hidden
            className="block w-px flex-1"
            style={{ background: MARKER_COLORS[marker.color] }}
          />
          {marker.label ? (
            <span
              className="pointer-events-none absolute left-[7px] top-0 max-w-[120px] truncate text-[9px] leading-[11px] text-muted-foreground"
              aria-hidden
            >
              {marker.label}
            </span>
          ) : null}
        </button>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <div className="flex gap-1 px-2 py-1.5" data-slot="marker-colours">
          {MARKER_COLOR_ORDER.map((color: MarkerColor) => (
            <button
              key={color}
              type="button"
              aria-label={`Colour ${color}`}
              data-selected={marker.color === color || undefined}
              onClick={() => void recolorMarker(marker, color)}
              className="size-4 rounded-full border border-border data-[selected]:ring-1 data-[selected]:ring-foreground/70"
              style={{ background: MARKER_COLORS[color] }}
            />
          ))}
        </div>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={() => onRename(marker)}>
          <PencilIcon />
          Rename
        </ContextMenuItem>
        <ContextMenuItem onSelect={() => void deleteMarker(marker)}>
          <Trash2Icon />
          Delete marker
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

/**
 * The marker's rename input, resolved exactly once.
 *
 * The commit unmounts the field, which fires its blur, so without the
 * one-shot guard Enter would send the rename twice and the second — now
 * stale against the renamed document — would come back refused.
 */
function MarkerRenameField({
  marker,
  zoom,
  onDone,
}: {
  marker: Marker;
  zoom: number;
  onDone: () => void;
}) {
  const [done, setDone] = useState(false);
  const finish = (value: string | null) => {
    if (done) return;
    setDone(true);
    onDone();
    if (value !== null && value.trim() !== marker.label) void renameMarker(marker, value);
  };

  return (
    <input
      // biome-ignore lint/a11y/noAutofocus: the field exists because the user just asked to type into it
      autoFocus
      type="text"
      aria-label="Marker label"
      defaultValue={marker.label}
      data-slot="marker-rename"
      onPointerDown={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          finish((event.target as HTMLInputElement).value);
        } else if (event.key === "Escape") {
          event.preventDefault();
          finish(null);
        }
      }}
      onBlur={(event) => finish(event.target.value)}
      className="absolute z-40 h-[16px] w-[140px] rounded-[2px] border border-primary/60 bg-background/95 px-1 text-[11px] text-foreground outline-none"
      style={{ left: marker.time * zoom + 6, top: 4 }}
    />
  );
}

/**
 * All markers, plus the one rename field at a time.
 *
 * Subscribes to the document itself so the timeline body does not re-render
 * for marker edits it is not drawing.
 */
export function MarkerLane({ zoom }: { zoom: number }) {
  const project = useProjectStore((s) => s.project);
  const [renaming, setRenaming] = useState<Marker | null>(null);
  const markers = markersOf(project);

  return (
    <>
      {markers.map((marker) => (
        <MarkerFlag key={marker.id} marker={marker} zoom={zoom} onRename={setRenaming} />
      ))}
      {renaming ? (
        <MarkerRenameField
          key={renaming.id}
          marker={renaming}
          zoom={zoom}
          onDone={() => setRenaming(null)}
        />
      ) : null}
    </>
  );
}

/**
 * The in/out marks: a shaded band over the ruler between the two, and a small
 * bracket for a mark whose partner is not set yet. Never a pointer target.
 *
 * The shading only appears when `exportRange` is non-null, which is the store's
 * invariant — both marks set and in before out — so what this draws is exactly
 * what "export only this range" would take.
 */
export function ExportRangeOverlay({ zoom }: { zoom: number }) {
  const markIn = useTimelineStore((s) => s.markIn);
  const markOut = useTimelineStore((s) => s.markOut);
  const exportRange = useTimelineStore((s) => s.exportRange);

  return (
    <>
      {exportRange ? (
        <span
          aria-hidden
          data-slot="export-range"
          className="pointer-events-none absolute top-0 z-10 border-x border-primary/70 bg-primary/15"
          style={{
            left: exportRange.start * zoom,
            width: Math.max(1, (exportRange.end - exportRange.start) * zoom),
            height: RULER_HEIGHT,
          }}
        />
      ) : null}
      {markIn !== null ? (
        <span
          aria-hidden
          data-slot="mark-in"
          className="pointer-events-none absolute top-0 z-10 w-[2px] bg-primary/80"
          style={{ left: markIn * zoom, height: RULER_HEIGHT }}
        />
      ) : null}
      {markOut !== null ? (
        <span
          aria-hidden
          data-slot="mark-out"
          className="pointer-events-none absolute top-0 z-10 w-[2px] bg-primary/80"
          style={{ left: markOut * zoom, height: RULER_HEIGHT }}
        />
      ) : null}
    </>
  );
}
