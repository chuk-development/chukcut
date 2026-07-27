/**
 * The selection, as a set.
 *
 * It used to be one id, and every consumer read that id directly. What makes
 * the replacement safe is that the ambiguous cases have one answer each and it
 * is written down here: what a plain click does to an existing selection, where
 * a shift-click measures from once the anchor's clip is gone, and what "the"
 * selected clip means when there are four.
 */

import { beforeEach, describe, expect, it } from "vitest";

import { soleSelection, useTimelineStore } from "@/modules/timeline/store";

beforeEach(() => {
  useTimelineStore.setState({ selection: [], selectionAnchor: null, clipboard: [] });
});

const store = () => useTimelineStore.getState();

describe("selecting", () => {
  it("replaces the selection on a plain select, and clears it on null", () => {
    store().select("a");
    expect(store().selection).toEqual(["a"]);
    store().select("b");
    expect(store().selection).toEqual(["b"]);
    store().select(null);
    expect(store().selection).toEqual([]);
    expect(store().selectionAnchor).toBeNull();
  });

  it("adds and removes on a toggle", () => {
    store().select("a");
    store().toggleSelection("b");
    expect(store().selection).toEqual(["a", "b"]);
    store().toggleSelection("a");
    expect(store().selection).toEqual(["b"]);
  });

  it("moves the anchor off a clip that was toggled out of the selection", () => {
    // Otherwise the next shift-click extends from a clip that is no longer in
    // the selection, and the run it takes has nothing to do with what is on
    // screen.
    store().select("a");
    store().toggleSelection("b");
    expect(store().selectionAnchor).toBe("b");
    store().toggleSelection("b");
    expect(store().selectionAnchor).toBe("a");
  });

  it("keeps the anchor where it was when a run is added", () => {
    // Shift-clicking twice extends from the original clip both times, which is
    // what every list in every OS does.
    store().select("a");
    store().extendSelection(["a", "b", "c"]);
    expect(store().selection).toEqual(["a", "b", "c"]);
    expect(store().selectionAnchor).toBe("a");
  });

  it("never holds the same clip twice", () => {
    store().selectMany(["a", "b", "a"]);
    expect(store().selection).toEqual(["a", "b"]);
    store().extendSelection(["b", "c"]);
    expect(store().selection).toEqual(["a", "b", "c"]);
  });
});

describe("the sole selection", () => {
  it("is the clip when there is exactly one", () => {
    expect(soleSelection(["a"])).toBe("a");
  });

  it("is nothing when there are none, and nothing when there are several", () => {
    // The second half is the point: with four clips selected there is no "the"
    // clip, and answering with the first would let the inspector edit a clip
    // the user is not looking at.
    expect(soleSelection([])).toBeNull();
    expect(soleSelection(["a", "b"])).toBeNull();
  });
});

describe("the clipboard", () => {
  it("is not cleared by anything the selection does", () => {
    // It outlives the document it came from, so it certainly outlives a click.
    store().setClipboard([
      {
        segment: { id: "a" } as never,
        trackId: "video-1",
        trackKind: "video",
        offset: 0,
        linkGroup: null,
        materialPath: "/media/a.mp4",
      },
    ]);
    store().select("b");
    store().select(null);
    expect(store().clipboard).toHaveLength(1);
  });
});

describe("the in/out marks", () => {
  beforeEach(() => {
    useTimelineStore.setState({ markIn: null, markOut: null, exportRange: null });
  });

  it("holds no range until both marks are set", () => {
    // The export side reads `exportRange` defensively: null means "the whole
    // project", and a half-set pair must read as exactly that.
    store().setMarkIn(1_000_000);
    expect(store().markIn).toBe(1_000_000);
    expect(store().exportRange).toBeNull();

    store().setMarkOut(3_000_000);
    expect(store().exportRange).toEqual({ start: 1_000_000, end: 3_000_000 });
  });

  it("holds no range when in does not precede out", () => {
    store().setMarkIn(3_000_000);
    store().setMarkOut(1_000_000);
    expect(store().exportRange).toBeNull();

    // Equal marks are an empty range, which is no range.
    store().setMarkOut(3_000_000);
    expect(store().exportRange).toBeNull();

    // Moving the in mark back below the out mark revives the range.
    store().setMarkIn(2_000_000);
    expect(store().exportRange).toEqual({ start: 2_000_000, end: 3_000_000 });
  });

  it("clamps marks to the timeline and rounds them to whole micros", () => {
    store().setMarkIn(-5);
    store().setMarkOut(1_000_000.6);
    expect(store().markIn).toBe(0);
    expect(store().markOut).toBe(1_000_001);
    expect(store().exportRange).toEqual({ start: 0, end: 1_000_001 });
  });

  it("X clears both marks and the range with them", () => {
    store().setMarkIn(1_000_000);
    store().setMarkOut(2_000_000);
    store().clearMarks();
    expect(store().markIn).toBeNull();
    expect(store().markOut).toBeNull();
    expect(store().exportRange).toBeNull();
  });
});
