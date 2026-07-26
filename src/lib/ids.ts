/**
 * Fresh document ids.
 *
 * In `lib/` rather than in the timeline's own folder because three features now
 * mint ids — a drop, a paste and a link group — and the alternative was one of
 * them importing another for a nine-line function, which is how an import cycle
 * starts.
 */

import type { Id } from "@/modules/project/types";

/** UUID v4. `crypto.randomUUID` is missing on some webview origins, so fall back. */
export function newId(): Id {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (char) => {
    const random = (Math.random() * 16) | 0;
    const value = char === "x" ? random : (random & 0x3) | 0x8;
    return value.toString(16);
  });
}
