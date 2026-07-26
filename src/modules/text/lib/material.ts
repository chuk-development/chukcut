/**
 * Reading a title out of the document, and the shapes the inspector needs when
 * a user turns something on.
 *
 * Nothing here talks to Rust. It is the pure half of the text UI: given a
 * document and a selected segment, is this a title, and what does turning on an
 * outline mean when the material has never had one.
 */

import type { Project, Rgba, Segment, TextMaterial, TextShadow } from "@/modules/project/types";

/** The title a segment shows, or `null` when the segment is not a title. */
export function textMaterialOf(project: Project, segment: Segment): TextMaterial | null {
  return project.materials.texts.find((m) => m.id === segment.material_id) ?? null;
}

/**
 * The outline width a user gets when they switch the outline on.
 *
 * Proportional to the font size rather than a fixed pixel count: 4 px is a
 * heavy outline on a 24 px caption and invisible on a 180 px title. The same
 * fraction Rust uses for a brand new title, restated here because turning the
 * switch back on has to land somewhere and a zero would be a no-op.
 */
export const OUTLINE_FRACTION = 0.045;

export function defaultOutlineWidth(fontSize: number): number {
  return Math.max(1, Math.round(fontSize * OUTLINE_FRACTION));
}

/**
 * The shadow a user gets when they switch the shadow on.
 *
 * Down and to the right, because that is where the light is in every editor,
 * and blurred by an eighth of the size so it reads as depth rather than as a
 * second copy of the text.
 */
export function defaultShadow(fontSize: number): TextShadow {
  const offset = Math.max(1, Math.round(fontSize / 12));
  return {
    color: [0, 0, 0, 0.55],
    offset: [offset, offset],
    blur: Math.max(1, Math.round(fontSize / 8)),
  };
}

/** The background box a user gets when they switch it on: black at 60%. */
export const DEFAULT_BACKGROUND: Rgba = [0, 0, 0, 0.6];

/**
 * Titles ready to drop on the timeline.
 *
 * Only the words differ — everything else about a title is the material's own
 * defaults, which Rust owns. A preset that also set colours and sizes would be
 * a second copy of that policy in the webview.
 */
export interface TitlePreset {
  id: string;
  label: string;
  content: string;
}

export const TITLE_PRESETS: TitlePreset[] = [
  { id: "title", label: "Title", content: "Your title here" },
  { id: "subtitle", label: "Two lines", content: "Your title here\nand a second line" },
  { id: "hook", label: "Hook", content: "WAIT FOR IT…" },
  { id: "cta", label: "Call to action", content: "FOLLOW FOR MORE" },
];
