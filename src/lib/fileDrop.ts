/**
 * Files dragged in from the desktop.
 *
 * This cannot be done with HTML5 drag-and-drop: Tauri intercepts OS file drops
 * at the window level, and the webview's `drop` event never carries a usable
 * path. The native event does, and it is the only way the frontend learns a
 * path it did not get from a dialog.
 *
 * Positions arrive in physical pixels; everything in the UI works in CSS
 * pixels, so they are converted here rather than at each call site.
 */

import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export interface FileDropEvent {
  paths: string[];
  clientX: number;
  clientY: number;
}

export interface FileDropHandlers {
  onEnter?: (position: { clientX: number; clientY: number }) => void;
  onLeave?: () => void;
  onDrop: (event: FileDropEvent) => void;
}

function toCssPixels(position: { x: number; y: number }): {
  clientX: number;
  clientY: number;
} {
  const ratio = window.devicePixelRatio || 1;
  return { clientX: position.x / ratio, clientY: position.y / ratio };
}

/**
 * Subscribe to desktop file drops. Resolves to an unlisten function; callers in
 * effects should call it on cleanup, and guard against the effect having been
 * torn down before the subscription resolved.
 *
 * Deliberately `async`: `getCurrentWebview()` throws synchronously when the
 * webview metadata is not there, and a subscription failing is not a reason for
 * the editor to go blank. As a promise it is a rejection the caller can shrug
 * off, not an exception thrown during render.
 */
export async function listenForFileDrop(handlers: FileDropHandlers): Promise<UnlistenFn> {
  return getCurrentWebview().onDragDropEvent((event) => {
    const payload = event.payload;
    if (payload.type === "over") {
      handlers.onEnter?.(toCssPixels(payload.position));
      return;
    }
    if (payload.type === "drop") {
      const { clientX, clientY } = toCssPixels(payload.position);
      handlers.onDrop({ paths: payload.paths, clientX, clientY });
      handlers.onLeave?.();
      return;
    }
    handlers.onLeave?.();
  });
}
