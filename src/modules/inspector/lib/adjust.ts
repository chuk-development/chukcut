/**
 * Crop and colour: the inspector gestures with a Rust command of their own.
 *
 * Unlike the transform sliders, which send `set_transform` through
 * `timeline_apply`, crop and colour have no `EditCommand` variant. Rust builds
 * each edit as a composite of existing primitives *from the live document*
 * (`src-tauri/src/modules/inspector/edit.rs` says how and why), so this side
 * sends intent — a segment id and the values — never a command it composed
 * itself from a possibly stale snapshot.
 */

import { invoke } from "@tauri-apps/api/core";

import { runEdit } from "@/modules/project/store";
import type {
  ColorAdjustMaterial,
  Crop,
  Id,
  LutRef,
  Project,
  Segment,
} from "@/modules/project/types";
import type { EditResponse } from "@/modules/timeline/lib/api";

/** The values `inspector_set_color` takes: a grade without an identity. */
export interface ColorEdit {
  brightness: number;
  contrast: number;
  saturation: number;
  temperature: number;
  lut: LutRef | null;
}

/** What every slider shows when the clip has no grade. */
export const IDENTITY_COLOR: ColorEdit = {
  brightness: 0,
  contrast: 1,
  saturation: 1,
  temperature: 0,
  lut: null,
};

/** What `inspector_lut_probe` answers for a readable, well-formed .cube. */
export interface LutInfo {
  title: string | null;
  size: number;
}

/**
 * Read and validate a .cube file without touching the document. Rejects with
 * the parser's line-numbered message, which is what the picker shows for a
 * malformed file.
 */
export function inspectorLutProbe(path: string): Promise<LutInfo> {
  return invoke<LutInfo>("inspector_lut_probe", { path });
}

export function inspectorSetCrop(segmentId: Id, crop: Crop | null): Promise<EditResponse> {
  return invoke<EditResponse>("inspector_set_crop", { segmentId, crop });
}

export function inspectorSetColor(segmentId: Id, color: ColorEdit | null): Promise<EditResponse> {
  return invoke<EditResponse>("inspector_set_color", { segmentId, color });
}

export function runSetCrop(segmentId: Id, crop: Crop | null): Promise<boolean> {
  return runEdit(() => inspectorSetCrop(segmentId, crop));
}

export function runSetColor(segmentId: Id, color: ColorEdit | null): Promise<boolean> {
  return runEdit(() => inspectorSetColor(segmentId, color));
}

/**
 * The colour adjustment applied to `segment`, if any.
 *
 * The mirror of `MaterialPool::color_adjust_of` in Rust: `extras` carries no
 * type tag, so an id is a grade when the pool's colour category resolves it.
 */
export function colorAdjustOf(project: Project, segment: Segment): ColorAdjustMaterial | null {
  const pool = project.materials.color_adjusts ?? [];
  for (const id of segment.extras) {
    const hit = pool.find((m) => m.id === id);
    if (hit) return hit;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Crop, as the panel speaks it
//
// The document stores a crop as the *kept* rectangle — `right: 1` means
// nothing is cut from the right — while a person cropping thinks in how much
// is taken *off each edge*. The panel therefore works in insets and these two
// functions translate at the boundary, so neither convention leaks into the
// other's side.
// ---------------------------------------------------------------------------

/** Fraction removed from each edge, all `0` when uncropped. */
export interface CropInsets {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * The most a single edge may take. Two opposing edges at the cap still leave
 * 10% of the picture, so no slider position can produce the empty crop Rust
 * refuses.
 */
export const MAX_INSET = 0.45;

export function insetsOf(crop: Crop | null): CropInsets {
  if (!crop) return { left: 0, top: 0, right: 0, bottom: 0 };
  return {
    left: crop.left,
    top: crop.top,
    right: 1 - crop.right,
    bottom: 1 - crop.bottom,
  };
}

/**
 * Insets back to the document's kept-rectangle, `null` when nothing is cut —
 * "uncropped" has exactly one spelling in the document, and it is the absent
 * one.
 */
export function cropFromInsets(insets: CropInsets): Crop | null {
  const clamp = (v: number) => Math.min(Math.max(v, 0), MAX_INSET);
  const left = clamp(insets.left);
  const top = clamp(insets.top);
  const right = clamp(insets.right);
  const bottom = clamp(insets.bottom);
  if (left === 0 && top === 0 && right === 0 && bottom === 0) return null;
  return { left, top, right: 1 - right, bottom: 1 - bottom };
}

// ---------------------------------------------------------------------------
// Rotation steps
// ---------------------------------------------------------------------------

/**
 * `current` turned by `degrees`, wrapped into the slider's `-180..180` range.
 *
 * Wrapped rather than accumulated so four quarter turns land back on exactly
 * `0` and the slider thumb never leaves its track. `180` and `-180` are the
 * same orientation; the wrap picks `-180..180)` half-open, matching what the
 * slider can express.
 */
export function rotateBy(current: number, degrees: number): number {
  return ((((current + degrees) % 360) + 540) % 360) - 180;
}
