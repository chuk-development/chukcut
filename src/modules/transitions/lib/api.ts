/**
 * Typed wrappers around the `transitions_*` commands.
 *
 * Every one of them answers with an `EditResponse` — the whole document plus
 * the history state — for the reason `timeline/lib/api.ts` gives: an
 * incremental patch is faster and produces desync bugs that are miserable to
 * chase.
 *
 * Nothing here mints an id, picks a default duration or decides how short a
 * clip shortens a transition to. That is policy, Rust owns it, and the reason
 * `transitions_add` exists as a command rather than as an `EditCommand` this
 * side could build is that policy in the webview is policy in two places.
 */

import { invoke } from "@tauri-apps/api/core";

import type { Id, Micros, TransitionKind, TransitionMaterial } from "@/modules/project/types";
import type { EditResponse } from "@/modules/timeline/lib/api";

/**
 * One entry in the picker.
 *
 * The `has_*` flags say which controls a parameter panel should show. They are
 * properties of the shader — a dissolve has no direction and never will — so
 * they come from Rust with the list rather than being a second table here.
 */
export interface TransitionDescriptor {
  kind: TransitionKind;
  label: string;
  description: string;
  directional: boolean;
  has_color: boolean;
  has_softness: boolean;
  has_zoom: boolean;
  default_duration: Micros;
}

/** Every transition the renderer implements. */
export function transitionsCatalog(): Promise<TransitionDescriptor[]> {
  return invoke<TransitionDescriptor[]>("transitions_catalog");
}

/**
 * The longest transition that may sit at the head of `segmentId`, in
 * microseconds. `0` means one may not be placed there at all.
 *
 * Asked before a drag is allowed to commit, so the gesture can be refused with
 * a cursor rather than with an error dialog.
 */
export function transitionsMaxDuration(segmentId: Id): Promise<Micros> {
  return invoke<Micros>("transitions_max_duration", { segmentId });
}

/** Put a transition at the head of `segmentId`. */
export function transitionsAdd(
  segmentId: Id,
  kind: TransitionKind,
  duration?: Micros,
): Promise<EditResponse> {
  return invoke<EditResponse>("transitions_add", {
    segmentId,
    kind,
    duration: duration ?? null,
  });
}

export function transitionsRemove(segmentId: Id): Promise<EditResponse> {
  return invoke<EditResponse>("transitions_remove", { segmentId });
}

/** What a duration drag sends when the pointer is released. */
export function transitionsRetime(segmentId: Id, duration: Micros): Promise<EditResponse> {
  return invoke<EditResponse>("transitions_retime", { segmentId, duration });
}

/** Replace the parameters wholesale, keeping the transition's identity. */
export function transitionsSet(
  segmentId: Id,
  transition: TransitionMaterial,
): Promise<EditResponse> {
  return invoke<EditResponse>("transitions_set", { segmentId, transition });
}
