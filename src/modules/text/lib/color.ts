/**
 * Colours, between the document and an `<input type="color">`.
 *
 * A `TextMaterial`'s colours are straight (non-premultiplied) **sRGB** in
 * `0..1`, not linear light: `text/raster.rs` blends on sRGB bytes and
 * multiplies each channel by 255 with no transfer function, deliberately, so
 * that glyph edges do not look thin and so that what the user picked is what
 * they get. That is a *different* answer from the one the compositor uses for
 * video, and it is why this conversion is a multiply rather than a curve.
 *
 * The canvas background in `CanvasConfig` is linear and must not be run through
 * these functions.
 */

import type { Rgba } from "@/modules/project/types";

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value));
}

/** `#rrggbb`, which is the only form `<input type="color">` accepts or emits. */
export function rgbaToHex(color: Rgba): string {
  const channel = (value: number) =>
    Math.round(clamp01(value) * 255)
      .toString(16)
      .padStart(2, "0");
  return `#${channel(color[0])}${channel(color[1])}${channel(color[2])}`;
}

/**
 * `#rrggbb` back to the document's form, keeping the alpha that was already
 * there — the swatch has no opinion about transparency and must not reset it.
 */
export function hexToRgba(hex: string, alpha = 1): Rgba {
  const parsed = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!parsed) return [0, 0, 0, clamp01(alpha)];
  const value = Number.parseInt(parsed[1], 16);
  return [
    ((value >> 16) & 0xff) / 255,
    ((value >> 8) & 0xff) / 255,
    (value & 0xff) / 255,
    clamp01(alpha),
  ];
}

/** The alpha channel as a percentage, for a slider. */
export function alphaOf(color: Rgba): number {
  return Math.round(clamp01(color[3]) * 100);
}

/** Replace only the alpha, leaving the colour alone. */
export function withAlpha(color: Rgba, alpha: number): Rgba {
  return [color[0], color[1], color[2], clamp01(alpha)];
}

/** A CSS colour for a preview swatch, alpha included. */
export function rgbaToCss(color: Rgba): string {
  const channel = (value: number) => Math.round(clamp01(value) * 255);
  return `rgba(${channel(color[0])}, ${channel(color[1])}, ${channel(color[2])}, ${clamp01(
    color[3],
  )})`;
}
