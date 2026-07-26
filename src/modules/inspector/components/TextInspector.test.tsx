/**
 * The title panel.
 *
 * Three contracts are worth defending and everything here is one of them:
 *
 * - **Typing is local and the write is debounced.** A round trip per keystroke
 *   is a cursor that jumps and a preview session per character.
 * - **A burst is one write, carrying the last value.** Not the first, and not
 *   several writes racing.
 * - **The document still wins.** An undo, or selecting another title, replaces
 *   what the panel shows — but the *echo* of our own write must not, or the
 *   cursor resets on every keystroke.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Inspector } from "@/modules/inspector/components/Inspector";
import { useProjectStore } from "@/modules/project/store";
import type { Project, Segment, TextMaterial, Track } from "@/modules/project/types";
import { useTimelineStore } from "@/modules/timeline/store";
import { IDENTITY_TRANSFORM, makeEditResponse, makeProject, makeSegment } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

const TITLE: TextMaterial = {
  id: "text-1",
  content: "Hello",
  font_family: "DejaVu Sans",
  font_size: 92,
  color: [1, 1, 1, 1],
  bold: true,
  italic: false,
  align: "center",
  stroke_width: 4,
  stroke_color: [0, 0, 0, 1],
  shadow: null,
  background: null,
};

function titleSegment(): Segment {
  return makeSegment("segment-title", { material_id: "text-1" });
}

function textTrack(segments: Segment[]): Track {
  return {
    id: "track-text",
    kind: "text",
    name: "Text 1",
    segments,
    muted: false,
    locked: false,
    hidden: false,
    volume: 1,
  };
}

function projectWithTitle(material: TextMaterial = TITLE): Project {
  return makeProject({
    materials: {
      videos: [],
      audios: [],
      images: [],
      texts: [material],
      links: [],
      transitions: [],
      extras: {},
    },
    tracks: [textTrack([titleSegment()])],
  });
}

function open(project: Project) {
  useProjectStore.getState().loadProject(project);
  useTimelineStore.getState().select("segment-title");
  return render(<Inspector />);
}

function textarea(): HTMLTextAreaElement {
  return screen.getByLabelText("Title text") as HTMLTextAreaElement;
}

/** The material as it crossed the boundary on the last `text_set`. */
function lastSent(ipc: IpcHarness): TextMaterial {
  const call = ipc.lastCall("text_set");
  if (!call) throw new Error("text_set was never called");
  return call.material as TextMaterial;
}

let ipc: IpcHarness;

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  ipc = installIpc();
  ipc.handle("text_fonts", ["Deja Vu Serif", "DejaVu Sans", "Inter"]);
  useTimelineStore.getState().select(null);
  useProjectStore.setState({ project: null, error: null });
});

afterEach(() => {
  ipc.restore();
  vi.useRealTimers();
});

describe("the panel appears for a title and not for anything else", () => {
  it("shows the title controls when the selected clip names a text material", () => {
    open(projectWithTitle());
    expect(textarea()).toHaveValue("Hello");
    expect(screen.getByLabelText("Outline")).toBeInTheDocument();
  });

  it("shows nothing text-shaped for an ordinary clip", () => {
    const project = makeProject({ tracks: [textTrack([makeSegment("segment-title")])] });
    open(project);
    expect(screen.queryByLabelText("Title text")).not.toBeInTheDocument();
  });
});

describe("typing", () => {
  it("shows the keystroke immediately and writes nothing yet", () => {
    ipc.handle("text_set", makeEditResponse(projectWithTitle()));
    open(projectWithTitle());

    fireEvent.change(textarea(), { target: { value: "Hello w" } });

    expect(textarea()).toHaveValue("Hello w");
    expect(ipc.count("text_set")).toBe(0);
  });

  it("folds a burst into one write carrying the last value", async () => {
    ipc.handle("text_set", makeEditResponse(projectWithTitle()));
    open(projectWithTitle());

    for (const value of ["H", "He", "Hel", "Hell", "Hello!"]) {
      fireEvent.change(textarea(), { target: { value } });
      vi.advanceTimersByTime(30);
    }
    await vi.advanceTimersByTimeAsync(200);

    expect(ipc.count("text_set")).toBe(1);
    expect(lastSent(ipc).content).toBe("Hello!");
  });

  it("sends the whole material, not a patch, keeping the id", async () => {
    ipc.handle("text_set", makeEditResponse(projectWithTitle()));
    open(projectWithTitle());

    fireEvent.change(textarea(), { target: { value: "New words" } });
    await vi.advanceTimersByTimeAsync(200);

    expect(lastSent(ipc)).toEqual({ ...TITLE, content: "New words" });
  });

  it("does not reset the field when the document echoes the write back", async () => {
    // What Rust answers with: a new object, same contents plus the edit.
    ipc.handle("text_set", (payload) =>
      makeEditResponse(projectWithTitle(payload.material as TextMaterial)),
    );
    open(projectWithTitle());

    fireEvent.change(textarea(), { target: { value: "Typed" } });
    await vi.advanceTimersByTimeAsync(200);
    expect(textarea()).toHaveValue("Typed");

    // And the next keystroke starts from what is on screen, not from the
    // document's older copy.
    fireEvent.change(textarea(), { target: { value: "Typed more" } });
    await vi.advanceTimersByTimeAsync(200);
    expect(lastSent(ipc).content).toBe("Typed more");
  });

  it("adopts the document when it changes under it — an undo, or another title", async () => {
    ipc.handle("text_set", makeEditResponse(projectWithTitle()));
    open(projectWithTitle());

    fireEvent.change(textarea(), { target: { value: "Draft" } });
    await vi.advanceTimersByTimeAsync(200);

    // Something else replaced the document: the undo of an earlier edit.
    useProjectStore
      .getState()
      .refreshDocument(projectWithTitle({ ...TITLE, content: "From the past" }));

    await waitFor(() => expect(textarea()).toHaveValue("From the past"));
  });

  it("writes nothing after the panel goes away mid-burst", async () => {
    ipc.handle("text_set", makeEditResponse(projectWithTitle()));
    const view = open(projectWithTitle());

    fireEvent.change(textarea(), { target: { value: "Half typed" } });
    view.unmount();
    await vi.advanceTimersByTimeAsync(500);

    expect(ipc.count("text_set")).toBe(0);
  });
});

describe("the controls", () => {
  beforeEach(() => {
    ipc.handle("text_set", makeEditResponse(projectWithTitle()));
  });

  it("toggles bold", async () => {
    open(projectWithTitle({ ...TITLE, bold: false }));

    fireEvent.click(screen.getByRole("button", { name: /bold/i }));
    await vi.advanceTimersByTimeAsync(200);

    expect(lastSent(ipc).bold).toBe(true);
  });

  it("sets the alignment", async () => {
    open(projectWithTitle());

    fireEvent.click(screen.getByLabelText("Align left"));
    await vi.advanceTimersByTimeAsync(200);

    expect(lastSent(ipc).align).toBe("left");
  });

  it("turns the outline off with a zero width and on with a size-relative one", async () => {
    open(projectWithTitle());

    fireEvent.click(screen.getByLabelText("Outline"));
    await vi.advanceTimersByTimeAsync(200);
    expect(lastSent(ipc).stroke_width).toBe(0);

    fireEvent.click(screen.getByLabelText("Outline"));
    await vi.advanceTimersByTimeAsync(200);
    // 92 px title: a fixed 4 px would be nearly invisible, so it scales.
    expect(lastSent(ipc).stroke_width).toBe(Math.round(92 * 0.045));
  });

  it("hides the outline's own controls while it is off", () => {
    open(projectWithTitle({ ...TITLE, stroke_width: 0 }));
    expect(screen.queryByLabelText("Outline colour")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Width")).not.toBeInTheDocument();
  });

  it("gives a switched-on shadow an offset and a blur, not zeroes", async () => {
    open(projectWithTitle());

    fireEvent.click(screen.getByLabelText("Shadow"));
    await vi.advanceTimersByTimeAsync(200);

    const shadow = lastSent(ipc).shadow;
    expect(shadow).not.toBeNull();
    expect(shadow?.offset[0]).toBeGreaterThan(0);
    expect(shadow?.blur).toBeGreaterThan(0);
    // Transparent black: a fully opaque shadow reads as a second copy of the text.
    expect(shadow?.color[3]).toBeLessThan(1);
  });

  it("switches the shadow back off as null, which is how the document spells absent", async () => {
    open(projectWithTitle({ ...TITLE, shadow: { color: [0, 0, 0, 1], offset: [4, 4], blur: 8 } }));

    fireEvent.click(screen.getByLabelText("Shadow"));
    await vi.advanceTimersByTimeAsync(200);

    expect(lastSent(ipc).shadow).toBeNull();
  });

  it("sends a colour as sRGB 0..1, not as bytes", async () => {
    open(projectWithTitle());

    fireEvent.change(screen.getByLabelText("Colour"), { target: { value: "#ff8000" } });
    await vi.advanceTimersByTimeAsync(200);

    expect(lastSent(ipc).color).toEqual([1, 128 / 255, 0, 1]);
  });

  it("keeps the alpha when only the swatch moves", async () => {
    open(projectWithTitle({ ...TITLE, color: [1, 1, 1, 0.5] }));

    fireEvent.change(screen.getByLabelText("Colour"), { target: { value: "#000000" } });
    await vi.advanceTimersByTimeAsync(200);

    expect(lastSent(ipc).color).toEqual([0, 0, 0, 0.5]);
  });

  it("refuses a cleared number field rather than putting a NaN in the document", async () => {
    open(projectWithTitle());

    fireEvent.change(screen.getByLabelText("Size"), { target: { value: "" } });
    await vi.advanceTimersByTimeAsync(200);

    expect(ipc.count("text_set")).toBe(0);
  });

  it("offers the fonts the machine has, and keeps a family it does not", async () => {
    open(projectWithTitle({ ...TITLE, font_family: "Bebas Neue" }));

    await waitFor(() => expect(screen.getByLabelText("Font")).toHaveDisplayValue(/Bebas Neue/));
    expect(screen.getByRole("option", { name: "Inter" })).toBeInTheDocument();
  });
});

describe("quick placement", () => {
  it("moves the clip with a set_transform, so it is undoable and keyframable", async () => {
    ipc.handle("timeline_apply", makeEditResponse(projectWithTitle()));
    open(projectWithTitle());

    fireEvent.click(screen.getByRole("button", { name: "Lower third" }));

    await waitFor(() => expect(ipc.count("timeline_apply")).toBe(1));
    const command = ipc.lastCall("timeline_apply")?.command as {
      type: string;
      after: { position: [number, number] };
    };
    expect(command.type).toBe("set_transform");
    // +y is up in half-canvas units, so the lower third is negative.
    expect(command.after.position[1]).toBeLessThan(0);
    expect(command.after.position[0]).toBe(0);
    // The rest of the transform rides along, so undo restores it exactly.
    expect(command.after).toMatchObject({ scale: IDENTITY_TRANSFORM.scale, opacity: 1 });
  });
});
