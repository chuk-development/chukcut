/**
 * The paste-attributes payload. Applying it — once per clip, one undo step,
 * grade shared as one immutable material — is Rust's contract and Rust's
 * tests (`inspector/edit.rs`); what this side owns is that the payload really
 * is the copied clip's look, grade values included, and nothing of its timing.
 */

import { describe, expect, it } from "vitest";

import { attributesOf } from "@/modules/inspector/lib/clip";
import type { ClipboardEntry } from "@/modules/timeline/lib/clipboard";
import { colorAdjustOf, copyEntries } from "@/modules/timeline/lib/clipboard";
import { makeProject, makeSegment, makeTrack } from "@/test/fixtures";

const SECOND = 1_000_000;

function entryFor(over: Partial<ClipboardEntry> = {}): ClipboardEntry {
  return {
    segment: makeSegment("a", {
      target_range: { start: 3 * SECOND, duration: 2 * SECOND },
      source_range: { start: SECOND, duration: 3 * SECOND },
      speed: 1.5,
      volume: 0.6,
      transform: {
        position: [0.1, 0.2],
        scale: [0.5, 0.5],
        rotation: 45,
        opacity: 0.7,
        flip_h: true,
        flip_v: false,
      },
      crop: { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 },
    }),
    trackId: "video-1",
    trackKind: "video",
    offset: 0,
    linkGroup: null,
    materialPath: "/media/a.mp4",
    ...over,
  };
}

describe("attributesOf", () => {
  it("carries transform, speed, volume and crop — and none of the timing", () => {
    const attributes = attributesOf(entryFor());
    expect(attributes.speed).toBe(1.5);
    expect(attributes.volume).toBe(0.6);
    expect(attributes.transform.rotation).toBe(45);
    expect(attributes.crop).toEqual({ left: 0.1, top: 0.2, right: 0.8, bottom: 0.9 });
    expect(attributes).not.toHaveProperty("target_range");
    expect(attributes).not.toHaveProperty("source_range");
  });

  it("carries the grade as values, so it survives leaving its project", () => {
    const attributes = attributesOf(
      entryFor({
        colorAdjust: {
          id: "grade-from-another-project",
          brightness: 0.2,
          contrast: 1.3,
          saturation: 0.8,
          temperature: -0.1,
          lut: { path: "/looks/warm.cube", intensity: 0.9 },
        },
      }),
    );
    expect(attributes.color).toEqual({
      brightness: 0.2,
      contrast: 1.3,
      saturation: 0.8,
      temperature: -0.1,
      lut: { path: "/looks/warm.cube", intensity: 0.9 },
    });
    // The id deliberately does not travel: Rust mints one fresh shared
    // material from these values per paste.
    expect(attributes.color).not.toHaveProperty("id");
  });

  it("sends no grade for an ungraded source", () => {
    expect(attributesOf(entryFor()).color).toBeNull();
  });
});

describe("copyEntries captures the grade", () => {
  it("resolves the segment's colour material into the entry at copy time", () => {
    const project = makeProject({
      materials: {
        videos: [
          {
            id: "material-a",
            path: "/media/a.mp4",
            width: 1920,
            height: 1080,
            duration: 10 * SECOND,
            fps: 30,
            has_audio: false,
            rotation: 0,
          },
        ],
        audios: [],
        images: [],
        texts: [],
        links: [],
        transitions: [],
        extras: {},
        color_adjusts: [
          {
            id: "grade-1",
            brightness: 0.25,
            contrast: 1,
            saturation: 1,
            temperature: 0.4,
            lut: null,
          },
        ],
      },
      tracks: [makeTrack("video-1", { segments: [makeSegment("a", { extras: ["grade-1"] })] })],
    });

    const [entry] = copyEntries(project, ["a"]);
    expect(entry.colorAdjust?.id).toBe("grade-1");
    expect(entry.colorAdjust?.temperature).toBe(0.4);

    // The resolver ignores extras ids that are not grades.
    const plain = makeProject({
      tracks: [makeTrack("video-1", { segments: [makeSegment("b", { extras: ["not-a-grade"] })] })],
    });
    expect(colorAdjustOf(plain, plain.tracks[0].segments[0])).toBeNull();
  });
});
