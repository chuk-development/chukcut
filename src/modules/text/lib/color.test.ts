/**
 * Colour conversion, pinned to exact values.
 *
 * The reason this is worth a test rather than being obvious: the document's
 * text colours are **sRGB** in 0..1 and the canvas background is **linear**, so
 * a "fix" that made this apply a transfer function would be a plausible-looking
 * change that silently darkened every title. The round trips below fail the
 * moment anything but a multiply by 255 happens here.
 */

import { describe, expect, it } from "vitest";

import type { Rgba } from "@/modules/project/types";
import { alphaOf, hexToRgba, rgbaToCss, rgbaToHex, withAlpha } from "@/modules/text/lib/color";

describe("rgbaToHex", () => {
  it("is a straight multiply by 255, not a transfer function", () => {
    // sRGB 0.5 is 128; a linear→sRGB conversion would give 188.
    expect(rgbaToHex([0.5, 0.5, 0.5, 1])).toBe("#808080");
    expect(rgbaToHex([1, 1, 1, 1])).toBe("#ffffff");
    expect(rgbaToHex([0, 0, 0, 1])).toBe("#000000");
  });

  it("pads every channel to two digits", () => {
    expect(rgbaToHex([1 / 255, 0, 0, 1])).toBe("#010000");
  });

  it("clamps rather than emitting something a colour input rejects", () => {
    expect(rgbaToHex([2, -1, 0.5, 1])).toBe("#ff0080");
    expect(rgbaToHex([Number.NaN, 0, 0, 1])).toBe("#000000");
  });
});

describe("hexToRgba", () => {
  it("round-trips every byte exactly", () => {
    for (const hex of ["#000000", "#ffffff", "#123456", "#abcdef", "#808080"]) {
      expect(rgbaToHex(hexToRgba(hex))).toBe(hex);
    }
  });

  it("keeps the alpha it was given, because the swatch has no opinion about it", () => {
    expect(hexToRgba("#ffffff", 0.35)).toEqual([1, 1, 1, 0.35]);
  });

  it("survives junk instead of putting a NaN in the document", () => {
    const black: Rgba = [0, 0, 0, 1];
    expect(hexToRgba("not a colour")).toEqual(black);
    expect(hexToRgba("#fff")).toEqual(black);
    expect(hexToRgba("#ffffff", Number.NaN)).toEqual([1, 1, 1, 0]);
  });

  it("accepts what an <input type=color> emits, with or without the hash", () => {
    expect(hexToRgba("ff0000")).toEqual([1, 0, 0, 1]);
    expect(hexToRgba("  #FF0000  ")).toEqual([1, 0, 0, 1]);
  });
});

describe("alpha", () => {
  it("reads and writes only the fourth channel", () => {
    const color: Rgba = [0.2, 0.4, 0.6, 0.8];
    expect(alphaOf(color)).toBe(80);
    expect(withAlpha(color, 0.25)).toEqual([0.2, 0.4, 0.6, 0.25]);
    expect(withAlpha(color, 5)).toEqual([0.2, 0.4, 0.6, 1]);
  });
});

describe("rgbaToCss", () => {
  it("carries the alpha the hex form cannot", () => {
    expect(rgbaToCss([0, 0, 0, 0.55])).toBe("rgba(0, 0, 0, 0.55)");
    expect(rgbaToCss([1, 0.5, 0, 1])).toBe("rgba(255, 128, 0, 1)");
  });
});
