/**
 * Adding a title, from the button's point of view.
 *
 * What is worth defending here is the gesture, not the payload arithmetic: one
 * press is one command, the command carries the playhead, and the clip that
 * comes back is selected — because a title the user then has to hunt for on the
 * timeline is a title they cannot type into.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useProjectStore } from "@/modules/project/store";
import type { TextAdded } from "@/modules/text/lib/api";
import { addTitle, useTextStore } from "@/modules/text/store";
import { useTimelineStore } from "@/modules/timeline/store";
import { makeEditResponse, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

let ipc: IpcHarness;

function added(): TextAdded {
  return {
    ...makeEditResponse(makeProject(), { undo_label: "Add title" }),
    material_id: "text-1",
    segment_id: "segment-title-1",
    track_id: "track-text-1",
    start: 2_000_000,
  };
}

beforeEach(() => {
  ipc = installIpc();
  useProjectStore.setState({ project: makeProject(), error: null });
  useTimelineStore.getState().select(null);
  useTimelineStore.getState().setPlayhead(0);
  useTextStore.setState({ fonts: [], fontsLoading: false, fontsError: null, adding: false });
});

afterEach(() => {
  ipc.restore();
});

describe("addTitle", () => {
  it("sends the playhead and no content, so Rust owns the defaults", async () => {
    ipc.handle("text_add", added());
    useTimelineStore.getState().setPlayhead(1_500_000);

    await addTitle();

    expect(ipc.lastCall("text_add")).toEqual({ at: 1_500_000, content: null, duration: null });
  });

  it("rounds and floors the playhead, because a segment start is whole microseconds", async () => {
    ipc.handle("text_add", added());
    useTimelineStore.setState({ playhead: -5 });

    await addTitle(undefined, -5);

    expect(ipc.lastCall("text_add")?.at).toBe(0);
  });

  it("carries a preset's words", async () => {
    ipc.handle("text_add", added());

    await addTitle("FOLLOW FOR MORE");

    expect(ipc.lastCall("text_add")?.content).toBe("FOLLOW FOR MORE");
  });

  it("selects the clip it created", async () => {
    ipc.handle("text_add", added());

    const id = await addTitle();

    expect(id).toBe("segment-title-1");
    expect(useTimelineStore.getState().selection).toEqual(["segment-title-1"]);
  });

  it("adopts the document that came back, history state and all", async () => {
    const response = added();
    ipc.handle("text_add", response);

    await addTitle();

    expect(useProjectStore.getState().project).toBe(response.project);
    expect(useProjectStore.getState().undoLabel).toBe("Add title");
    expect(useProjectStore.getState().dirty).toBe(true);
  });

  it("surfaces a refusal instead of throwing, and changes nothing", async () => {
    ipc.fail("text_add", "no project is open");
    const before = useProjectStore.getState().project;

    const id = await addTitle();

    expect(id).toBeNull();
    expect(useProjectStore.getState().error).toBe("no project is open");
    expect(useProjectStore.getState().project).toBe(before);
    expect(useTimelineStore.getState().selection).toEqual([]);
  });

  it("clears the in-flight flag even when the command fails", async () => {
    ipc.fail("text_add", "nope");
    await addTitle();
    expect(useTextStore.getState().adding).toBe(false);
  });

  it("refuses a second press while one is in flight", async () => {
    let release: (value: TextAdded) => void = () => {};
    ipc.handle("text_add", () => new Promise<TextAdded>((resolve) => (release = resolve)));

    const first = addTitle();
    const second = await addTitle();

    expect(second).toBeNull();
    expect(ipc.count("text_add")).toBe(1);

    release(added());
    await first;
  });
});

describe("loadFonts", () => {
  it("asks once and holds the answer", async () => {
    ipc.handle("text_fonts", ["DejaVu Sans", "Inter"]);

    await useTextStore.getState().loadFonts();
    await useTextStore.getState().loadFonts();

    expect(ipc.count("text_fonts")).toBe(1);
    expect(useTextStore.getState().fonts).toEqual(["DejaVu Sans", "Inter"]);
  });

  it("records the failure rather than leaving the panel loading forever", async () => {
    ipc.fail("text_fonts", "fontique fell over");

    await useTextStore.getState().loadFonts();

    expect(useTextStore.getState().fontsLoading).toBe(false);
    expect(useTextStore.getState().fontsError).toBe("fontique fell over");
  });
});
