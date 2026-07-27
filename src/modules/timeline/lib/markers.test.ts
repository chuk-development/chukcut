/**
 * The marker command builders. Round-tripping through the document and exact
 * undo are Rust's tests (`ops.rs`, `document.rs`); what this side owns is the
 * shape of the commands it sends — the whole `before` for the stale check, the
 * id surviving every patch — and the defaults M hands out.
 */

import { describe, expect, it } from "vitest";

import type { Marker } from "@/modules/project/types";
import { markersOf } from "@/modules/project/types";
import { blankMarker, removeMarkerCommand, setMarkerCommand } from "@/modules/timeline/lib/markers";
import { makeProject } from "@/test/fixtures";

const MARKER: Marker = { id: "m1", time: 2_000_000, label: "beat", color: "red" };

describe("blankMarker", () => {
  it("rounds to whole micros and never sits before the timeline", () => {
    expect(blankMarker(1_000_000.6, () => "id").time).toBe(1_000_001);
    expect(blankMarker(-500, () => "id").time).toBe(0);
    const marker = blankMarker(0, () => "id");
    expect(marker.label).toBe("");
    expect(marker.color).toBe("blue");
  });
});

describe("setMarkerCommand", () => {
  it("carries the whole before and keeps the id through any patch", () => {
    const command = setMarkerCommand(MARKER, { time: 3_000_000, id: "hijack" } as Partial<Marker>);
    // `before` is the stale check: Rust refuses the edit if the document's
    // marker no longer matches it. And the id is an identity, not a field —
    // even a patch that tries cannot change it.
    expect(command).toEqual({
      type: "set_marker",
      before: MARKER,
      after: { id: "m1", time: 3_000_000, label: "beat", color: "red" },
    });
  });

  it("patches one field and leaves the rest", () => {
    const recoloured = setMarkerCommand(MARKER, { color: "green" });
    if (recoloured.type !== "set_marker") throw new Error("builder builds set_marker");
    expect(recoloured.after).toEqual({ ...MARKER, color: "green" });
  });
});

describe("removeMarkerCommand", () => {
  it("carries the whole marker so undo can rebuild it", () => {
    expect(removeMarkerCommand(MARKER)).toEqual({ type: "remove_marker", marker: MARKER });
  });
});

describe("markersOf", () => {
  it("tolerates documents from before markers existed", () => {
    expect(markersOf(null)).toEqual([]);
    expect(markersOf(makeProject())).toEqual([]);
    expect(markersOf(makeProject({ markers: [MARKER] }))).toEqual([MARKER]);
  });
});
