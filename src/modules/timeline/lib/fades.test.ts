/**
 * Fades are volume keyframes and nothing else, so what these pin is the
 * translation in both directions: reading fades out of a keyframe track, and
 * the exact commands a released handle writes — including the two edge shapes
 * that corrupt a clip if they go wrong, the shared apex and the drag back to
 * zero.
 */

import { describe, expect, it } from "vitest";

import type { Keyframe, Segment } from "@/modules/project/types";
import type { EditCommand } from "@/modules/timeline/lib/api";
import { clampFades, fadeCommand, fadeKeyframes, fadesOf } from "@/modules/timeline/lib/fades";
import { makeSegment, range } from "@/test/fixtures";

const SECOND = 1_000_000;

function withVolumeKeyframes(keyframes: Keyframe[]): Segment {
  return makeSegment("clip-1", {
    target_range: range(0, 4 * SECOND),
    source_range: range(0, 4 * SECOND),
    keyframes: keyframes.length > 0 ? [{ property: "volume", keyframes }] : [],
  });
}

function commandsOf(command: EditCommand | null): EditCommand[] {
  if (!command) return [];
  return command.type === "composite" ? command.commands : [command];
}

describe("reading fades out of a volume track", () => {
  it("sees no fades on a clip with no keyframes", () => {
    expect(fadesOf(withVolumeKeyframes([]))).toEqual({ fadeIn: 0, fadeOut: 0 });
  });

  it("reads a fade-in from a zero-at-zero ramp", () => {
    const segment = withVolumeKeyframes([
      { time: 0, value: 0, easing: "linear" },
      { time: SECOND, value: 1, easing: "linear" },
    ]);
    expect(fadesOf(segment)).toEqual({ fadeIn: SECOND, fadeOut: 0 });
  });

  it("reads a fade-out from a zero-at-the-end ramp", () => {
    const segment = withVolumeKeyframes([
      { time: 3 * SECOND, value: 1, easing: "linear" },
      { time: 4 * SECOND, value: 0, easing: "linear" },
    ]);
    expect(fadesOf(segment)).toEqual({ fadeIn: 0, fadeOut: SECOND });
  });

  it("reads both from the four-keyframe shape, and from the shared apex", () => {
    const both = withVolumeKeyframes([
      { time: 0, value: 0, easing: "linear" },
      { time: SECOND, value: 1, easing: "linear" },
      { time: 3 * SECOND, value: 1, easing: "linear" },
      { time: 4 * SECOND, value: 0, easing: "linear" },
    ]);
    expect(fadesOf(both)).toEqual({ fadeIn: SECOND, fadeOut: SECOND });

    // Two ramps meeting in the middle share one apex keyframe.
    const apex = withVolumeKeyframes([
      { time: 0, value: 0, easing: "linear" },
      { time: 2 * SECOND, value: 1, easing: "linear" },
      { time: 4 * SECOND, value: 0, easing: "linear" },
    ]);
    expect(fadesOf(apex)).toEqual({ fadeIn: 2 * SECOND, fadeOut: 2 * SECOND });
  });

  it("does not mistake hand-authored automation for a fade", () => {
    // Ducking under a voiceover: starts loud, dips, comes back. No fade.
    const segment = withVolumeKeyframes([
      { time: SECOND, value: 1, easing: "linear" },
      { time: 2 * SECOND, value: 0.3, easing: "linear" },
      { time: 3 * SECOND, value: 1, easing: "linear" },
    ]);
    expect(fadesOf(segment)).toEqual({ fadeIn: 0, fadeOut: 0 });
  });
});

describe("the keyframes a pair of fades spells", () => {
  it("merges the apex when the ramps meet exactly", () => {
    const keyframes = fadeKeyframes(4 * SECOND, { fadeIn: 2 * SECOND, fadeOut: 2 * SECOND });
    expect(keyframes.map((k) => [k.time, k.value])).toEqual([
      [0, 0],
      [2 * SECOND, 1],
      [4 * SECOND, 0],
    ]);
  });

  it("clamps the pair inside the clip, the one being derived second yielding", () => {
    expect(clampFades(4 * SECOND, { fadeIn: 10 * SECOND, fadeOut: 2 * SECOND })).toEqual({
      fadeIn: 4 * SECOND,
      fadeOut: 0,
    });
  });
});

describe("the command a released handle writes", () => {
  it("adds the ramp for a first fade-in", () => {
    const segment = withVolumeKeyframes([]);
    const commands = commandsOf(fadeCommand(segment, { fadeIn: SECOND, fadeOut: 0 }));

    expect(commands).toEqual([
      {
        type: "add_keyframe",
        segment_id: "clip-1",
        property: "volume",
        keyframe: { time: 0, value: 0, easing: "linear" },
      },
      {
        type: "add_keyframe",
        segment_id: "clip-1",
        property: "volume",
        keyframe: { time: SECOND, value: 1, easing: "linear" },
      },
    ]);
  });

  it("retimes a fade by removing the old apex before adding the new one", () => {
    const segment = withVolumeKeyframes([
      { time: 0, value: 0, easing: "linear" },
      { time: SECOND, value: 1, easing: "linear" },
    ]);
    const commands = commandsOf(fadeCommand(segment, { fadeIn: 2 * SECOND, fadeOut: 0 }));

    // The zero-at-zero anchor is in both shapes and is left alone.
    expect(commands.map((c) => c.type)).toEqual(["remove_keyframe", "add_keyframe"]);
    expect(commands).toContainEqual({
      type: "remove_keyframe",
      segment_id: "clip-1",
      property: "volume",
      keyframe: { time: SECOND, value: 1, easing: "linear" },
    });
    expect(commands).toContainEqual({
      type: "add_keyframe",
      segment_id: "clip-1",
      property: "volume",
      keyframe: { time: 2 * SECOND, value: 1, easing: "linear" },
    });
  });

  it("dragging a fade back to zero removes its keyframes outright", () => {
    const segment = withVolumeKeyframes([
      { time: 0, value: 0, easing: "linear" },
      { time: SECOND, value: 1, easing: "linear" },
    ]);
    const commands = commandsOf(fadeCommand(segment, { fadeIn: 0, fadeOut: 0 }));

    expect(commands.every((c) => c.type === "remove_keyframe")).toBe(true);
    expect(commands).toHaveLength(2);
  });

  it("touching one fade leaves the other's keyframes alone", () => {
    const segment = withVolumeKeyframes([
      { time: 0, value: 0, easing: "linear" },
      { time: SECOND, value: 1, easing: "linear" },
      { time: 3 * SECOND, value: 1, easing: "linear" },
      { time: 4 * SECOND, value: 0, easing: "linear" },
    ]);
    const commands = commandsOf(fadeCommand(segment, { fadeIn: SECOND, fadeOut: SECOND / 2 }));

    // Only the fade-out's apex moves; the fade-in's two keyframes and the
    // terminal zero are in both shapes.
    expect(commands).toHaveLength(2);
    expect(commands).toContainEqual({
      type: "remove_keyframe",
      segment_id: "clip-1",
      property: "volume",
      keyframe: { time: 3 * SECOND, value: 1, easing: "linear" },
    });
    expect(commands).toContainEqual({
      type: "add_keyframe",
      segment_id: "clip-1",
      property: "volume",
      keyframe: { time: 3.5 * SECOND, value: 1, easing: "linear" },
    });
  });

  it("does nothing when the handle never moved", () => {
    const segment = withVolumeKeyframes([
      { time: 0, value: 0, easing: "linear" },
      { time: SECOND, value: 1, easing: "linear" },
    ]);
    expect(fadeCommand(segment, { fadeIn: SECOND, fadeOut: 0 })).toBeNull();
  });

  it("yields to a hand-authored keyframe sitting on the target time", () => {
    // The inspector put a keyframe at 2s; a fade whose apex would land there
    // must not collide — Rust would refuse the whole composite.
    const segment = withVolumeKeyframes([{ time: 2 * SECOND, value: 0.5, easing: "linear" }]);
    const commands = commandsOf(fadeCommand(segment, { fadeIn: 2 * SECOND, fadeOut: 0 }));

    expect(commands).toContainEqual({
      type: "add_keyframe",
      segment_id: "clip-1",
      property: "volume",
      keyframe: { time: 0, value: 0, easing: "linear" },
    });
    expect(
      commands.filter((c) => c.type === "add_keyframe" && c.keyframe.time === 2 * SECOND),
    ).toHaveLength(0);
  });
});
