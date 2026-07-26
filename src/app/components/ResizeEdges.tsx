/**
 * Eight invisible grips around the window, because an undecorated one has none.
 *
 * This is the part of `decorations: false` that is easy to forget until the
 * window cannot be made bigger. On GTK the resize border *is* the decoration —
 * take the frame away and the window manager has nothing left to hit-test, so
 * `startResizeDragging` from inside the webview is the only way back. Wayland
 * and X11 both honour it; it is the same call a GTK client-side-decorated app
 * makes.
 *
 * Four pixels for an edge and twelve for a corner, which is roughly what a GTK
 * frame offers. They sit above the whole app, dialogs included, so a window can
 * always be resized — a modal that pins the window to whatever size it happened
 * to be is worse than a grip overlapping its border.
 *
 * These are chrome, not controls: there is nothing here for a keyboard or a
 * screen reader to do, which is why they are `aria-hidden` and take no focus.
 * Resizing from the keyboard is the window manager's, and it still works.
 */

import { type ResizeEdge, startResize } from "@/modules/workspace/lib/window";

/** Edges first, so the corners are painted over them and win the hit test. */
const GRIPS: { edge: ResizeEdge; className: string }[] = [
  { edge: "North", className: "inset-x-0 top-0 h-1 cursor-ns-resize" },
  { edge: "South", className: "inset-x-0 bottom-0 h-1 cursor-ns-resize" },
  { edge: "West", className: "inset-y-0 left-0 w-1 cursor-ew-resize" },
  { edge: "East", className: "inset-y-0 right-0 w-1 cursor-ew-resize" },
  { edge: "NorthWest", className: "left-0 top-0 size-3 cursor-nwse-resize" },
  { edge: "NorthEast", className: "right-0 top-0 size-3 cursor-nesw-resize" },
  { edge: "SouthWest", className: "bottom-0 left-0 size-3 cursor-nesw-resize" },
  { edge: "SouthEast", className: "bottom-0 right-0 size-3 cursor-nwse-resize" },
];

export function ResizeEdges() {
  return (
    <>
      {GRIPS.map(({ edge, className }) => (
        <div
          key={edge}
          aria-hidden
          data-slot="resize-edge"
          data-edge={edge}
          // Pointer*down*, not click: a resize is a drag, and by the time a
          // click has completed the gesture it was meant to start is over.
          onPointerDown={(event) => {
            // Left button only. A right-click on the frame is the window
            // manager's menu, and a middle one is nothing.
            if (event.button !== 0) return;
            event.preventDefault();
            void startResize(edge);
          }}
          className={`fixed z-100 ${className}`}
        />
      ))}
    </>
  );
}
