/**
 * The Text tab of the media library.
 *
 * One press, one command, and the clip that comes back is selected. That last
 * part is the whole reason the button is worth a test: a title that appears on
 * the timeline without being selected is a title the user has to find before
 * they can type into it.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useProjectStore } from "@/modules/project/store";
import { TextPanel } from "@/modules/text/components/TextPanel";
import type { TextAdded } from "@/modules/text/lib/api";
import { useTextStore } from "@/modules/text/store";
import { useTimelineStore } from "@/modules/timeline/store";
import { makeEditResponse, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

let ipc: IpcHarness;

const ADDED: TextAdded = {
  ...makeEditResponse(makeProject()),
  material_id: "text-1",
  segment_id: "segment-title-1",
  track_id: "track-text-1",
  start: 0,
};

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

describe("Add title", () => {
  it("adds one at the playhead and selects it", async () => {
    ipc.handle("text_add", ADDED);
    useTimelineStore.getState().setPlayhead(4_000_000);
    render(<TextPanel />);

    fireEvent.click(screen.getByRole("button", { name: /add title/i }));

    await waitFor(() => expect(ipc.count("text_add")).toBe(1));
    expect(ipc.lastCall("text_add")?.at).toBe(4_000_000);
    expect(useTimelineStore.getState().selection).toEqual(["segment-title-1"]);
  });

  it("sends no content, so the placeholder and every default stay in Rust", async () => {
    ipc.handle("text_add", ADDED);
    render(<TextPanel />);

    fireEvent.click(screen.getByRole("button", { name: /add title/i }));

    await waitFor(() => expect(ipc.count("text_add")).toBe(1));
    expect(ipc.lastCall("text_add")?.content).toBeNull();
  });

  it("is dead with no project open, rather than failing at the boundary", () => {
    useProjectStore.setState({ project: null });
    render(<TextPanel />);

    expect(screen.getByRole("button", { name: /add title/i })).toBeDisabled();
  });
});

describe("the presets", () => {
  it("send their own words and nothing else", async () => {
    ipc.handle("text_add", ADDED);
    render(<TextPanel />);

    fireEvent.click(screen.getByRole("button", { name: /call to action/i }));

    await waitFor(() => expect(ipc.count("text_add")).toBe(1));
    expect(ipc.lastCall("text_add")?.content).toBe("FOLLOW FOR MORE");
    expect(ipc.lastCall("text_add")?.duration).toBeNull();
  });
});
