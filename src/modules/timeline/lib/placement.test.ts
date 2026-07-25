import { describe, expect, it } from "vitest";

import { MICROS_PER_SECOND } from "@/lib/time";
import type { Micros } from "@/modules/project/types";
import {
  freeSpan,
  isRangeFree,
  nearestFreeStart,
  type Placed,
} from "@/modules/timeline/lib/placement";

const SEC = MICROS_PER_SECOND;

function at(id: string, start: Micros, duration: Micros): Placed {
  return { id, target_range: { start, duration } };
}

describe("isRangeFree", () => {
  const lane = [at("a", 0, 4 * SEC), at("b", 6 * SEC, 4 * SEC)];

  it("rejects an overlap", () => {
    expect(isRangeFree(lane, { start: 3 * SEC, duration: 2 * SEC }, null)).toBe(false);
  });

  it("accepts a range that only touches, since ranges are half-open", () => {
    expect(isRangeFree(lane, { start: 4 * SEC, duration: 2 * SEC }, null)).toBe(true);
  });

  it("ignores the clip being moved", () => {
    // Its own range no longer blocks it, but the next clip along still does.
    expect(isRangeFree(lane, { start: 0, duration: 6 * SEC }, "a")).toBe(true);
    expect(isRangeFree(lane, { start: 0, duration: 7 * SEC }, "a")).toBe(false);
  });
});

describe("nearestFreeStart", () => {
  it("leaves a position that is already free alone", () => {
    const lane = [at("a", 0, 4 * SEC)];
    expect(nearestFreeStart(lane, 5 * SEC, 2 * SEC, null)).toBe(5 * SEC);
  });

  it("pushes a clip dropped on a neighbour's tail past its trailing edge", () => {
    const lane = [at("a", 0, 4 * SEC)];
    expect(nearestFreeStart(lane, 3 * SEC, 2 * SEC, null)).toBe(4 * SEC);
  });

  it("pushes a clip dropped on a neighbour's head against its leading edge", () => {
    const lane = [at("a", 10 * SEC, 4 * SEC)];
    const start = nearestFreeStart(lane, 9_500_000, 2 * SEC, null);
    expect(start).toBe(8 * SEC);
    // Butted up against the neighbour, with nothing left over.
    expect(start + 2 * SEC).toBe(10 * SEC);
  });

  it("drops into a gap that fits exactly, touching on both sides", () => {
    const lane = [at("a", 0, 4 * SEC), at("b", 6 * SEC, 4 * SEC)];
    const start = nearestFreeStart(lane, 4_500_000, 2 * SEC, null);
    expect(start).toBe(4 * SEC);
    expect(start + 2 * SEC).toBe(6 * SEC);
  });

  it("skips a gap the clip cannot fit in", () => {
    const lane = [at("a", 0, 4 * SEC), at("b", 5 * SEC, 5 * SEC)];
    expect(nearestFreeStart(lane, 4_200_000, 2 * SEC, null)).toBe(10 * SEC);
  });

  it("does not collide with the clip being dragged", () => {
    const lane = [at("a", 0, 4 * SEC)];
    expect(nearestFreeStart(lane, 1 * SEC, 4 * SEC, "a")).toBe(1 * SEC);
  });

  it("never lands before zero", () => {
    const lane = [at("a", 0, 4 * SEC)];
    expect(nearestFreeStart(lane, -3 * SEC, 2 * SEC, null)).toBe(4 * SEC);
  });

  it("always has an answer, because the space after the last clip is unbounded", () => {
    const lane = [at("a", 0, 10 * SEC)];
    expect(nearestFreeStart(lane, 2 * SEC, 30 * SEC, null)).toBe(10 * SEC);
  });
});

describe("freeSpan", () => {
  const lane = [at("a", 0, 4 * SEC), at("b", 4 * SEC, 2 * SEC), at("c", 8 * SEC, 2 * SEC)];

  it("reports the neighbours a trim would run into", () => {
    expect(freeSpan(lane, "b", { start: 4 * SEC, duration: 2 * SEC })).toEqual({
      min: 4 * SEC,
      max: 8 * SEC,
    });
  });

  it("is unbounded to the right when nothing follows", () => {
    expect(freeSpan(lane, "c", { start: 8 * SEC, duration: 2 * SEC })).toEqual({
      min: 6 * SEC,
      max: Number.POSITIVE_INFINITY,
    });
  });

  it("starts at zero when nothing precedes", () => {
    expect(freeSpan(lane, "a", { start: 0, duration: 4 * SEC })).toEqual({
      min: 0,
      max: 4 * SEC,
    });
  });
});
