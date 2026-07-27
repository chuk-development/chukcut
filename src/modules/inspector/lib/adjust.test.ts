/**
 * The two translations `adjust.ts` owns — panel insets to the document's kept
 * rectangle, and quarter-turn wrapping — plus the grade resolution. All pure;
 * the IPC side is exercised through the Inspector component tests.
 */

import { describe, expect, it } from "vitest";

import {
  colorAdjustOf,
  cropFromInsets,
  insetsOf,
  MAX_INSET,
  rotateBy,
} from "@/modules/inspector/lib/adjust";
import { makeSegment, projectWithSegments } from "@/test/fixtures";

describe("crop insets", () => {
  it("round-trips through the document's kept-rectangle form", () => {
    const insets = { left: 0.1, top: 0.2, right: 0.3, bottom: 0.05 };
    const crop = cropFromInsets(insets);
    expect(crop).toEqual({ left: 0.1, top: 0.2, right: 0.7, bottom: 0.95 });
    const back = insetsOf(crop);
    // Close, not equal: `1 - (1 - 0.3)` is `0.30000000000000004`, and the
    // document round-trip is allowed exactly that much.
    for (const key of ["left", "top", "right", "bottom"] as const) {
      expect(back[key]).toBeCloseTo(insets[key], 10);
    }
  });

  it("spells uncropped as null, in both directions", () => {
    expect(cropFromInsets({ left: 0, top: 0, right: 0, bottom: 0 })).toBeNull();
    expect(insetsOf(null)).toEqual({ left: 0, top: 0, right: 0, bottom: 0 });
  });

  it("caps each edge so no slider position can empty the picture", () => {
    const crop = cropFromInsets({ left: 0.9, top: 0, right: 0.9, bottom: 0 });
    expect(crop).not.toBeNull();
    expect(crop?.left).toBe(MAX_INSET);
    expect(crop?.right).toBe(1 - MAX_INSET);
    // Two opposing edges at the cap still keep a slice.
    expect((crop?.right ?? 0) - (crop?.left ?? 0)).toBeGreaterThan(0);
  });
});

describe("rotateBy", () => {
  it("steps by quarter turns and wraps into the slider's range", () => {
    expect(rotateBy(0, 90)).toBe(90);
    expect(rotateBy(90, 90)).toBe(-180);
    expect(rotateBy(-180, 90)).toBe(-90);
    expect(rotateBy(135, 90)).toBe(-135);
    expect(rotateBy(0, -90)).toBe(-90);
    expect(rotateBy(-135, -90)).toBe(135);
  });

  it("returns to exactly zero after four turns either way", () => {
    let angle = 0;
    for (let i = 0; i < 4; i++) angle = rotateBy(angle, 90);
    expect(angle).toBe(0);
    for (let i = 0; i < 4; i++) angle = rotateBy(angle, -90);
    expect(angle).toBe(0);
  });
});

describe("colorAdjustOf", () => {
  it("resolves the grade through the segment's extras, ignoring foreign ids", () => {
    const segment = makeSegment("segment-1", { extras: ["not-a-grade", "grade-1"] });
    const project = projectWithSegments(segment);
    project.materials.color_adjusts = [
      { id: "grade-1", brightness: 0.2, contrast: 1, saturation: 1, temperature: 0, lut: null },
    ];

    expect(colorAdjustOf(project, segment)?.brightness).toBe(0.2);
  });

  it("answers null for an ungraded clip and for a pool the backend has not sent", () => {
    const segment = makeSegment("segment-1");
    const project = projectWithSegments(segment);
    expect(colorAdjustOf(project, segment)).toBeNull();
    project.materials.color_adjusts = undefined;
    expect(colorAdjustOf(project, segment)).toBeNull();
  });
});
