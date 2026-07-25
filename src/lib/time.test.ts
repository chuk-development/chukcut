import { describe, expect, it } from "vitest";

import {
  clamp,
  formatDuration,
  formatRulerLabel,
  formatTimecode,
  frameDuration,
  frameToMicros,
  microsToFrame,
} from "@/lib/time";

/** The rates an editor actually meets. The fractional two are where rounding breaks. */
const RATES = [23.976, 25, 29.97, 30, 60];

describe("frame and time conversion", () => {
  it("gives a whole number of microseconds for one frame at every rate", () => {
    expect(frameDuration(23.976)).toBe(41708);
    expect(frameDuration(25)).toBe(40000);
    expect(frameDuration(29.97)).toBe(33367);
    expect(frameDuration(30)).toBe(33333);
    expect(frameDuration(60)).toBe(16667);
  });

  it("never returns a zero-length frame, which would make stepping a no-op", () => {
    // A silly rate is still a rate; 0 µs per frame would freeze the step buttons.
    for (const fps of [...RATES, 1000, 1e7]) {
      expect(frameDuration(fps)).toBeGreaterThanOrEqual(1);
    }
  });

  it("survives a round trip from frame to microseconds and back at every rate", () => {
    for (const fps of RATES) {
      for (let frame = 0; frame <= 600; frame++) {
        expect(microsToFrame(frameToMicros(frame, fps), fps)).toBe(frame);
      }
    }
  });

  it("converts the first frame of each second correctly at 29.97", () => {
    // 29.97 is 30000/1001, so the second boundary and the frame boundary drift
    // apart. Frame 30 is past one second, frame 29 is not.
    expect(frameToMicros(29, 29.97)).toBe(967_634);
    expect(frameToMicros(30, 29.97)).toBe(1_001_001);
    expect(microsToFrame(1_000_000, 29.97)).toBe(30);
  });
});

describe("the transport timecode", () => {
  it("reads HH:MM:SS:FF, dropping the hours below one hour", () => {
    expect(formatTimecode(0, 30)).toBe("00:00:00");
    expect(formatTimecode(65_500_000, 30)).toBe("01:05:15");
    expect(formatTimecode(3_600_000_000, 30)).toBe("01:00:00:00");
    expect(formatTimecode(3_661_000_000, 25)).toBe("01:01:01:00");
  });

  it("never displays a frame number the rate does not have", () => {
    // The last microsecond of a second must floor to the last real frame. If
    // this rounded instead, 30 fps would show frame 30 — a frame that does not
    // exist, and the bug this function's comment is about.
    for (const fps of RATES) {
      for (const micros of [999_999, 1_999_999, 41_999_999, 999_998, 500_000]) {
        const frames = Number(formatTimecode(micros, fps).split(":").pop());
        expect(frames).toBeLessThan(Math.ceil(fps));
      }
    }
    expect(formatTimecode(999_999, 30)).toBe("00:00:29");
    expect(formatTimecode(999_999, 60)).toBe("00:00:59");
    expect(formatTimecode(999_999, 23.976)).toBe("00:00:23");
    expect(formatTimecode(999_999, 29.97)).toBe("00:00:29");
  });

  it("clamps a negative position to zero rather than rendering a minus sign", () => {
    expect(formatTimecode(-5_000_000, 30)).toBe("00:00:00");
  });

  it("pads every field to two digits", () => {
    expect(formatTimecode(1_000_000 + 33_333, 30)).toBe("00:01:00");
    expect(formatTimecode(9_000_000, 30)).toBe("00:09:00");
  });
});

describe("the tile duration and the ruler label", () => {
  it("rounds the tile duration to the nearest second", () => {
    expect(formatDuration(1_499_999)).toBe("0:01");
    expect(formatDuration(1_500_000)).toBe("0:02");
    expect(formatDuration(0)).toBe("0:00");
    expect(formatDuration(3_661_000_000)).toBe("1:01:01");
  });

  it("floors the ruler label, so a tick never claims a second that has not arrived", () => {
    expect(formatRulerLabel(1_999_999)).toBe("00:01");
    expect(formatRulerLabel(0)).toBe("00:00");
    expect(formatRulerLabel(3_599_999_999)).toBe("59:59");
    expect(formatRulerLabel(3_600_000_000)).toBe("1:00:00");
  });

  it("clamps a negative time to zero", () => {
    expect(formatDuration(-1)).toBe("0:00");
    expect(formatRulerLabel(-1)).toBe("00:00");
  });
});

describe("clamp", () => {
  it("returns the bound when the value is outside it, and the value when inside", () => {
    expect(clamp(5, 0, 10)).toBe(5);
    expect(clamp(-1, 0, 10)).toBe(0);
    expect(clamp(11, 0, 10)).toBe(10);
  });
});
