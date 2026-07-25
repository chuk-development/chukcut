/**
 * Real window fullscreen, not a CSS expansion.
 *
 * The point of the feature is judging the picture, and a `position: fixed` div
 * inside a 1280px window shows the same number of pixels it always did. Only
 * the window manager can give the frame the whole panel, so this goes through
 * `setFullscreen` and the UI follows the window rather than the other way
 * about.
 *
 * ## Permission
 *
 * `core:default` grants `core:window:allow-is-fullscreen` (reading) but **not**
 * `core:window:allow-set-fullscreen` (setting). Without the latter in
 * `src-tauri/capabilities/default.json` every call here is rejected, which is
 * why the store is only updated once the window has agreed: a UI that goes
 * fullscreen while the window did not is a black overlay the user cannot
 * explain.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";

import { usePreviewStore } from "@/modules/preview/store";
import { describeError } from "@/modules/project/store";

/** Returns whether the window actually changed. */
export async function setFullscreen(wanted: boolean): Promise<boolean> {
  try {
    await getCurrentWindow().setFullscreen(wanted);
  } catch (error) {
    usePreviewStore.getState().setError(describeError(error));
    return false;
  }
  usePreviewStore.getState().setFullscreen(wanted);
  usePreviewStore.getState().setError(null);
  return true;
}

export function toggleFullscreen(): Promise<boolean> {
  return setFullscreen(!usePreviewStore.getState().fullscreen);
}

/**
 * Follow the window when something else changes it.
 *
 * F11, a tiling window manager or the compositor can drop the window out of
 * fullscreen without going through us, and an overlay left covering an ordinary
 * window is the kind of stuck state a user reaches for the task manager over.
 * Resize is the only signal Tauri offers, so the flag is re-read on every one.
 */
export function watchWindowFullscreen(): () => void {
  let stop: (() => void) | null = null;
  let cancelled = false;

  void (async () => {
    try {
      const current = getCurrentWindow();
      const off = await current.onResized(async () => {
        try {
          usePreviewStore.getState().setFullscreen(await current.isFullscreen());
        } catch {
          // Reading it back is a nicety. Losing it is not worth a message.
        }
      });
      if (cancelled) off();
      else stop = off;
    } catch {
      // No webview metadata: `getCurrentWindow()` throws, fullscreen is simply
      // unavailable, and the player must survive that — see App.test.tsx.
    }
  })();

  return () => {
    cancelled = true;
    stop?.();
  };
}
