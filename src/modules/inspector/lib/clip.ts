/**
 * Clip-level inspector commands: paste attributes and rename.
 *
 * Both are built in Rust against the real document — see
 * `src-tauri/src/modules/inspector/edit.rs` — and both land on the ordinary
 * undo stack as one step. This file is the typed `invoke()` surface plus the
 * one piece of assembly the webview owns: turning a clipboard entry into the
 * attribute payload.
 */

import { invoke } from "@tauri-apps/api/core";

import { runEdit } from "@/modules/project/store";
import type { ColorAdjustMaterial, Crop, Id, Transform } from "@/modules/project/types";
import type { EditResponse } from "@/modules/timeline/lib/api";
import type { ClipboardEntry } from "@/modules/timeline/lib/clipboard";

/**
 * What "Paste attributes" carries: the copied clip's look, none of its timing
 * or identity. Mirrors `ClipAttributes` in `inspector/edit.rs`; the grade goes
 * as values because the clipboard outlives the document it copied from and an
 * id would resolve to nothing there — Rust mints one shared immutable material
 * from these values per paste.
 */
export interface ClipAttributes {
  transform: Transform;
  speed: number;
  volume: number;
  crop: Crop | null;
  color: ColorAdjustColor | null;
}

/** The grade's values, in the shape Rust's `ColorEdit` deserialises. */
export interface ColorAdjustColor {
  brightness: number;
  contrast: number;
  saturation: number;
  temperature: number;
  lut: ColorAdjustMaterial["lut"];
}

/** The attributes a clipboard entry would paste. */
export function attributesOf(entry: ClipboardEntry): ClipAttributes {
  const grade = entry.colorAdjust ?? null;
  return {
    transform: { ...entry.segment.transform },
    speed: entry.segment.speed,
    volume: entry.segment.volume,
    crop: entry.segment.crop ? { ...entry.segment.crop } : null,
    color: grade
      ? {
          brightness: grade.brightness,
          contrast: grade.contrast,
          saturation: grade.saturation,
          temperature: grade.temperature,
          lut: grade.lut ? { ...grade.lut } : null,
        }
      : null,
  };
}

function inspectorPasteAttributes(
  attributes: ClipAttributes,
  segmentIds: Id[],
): Promise<EditResponse> {
  return invoke<EditResponse>("inspector_paste_attributes", { attributes, segmentIds });
}

/** Apply a copied clip's attributes to `segmentIds`, as one undo step. */
export function pasteAttributes(entry: ClipboardEntry, segmentIds: Id[]): Promise<boolean> {
  return runEdit(() => inspectorPasteAttributes(attributesOf(entry), segmentIds));
}

function inspectorRenameClip(segmentId: Id, name: string | null): Promise<EditResponse> {
  return invoke<EditResponse>("inspector_rename_clip", { segmentId, name });
}

/** Name a clip, or clear its name with null / whitespace. One undo step. */
export function renameClip(segmentId: Id, name: string | null): Promise<boolean> {
  return runEdit(() => inspectorRenameClip(segmentId, name));
}
