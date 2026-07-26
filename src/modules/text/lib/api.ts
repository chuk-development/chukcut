/**
 * Typed wrappers around the `text_*` commands.
 *
 * ## Casing
 *
 * Command *arguments* are camelCase, because Tauri maps them onto Rust's
 * snake_case parameters itself — the same convention `timeline/lib/api.ts`
 * follows. The *payloads* stay snake_case, because they are the document's own
 * types and serde writes them that way.
 *
 * ## Why adding a title is one command and not three
 *
 * A title is a material plus a segment plus, usually, a lane to put it on.
 * Doing that from here would mean the webview minting a uuid, knowing how long
 * a title is, knowing that it belongs above the pictures, and knowing what to
 * do when the playhead is over an existing title. That is four pieces of
 * policy, and Rust already has all four.
 */

import { invoke } from "@tauri-apps/api/core";

import type { Id, Micros, TextMaterial } from "@/modules/project/types";
import type { EditResponse } from "@/modules/timeline/lib/api";

/**
 * What `text_add` answers with: an ordinary `EditResponse` — the fields are
 * flattened into it on the Rust side — plus enough identity to select what was
 * just created.
 */
export interface TextAdded extends EditResponse {
  material_id: Id;
  segment_id: Id;
  track_id: Id;
  /** Where it actually landed, which is not `at` when that instant was taken. */
  start: Micros;
}

/**
 * Every font family this machine can draw, sorted.
 *
 * From the same collection the rasteriser resolves against, so a name offered
 * here is a name that will render. Worth caching in a store: the answer cannot
 * change while the app runs.
 */
export function textFonts(): Promise<string[]> {
  return invoke<string[]>("text_fonts");
}

/**
 * Put a new title on the timeline at `at`.
 *
 * `content` and `duration` are optional; omitting them takes Rust's defaults,
 * which is what the "Text" button does.
 */
export function textAdd(at: Micros, content?: string, duration?: Micros): Promise<TextAdded> {
  return invoke<TextAdded>("text_add", {
    at: Math.max(0, Math.round(at)),
    content: content ?? null,
    duration: duration ?? null,
  });
}

/**
 * Replace a title's parameters wholesale, keeping its id.
 *
 * Not undoable — there is no `EditCommand` that carries a `TextMaterial` yet,
 * and the reasoning is in `src-tauri/src/modules/text/commands.rs`. The
 * response still carries the document, so the preview and the timeline label
 * follow the change.
 */
export function textSet(material: TextMaterial): Promise<EditResponse> {
  return invoke<EditResponse>("text_set", { material });
}
