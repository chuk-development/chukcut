/**
 * Track management is mostly Rust's; what lives here is the naming of a fresh
 * lane and the geometry of a header drag — lanes are different heights, so a
 * reorder target cannot be had by dividing pixels.
 */

import { describe, expect, it } from "vitest";

import { blankTrack, nextTrackName, reorderTarget } from "@/modules/timeline/lib/tracks";
import { trackHeight } from "@/modules/timeline/store";
import { makeProject, makeTrack } from "@/test/fixtures";

describe("naming a fresh lane", () => {
  it("counts lanes of its own kind, whatever order they sit in", () => {
    const project = makeProject({
      tracks: [
        makeTrack("v1", { kind: "video" }),
        makeTrack("a1", { kind: "audio" }),
        makeTrack("v2", { kind: "video" }),
      ],
    });
    expect(nextTrackName(project, "video")).toBe("Video 3");
    expect(nextTrackName(project, "audio")).toBe("Audio 2");
    expect(nextTrackName(project, "text")).toBe("Text 1");
  });

  it("builds a lane with every switch off and unit volume", () => {
    const track = blankTrack(makeProject(), "audio");
    expect(track.kind).toBe("audio");
    expect(track.segments).toEqual([]);
    expect(track.muted).toBe(false);
    expect(track.locked).toBe(false);
    expect(track.hidden).toBe(false);
    expect(track.volume).toBe(1);
  });
});

describe("where a dragged header lands", () => {
  // video (56) / audio (44) / text (34): deliberately mixed heights.
  const tracks = [
    makeTrack("v", { kind: "video" }),
    makeTrack("a", { kind: "audio" }),
    makeTrack("t", { kind: "text" }),
  ];

  it("stays put for a wobble", () => {
    expect(reorderTarget(tracks, 1, 4)).toBe(1);
    expect(reorderTarget(tracks, 1, -4)).toBe(1);
  });

  it("yields a place once the centre passes the middle of a neighbour", () => {
    // Dragging the video lane down. Its centre reaches the audio lane's centre
    // after half of each; the text lane's centre is a whole audio lane further.
    const toAudio = trackHeight("video") / 2 + trackHeight("audio") / 2 + 1;
    expect(reorderTarget(tracks, 0, toAudio)).toBe(1);
    expect(reorderTarget(tracks, 0, toAudio - 2)).toBe(0);
    const toText = toAudio + trackHeight("audio") / 2 + trackHeight("text") / 2;
    expect(reorderTarget(tracks, 0, toText)).toBe(2);

    // And back up: the text lane dragged past the audio lane's centre.
    const upToAudio = -(trackHeight("text") / 2 + trackHeight("audio") / 2 + 1);
    expect(reorderTarget(tracks, 2, upToAudio)).toBe(1);
  });

  it("clamps to the ends however far the pointer runs", () => {
    expect(reorderTarget(tracks, 0, -10_000)).toBe(0);
    expect(reorderTarget(tracks, 0, 10_000)).toBe(2);
    expect(reorderTarget(tracks, 2, -10_000)).toBe(0);
  });
});
