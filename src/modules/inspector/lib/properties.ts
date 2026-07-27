/**
 * What the inspector can animate, in one table.
 *
 * A row is not a property: the scale control is a single slider that drives
 * both `scale_x` and `scale_y`, because a video editor that makes you keyframe
 * two axes to zoom is a video editor nobody uses. So a row names the tracks it
 * writes — usually one, two for scale — and every keyframe operation maps over
 * them and comes back as one command.
 *
 * `staticCommand` is what the row sends when the property is *not* animated:
 * the existing `set_transform` / `set_volume` edits, unchanged. Animation does
 * not replace the static value, it overrides it while it lasts (see
 * `animated_transform` in `src-tauri/src/modules/render/layout.rs`), so the
 * static path has to stay exactly as it was.
 */

import type { AnimatableProperty, Segment } from "@/modules/project/types";
import type { EditCommand } from "@/modules/timeline/lib/api";

export interface PropertyDef {
  /** Row id. Stable, and what the curve view's selection is keyed by. */
  id: string;
  label: string;
  /** The keyframe tracks this row drives. The first is the one it reads back. */
  properties: AnimatableProperty[];
  min: number;
  max: number;
  step: number;
  format?: (value: number) => string;
  /** The document value, ignoring animation. */
  read: (segment: Segment) => number;
  /** The edit for a value change while the property is not animated. */
  staticCommand: (segment: Segment, value: number) => EditCommand;
}

const percent = (value: number) => `${Math.round(value * 100)}%`;

function setTransform(segment: Segment, patch: Partial<Segment["transform"]>): EditCommand {
  return {
    type: "set_transform",
    segment_id: segment.id,
    before: segment.transform,
    after: { ...segment.transform, ...patch },
  };
}

export const POSITION_X: PropertyDef = {
  id: "position_x",
  label: "Position X",
  properties: ["position_x"],
  min: -2,
  max: 2,
  step: 0.01,
  read: (segment) => segment.transform.position[0],
  staticCommand: (segment, value) =>
    setTransform(segment, { position: [value, segment.transform.position[1]] }),
};

export const POSITION_Y: PropertyDef = {
  id: "position_y",
  label: "Position Y",
  properties: ["position_y"],
  min: -2,
  max: 2,
  step: 0.01,
  read: (segment) => segment.transform.position[1],
  staticCommand: (segment, value) =>
    setTransform(segment, { position: [segment.transform.position[0], value] }),
};

export const SCALE: PropertyDef = {
  id: "scale",
  label: "Scale",
  properties: ["scale_x", "scale_y"],
  min: 0.05,
  max: 4,
  step: 0.01,
  format: percent,
  read: (segment) => segment.transform.scale[0],
  staticCommand: (segment, value) => setTransform(segment, { scale: [value, value] }),
};

export const ROTATION: PropertyDef = {
  id: "rotation",
  label: "Rotation",
  properties: ["rotation"],
  min: -180,
  max: 180,
  step: 1,
  format: (value) => `${Math.round(value)}°`,
  read: (segment) => segment.transform.rotation,
  staticCommand: (segment, value) => setTransform(segment, { rotation: value }),
};

export const OPACITY: PropertyDef = {
  id: "opacity",
  label: "Opacity",
  properties: ["opacity"],
  min: 0,
  max: 1,
  step: 0.01,
  format: percent,
  read: (segment) => segment.transform.opacity,
  staticCommand: (segment, value) => setTransform(segment, { opacity: value }),
};

export const VOLUME: PropertyDef = {
  id: "volume",
  label: "Volume",
  properties: ["volume"],
  min: 0,
  max: 2,
  step: 0.01,
  format: percent,
  read: (segment) => segment.volume,
  staticCommand: (segment, value) => ({
    type: "set_volume",
    segment_id: segment.id,
    before: segment.volume,
    after: value,
  }),
};

/**
 * The rows the Transform section shows. Opacity is deliberately not here: it
 * renders in the Colour section with the grade sliders, where an editor's
 * users look for it — while staying a `Transform` field in the document and a
 * keyframable row like any other.
 */
export const TRANSFORM_PROPERTIES: PropertyDef[] = [POSITION_X, POSITION_Y, SCALE, ROTATION];

/** Every row that can be keyframed, in the order the curve view lists them. */
export const ANIMATABLE_PROPERTIES: PropertyDef[] = [...TRANSFORM_PROPERTIES, OPACITY, VOLUME];

export function propertyById(id: string): PropertyDef | null {
  return ANIMATABLE_PROPERTIES.find((def) => def.id === id) ?? null;
}

/** Snap a value onto the row's step, so a drag does not produce 0.6100000000000001. */
export function roundToStep(value: number, step: number): number {
  if (step <= 0) return value;
  const decimals = Math.max(0, Math.ceil(-Math.log10(step)));
  return Number((Math.round(value / step) * step).toFixed(decimals));
}
