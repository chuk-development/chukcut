/**
 * Sampling, against the same cases Rust pins.
 *
 * `sampleTrack` is a second implementation of `KeyframeTrack::sample`, and a
 * second implementation is a promise that the two agree. The interesting cases
 * are the ones where they could quietly stop agreeing: the ends, where the
 * value is held rather than extrapolated, and `Hold`, whose progress is zero
 * for the whole span rather than at its start.
 */

import { describe, expect, it } from "vitest";

import {
  easingApply,
  easingForInsertion,
  isAnimated,
  keyframeAt,
  keyframeTolerance,
  nextKeyframe,
  previousKeyframe,
  propertyValue,
  relativeTime,
  sampleTrack,
  snapToFrame,
  staticValue,
} from "@/modules/inspector/lib/keyframes";
import type { Easing, Keyframe, KeyframeTrack } from "@/modules/project/types";
import { makeSegment } from "@/test/fixtures";

function key(time: number, value: number, easing: Easing = "linear"): Keyframe {
  return { time, value, easing };
}

function track(...keyframes: Keyframe[]): KeyframeTrack {
  return { property: "opacity", keyframes };
}

describe("sampling a track", () => {
  it("holds the first and last values instead of extrapolating", () => {
    const fade = track(key(0, 0), key(1_000_000, 1));

    expect(sampleTrack(fade, -5)).toBe(0);
    expect(sampleTrack(fade, 500_000)).toBe(0.5);
    expect(sampleTrack(fade, 2_000_000)).toBe(1);
  });

  it("has no value when there are no keyframes", () => {
    expect(sampleTrack(track(), 0)).toBeNull();
  });

  it("holds a single keyframe's value everywhere", () => {
    const single = track(key(500_000, 0.25));

    expect(sampleTrack(single, 0)).toBe(0.25);
    expect(sampleTrack(single, 10_000_000)).toBe(0.25);
  });

  it("steps rather than ramps through a hold", () => {
    const stepped = track(key(0, 0, "hold"), key(1_000_000, 1));

    expect(sampleTrack(stepped, 999_999)).toBe(0);
    expect(sampleTrack(stepped, 1_000_000)).toBe(1);
  });

  it("uses the easing of the keyframe it is leaving, not the one it is arriving at", () => {
    // `ease_in` is t², so a quarter of the way along is a sixteenth of the way up.
    const eased = track(key(0, 0, "ease_in"), key(1_000_000, 1, "hold"));

    expect(sampleTrack(eased, 250_000)).toBeCloseTo(0.0625, 6);
  });

  it("interpolates the two easings the same way Easing::apply does", () => {
    expect(easingApply("linear", 0.25)).toBe(0.25);
    expect(easingApply("ease_out", 0.5)).toBe(0.75);
    expect(easingApply("ease_in_out", 0.25)).toBeCloseTo(0.125, 6);
    expect(easingApply("ease_in_out", 0.75)).toBeCloseTo(0.875, 6);
    // Out of range progress is clamped, not extrapolated.
    expect(easingApply("linear", 2)).toBe(1);
    expect(easingApply("hold", 1)).toBe(0);
  });
});

describe("reading a property off a segment", () => {
  const animated = makeSegment("segment-1", {
    target_range: { start: 2_000_000, duration: 4_000_000 },
    transform: {
      position: [0, 0],
      scale: [1, 1],
      rotation: 0,
      opacity: 1,
      flip_h: false,
      flip_v: false,
    },
    keyframes: [{ property: "opacity", keyframes: [key(0, 0), key(2_000_000, 1)] }],
  });

  it("times animation from the segment, not from the timeline", () => {
    // The clip starts two seconds in, so the timeline instant that is one
    // second into the clip is three seconds on the ruler.
    expect(relativeTime(animated, 3_000_000)).toBe(1_000_000);
    expect(propertyValue(animated, "opacity", relativeTime(animated, 3_000_000))).toBe(0.5);
  });

  it("falls back to the static value for a property nothing animates", () => {
    expect(propertyValue(animated, "rotation", 0)).toBe(0);
    expect(staticValue(animated, "opacity")).toBe(1);
    expect(isAnimated(animated, "opacity")).toBe(true);
    expect(isAnimated(animated, "rotation")).toBe(false);
  });
});

describe("finding the keyframe under the playhead", () => {
  const fade = track(key(0, 0), key(1_000_000, 1), key(2_000_000, 0));
  const tolerance = keyframeTolerance(30);

  it("counts a playhead within half a frame as sitting on one", () => {
    expect(keyframeAt(fade, 1_000_000, tolerance)?.time).toBe(1_000_000);
    expect(keyframeAt(fade, 1_000_000 + tolerance, tolerance)?.time).toBe(1_000_000);
    expect(keyframeAt(fade, 1_000_000 + tolerance + 1, tolerance)).toBeNull();
  });

  it("skips the keyframe it is standing on when navigating", () => {
    expect(previousKeyframe(fade, 1_000_000, tolerance)?.time).toBe(0);
    expect(nextKeyframe(fade, 1_000_000, tolerance)?.time).toBe(2_000_000);
    expect(previousKeyframe(fade, 0, tolerance)).toBeNull();
    expect(nextKeyframe(fade, 2_000_000, tolerance)).toBeNull();
  });

  it("gives a new keyframe the easing of the span it lands in", () => {
    const shaped = track(key(0, 0, "ease_in_out"), key(1_000_000, 1, "hold"));

    expect(easingForInsertion(shaped, 500_000)).toBe("ease_in_out");
    expect(easingForInsertion(shaped, 1_500_000)).toBe("hold");
    expect(easingForInsertion(null, 0)).toBe("linear");
  });
});

describe("snapping to the frame grid", () => {
  it("lands on exact frame times, however far in", () => {
    expect(snapToFrame(2_000_000, 30)).toBe(2_000_000);
    expect(snapToFrame(30_000_000, 30)).toBe(30_000_000);
    // Half a frame late still belongs to the frame it is nearest.
    expect(snapToFrame(1_010_000, 30)).toBe(1_000_000);
    expect(snapToFrame(1_030_000, 30)).toBe(1_033_333);
  });
});
