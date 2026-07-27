import { describe, expect, it } from "vitest";

import { MICROS_PER_SECOND } from "@/lib/time";
import type { Micros, Project, Segment } from "@/modules/project/types";
import {
  buildSnapContext,
  SNAP_RADIUS_PX,
  type SnapContext,
  snapInstant,
  snapRadius,
  snapRange,
} from "@/modules/timeline/lib/snapping";
import { DEFAULT_ZOOM, MAX_ZOOM, MIN_ZOOM } from "@/modules/timeline/store";

const SEC = MICROS_PER_SECOND;

function clip(id: string, start: Micros, duration: Micros): Segment {
  return {
    id,
    material_id: "m",
    target_range: { start, duration },
    source_range: { start: 0, duration },
    render_index: 0,
    speed: 1,
    volume: 1,
    transform: {
      position: [0, 0],
      scale: [1, 1],
      rotation: 0,
      opacity: 1,
      flip_h: false,
      flip_v: false,
    },
    crop: null,
    extras: [],
    keyframes: [],
  };
}

function projectWith(...lanes: Segment[][]): Project {
  return {
    id: "p",
    schema_version: 1,
    name: "test",
    created_at: 0,
    updated_at: 0,
    canvas: { width: 1080, height: 1920, background: [0, 0, 0, 1] },
    fps: 30,
    materials: {
      videos: [],
      audios: [],
      images: [],
      texts: [],
      links: [],
      transitions: [],
      extras: {},
    },
    tracks: lanes.map((segments, index) => ({
      id: `t${index}`,
      kind: "video" as const,
      name: `V${index}`,
      segments,
      muted: false,
      locked: false,
      hidden: false,
      volume: 1,
    })),
  };
}

/** A context with the grid off, for tests that are about the threshold itself. */
function only(...times: Micros[]): SnapContext {
  return { candidates: times.map((time) => ({ time, kind: "clip" as const })), secondGrid: false };
}

describe("buildSnapContext", () => {
  const project = projectWith([clip("a", 0, 2 * SEC)], [clip("b", 5 * SEC, 2 * SEC)]);

  it("collects both edges of every clip on every track", () => {
    const context = buildSnapContext(project, 3 * SEC, null);
    const edges = context.candidates.filter((c) => c.kind === "clip").map((c) => c.time);
    expect(edges.sort((x, y) => x - y)).toEqual([0, 2 * SEC, 5 * SEC, 7 * SEC]);
  });

  it("offers zero and the playhead", () => {
    const context = buildSnapContext(project, 3 * SEC, null);
    expect(context.candidates).toContainEqual({ time: 0, kind: "origin" });
    expect(context.candidates).toContainEqual({ time: 3 * SEC, kind: "playhead" });
  });

  it("leaves out the clip being dragged, whose edges travel with it", () => {
    const context = buildSnapContext(project, 3 * SEC, "a");
    const edges = context.candidates.filter((c) => c.kind === "clip").map((c) => c.time);
    expect(edges.sort((x, y) => x - y)).toEqual([5 * SEC, 7 * SEC]);
  });

  it("omits the playhead when the playhead is the thing moving", () => {
    const context = buildSnapContext(project, null, null);
    expect(context.candidates.some((c) => c.kind === "playhead")).toBe(false);
  });

  it("takes markers as candidates", () => {
    const context = buildSnapContext(project, null, null, [1_500_000]);
    expect(context.candidates).toContainEqual({ time: 1_500_000, kind: "marker" });
  });
});

describe("snapInstant", () => {
  it("is null when nothing is within the radius", () => {
    // Half a second from either whole second, so the grid cannot rescue it.
    expect(snapInstant(5_500_000, buildSnapContext(null, null, null), 1_000)).toBeNull();
  });

  it("takes the nearest candidate", () => {
    const hit = snapInstant(4_180_000, only(4_100_000, 4_300_000), 100_000);
    expect(hit?.value).toBe(4_100_000);
    expect(hit?.at).toBe(4_100_000);
    expect(hit?.edge).toBe("start");
  });

  it("prefers a clip edge to a second boundary at the same distance", () => {
    const context: SnapContext = {
      candidates: [{ time: 4_200_000, kind: "clip" }],
      secondGrid: true,
    };
    // 100 ms from the clip edge on one side and from 4 s on the other.
    const hit = snapInstant(4_100_000, context, 150_000);
    expect(hit?.kind).toBe("clip");
    expect(hit?.value).toBe(4_200_000);
  });

  it("still takes a second boundary when it is strictly nearer", () => {
    const context: SnapContext = {
      candidates: [{ time: 4_300_000, kind: "clip" }],
      secondGrid: true,
    };
    const hit = snapInstant(4_050_000, context, 400_000);
    expect(hit?.kind).toBe("second");
    expect(hit?.value).toBe(4 * SEC);
  });
});

describe("snapRange", () => {
  it("docks the trailing edge against the clip on its right", () => {
    // The leading edge is near nothing: only the far end of the clip is in
    // range. Considering the start alone is what used to make this impossible.
    const hit = snapRange(6_770_000, 3_200_000, only(10 * SEC), 100_000);
    expect(hit?.edge).toBe("end");
    expect(hit?.at).toBe(10 * SEC);
    expect(hit?.value).toBe(6_800_000);
    // Butted, not nearly butted.
    expect((hit?.value ?? 0) + 3_200_000).toBe(10 * SEC);
  });

  it("takes the smaller correction when both edges are in range", () => {
    const hit = snapRange(4_060_000, 7_100_000, only(4 * SEC, 11 * SEC), 200_000);
    expect(hit?.edge).toBe("start");
    expect(hit?.value).toBe(4 * SEC);
  });

  it("is null when neither edge is close to anything", () => {
    expect(snapRange(4_500_000, 2_300_000, buildSnapContext(null, null, null), 100_000)).toBeNull();
  });
});

describe("snapRadius", () => {
  it("converts pixels to microseconds at the current zoom", () => {
    // Zoom is pixels per microsecond: 1e-4 is 100 px per second, so 9 px is 90 ms.
    expect(snapRadius(DEFAULT_ZOOM)).toBe(SNAP_RADIUS_PX / DEFAULT_ZOOM);
    expect(snapRadius(1e-4)).toBe(90_000);
    expect(snapRadius(4e-3)).toBe(2_250);
    expect(snapRadius(2e-6)).toBe(4_500_000);
  });

  it("pulls from the same distance on screen at every zoom", () => {
    const target = 4 * SEC;
    for (const zoom of [MIN_ZOOM, 5e-5, DEFAULT_ZOOM, 1e-3, MAX_ZOOM]) {
      const radius = snapRadius(zoom);
      const inside = target + (SNAP_RADIUS_PX - 1) / zoom;
      const outside = target + (SNAP_RADIUS_PX + 1) / zoom;
      expect(snapInstant(inside, only(target), radius)?.value).toBe(target);
      expect(snapInstant(outside, only(target), radius)).toBeNull();
    }
  });
});

describe("markers as snap targets", () => {
  it("puts every marker time into the context and reports the hit as a marker", () => {
    const context = buildSnapContext(null, null, null, [2 * SEC, 7 * SEC]);
    const hit = snapInstant(2 * SEC + 40_000, context, 100_000);
    expect(hit?.value).toBe(2 * SEC);
    expect(hit?.kind).toBe("marker");
  });

  it("lets a clip edge beat a marker at the same distance", () => {
    // Docking to material is the edit that was meant; the marker at the same
    // spot is a coincidence. The tie-break in PRIORITY says so.
    const project = projectWith([clip("a", 4 * SEC, SEC)]);
    const context = buildSnapContext(project, null, null, [6 * SEC]);
    // 5.5s sits exactly between the clip end at 5s and the marker at 6s.
    const hit = snapInstant(5_500_000, context, SEC);
    expect(hit?.kind).toBe("clip");
    expect(hit?.value).toBe(5 * SEC);
  });

  it("pulls a dragged clip's edge onto a marker", () => {
    const context = buildSnapContext(null, null, null, [10 * SEC]);
    // The clip's *end* lands near the marker, so the whole clip shifts left.
    const hit = snapRange(8 * SEC + 30_000, 2 * SEC, context, 100_000);
    expect(hit?.edge).toBe("end");
    expect(hit?.kind).toBe("marker");
    expect(hit?.value).toBe(8 * SEC);
  });
});
