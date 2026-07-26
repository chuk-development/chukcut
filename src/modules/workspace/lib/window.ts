/**
 * The window itself — minimise, maximise, close, drag, resize.
 *
 * None of this existed while the window had decorations: the desktop drew the
 * title bar and did all five. `decorations: false` in `tauri.conf.json` bought
 * us a menu bar we can theme and left us owing everything that bar used to come
 * with, so this file is the debt, in one place, wrapped the way every other
 * `lib/` folder wraps its commands — components never reach for
 * `@tauri-apps/api` directly.
 *
 * ## Permissions
 *
 * Every call here needs a permission `core:default` does *not* grant, because a
 * decorated window never asks for them. They are listed in
 * `src-tauri/capabilities/default.json`; without one the call is rejected at the
 * boundary and the button silently does nothing, which is why each of these
 * returns whether it worked rather than swallowing the error.
 *
 * ## Failure
 *
 * Every function is total: nothing throws. `getCurrentWindow()` throws outright
 * when there is no webview metadata — a browser tab, or a test that did not ask
 * for it — and the editor has to survive that (see `App.test.tsx`, the
 * white-screen regression). A window control that cannot reach the window is a
 * dead button, not a dead app.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";

/** Which edge or corner a resize grip drives. Matches Tauri's `ResizeDirection`. */
export type ResizeEdge =
  | "North"
  | "South"
  | "East"
  | "West"
  | "NorthEast"
  | "NorthWest"
  | "SouthEast"
  | "SouthWest";

async function onWindow<T>(act: (window: WindowHandle) => Promise<T>): Promise<T | null> {
  try {
    return await act(getCurrentWindow());
  } catch {
    return null;
  }
}

/** Only the parts of Tauri's `Window` this file uses. */
type WindowHandle = Pick<
  ReturnType<typeof getCurrentWindow>,
  "minimize" | "toggleMaximize" | "close" | "isMaximized" | "startDragging" | "startResizeDragging"
>;

/** Returns whether the window actually went down. */
export async function minimizeWindow(): Promise<boolean> {
  return (await onWindow(async (w) => w.minimize())) !== null;
}

/**
 * Maximise, or come back out of it.
 *
 * The same call either way, so the button never has to know which state it is
 * in — only which icon to draw, which is what `isWindowMaximized` is for.
 */
export async function toggleMaximizeWindow(): Promise<boolean> {
  return (await onWindow(async (w) => w.toggleMaximize())) !== null;
}

/**
 * Ask the window to close.
 *
 * This is a request, not an ending: Rust prevents the close and raises
 * `menu://close-requested`, the webview runs its unsaved-changes guard, and
 * `workspace_close_answer` decides. Identical to File → Quit on purpose — they
 * are the same action and must be guarded the same way.
 */
export async function closeWindow(): Promise<boolean> {
  return (await onWindow(async (w) => w.close())) !== null;
}

/** `null` when the window cannot be reached at all. */
export async function isWindowMaximized(): Promise<boolean | null> {
  return onWindow((w) => w.isMaximized());
}

/**
 * Start a resize from an edge or a corner.
 *
 * An undecorated GTK window has no resize grips of its own — that frame *was*
 * the decoration — so the eight grips in `ResizeEdges.tsx` are the only way to
 * resize this window with a pointer, and this is what they call.
 */
export async function startResize(edge: ResizeEdge): Promise<boolean> {
  return (await onWindow(async (w) => w.startResizeDragging(edge))) !== null;
}
