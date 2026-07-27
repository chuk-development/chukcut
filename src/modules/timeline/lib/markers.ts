/**
 * Timeline markers: the command builders and the palette.
 *
 * Markers live in the document (`Project.markers`, a project-level list — see
 * `document.rs` for why it is not per-track) and every mutation goes through an
 * `EditCommand`, so adding, dragging, renaming, recolouring and deleting are
 * each one entry on the ordinary undo stack. The builders here are pure; the
 * exported `run*` wrappers send the command and fold the reply into the store.
 */

import { newId } from "@/lib/ids";
import { runEdit } from "@/modules/project/store";
import type { Marker, MarkerColor, Micros } from "@/modules/project/types";
import { type EditCommand, timelineApply } from "@/modules/timeline/lib/api";

/**
 * The fixed palette, in the order the context menu offers it. The CSS values
 * are here rather than in a stylesheet because a marker's colour is data, not
 * theme: a red marker is red in every theme, like a red label in a file
 * manager.
 */
export const MARKER_COLORS: Record<MarkerColor, string> = {
  blue: "#3b82f6",
  green: "#22c55e",
  yellow: "#eab308",
  orange: "#f97316",
  red: "#ef4444",
  purple: "#a855f7",
};

export const MARKER_COLOR_ORDER: MarkerColor[] = [
  "blue",
  "green",
  "yellow",
  "orange",
  "red",
  "purple",
];

/** A fresh marker at `at`, with the defaults M gives it. */
export function blankMarker(at: Micros, makeId: () => string = newId): Marker {
  return {
    id: makeId(),
    time: Math.max(0, Math.round(at)),
    label: "",
    color: "blue",
  };
}

export function addMarkerCommand(marker: Marker): EditCommand {
  return { type: "add_marker", marker };
}

/**
 * The edit that changes one field of a marker. The whole `before` travels so
 * Rust can refuse a stale menu — see `set_marker` in `ops.rs`.
 */
export function setMarkerCommand(before: Marker, patch: Partial<Marker>): EditCommand {
  return {
    type: "set_marker",
    before,
    after: { ...before, ...patch, id: before.id },
  };
}

export function removeMarkerCommand(marker: Marker): EditCommand {
  return { type: "remove_marker", marker };
}

export function addMarkerAt(at: Micros): Promise<boolean> {
  return runEdit(() => timelineApply(addMarkerCommand(blankMarker(at))));
}

export function moveMarker(marker: Marker, to: Micros): Promise<boolean> {
  return runEdit(() =>
    timelineApply(setMarkerCommand(marker, { time: Math.max(0, Math.round(to)) })),
  );
}

export function renameMarker(marker: Marker, label: string): Promise<boolean> {
  return runEdit(() => timelineApply(setMarkerCommand(marker, { label: label.trim() })));
}

export function recolorMarker(marker: Marker, color: MarkerColor): Promise<boolean> {
  return runEdit(() => timelineApply(setMarkerCommand(marker, { color })));
}

export function deleteMarker(marker: Marker): Promise<boolean> {
  return runEdit(() => timelineApply(removeMarkerCommand(marker)));
}
