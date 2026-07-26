/**
 * The seam between the bar and the rest of the app.
 *
 * **What is clickable.** `describeMenuState` turns the stores into the facts
 * Rust greys items out from, and every fact here is one an item depends on — a
 * wrong answer is an item that looks available and does nothing, which is the
 * failure the whole arrangement exists to prevent. What the bar then *looks*
 * like is `components/MenuBar.test.tsx`; this file is the state it is drawn
 * from and the actions it runs.
 *
 * **Whether the app may close.** Closing is guarded exactly like New and Open,
 * and the guard's third answer is the one that matters: "Save" only lets the
 * close through if the write actually landed. A cancelled file dialog that
 * still quits is worse than no prompt at all.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useProjectStore } from "@/modules/project/store";
import { useTimelineStore } from "@/modules/timeline/store";
import {
  acceleratorAction,
  answerCloseRequest,
  currentMenuState,
  describeMenuState,
  installMenu,
  MENU_IDS,
  RUST_OWNED_IDS,
  runMenuAction,
  sameMenuState,
} from "@/modules/workspace/lib/menu";
import { useWorkspaceStore } from "@/modules/workspace/store";
import { makeEditResponse, makeSegment, projectWithSegments } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

/** One clip on one video track, running from 1s to 3s. */
const CLIP = makeSegment("clip-1", { target_range: { start: 1_000_000, duration: 2_000_000 } });
const PROJECT = projectWithSegments(CLIP);

const NOTHING_HAPPENING = {
  project: null,
  dirty: false,
  canUndo: false,
  canRedo: false,
} as const;

function documentFacts(overrides: Partial<Parameters<typeof describeMenuState>[0]> = {}) {
  return { project: PROJECT, dirty: false, canUndo: false, canRedo: false, ...overrides };
}

function timelineFacts(overrides: Partial<Parameters<typeof describeMenuState>[1]> = {}) {
  return { playhead: 0, selection: [], clipboard: 0, ...overrides };
}

/** The timeline store's own reset, which every test starts from. */
const QUIET_TIMELINE = { playhead: 0, selection: [], selectionAnchor: null, clipboard: [] };

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  useWorkspaceStore.setState({ discardPrompt: null });
});

afterEach(() => {
  ipc.restore();
  useProjectStore.setState({
    project: null,
    path: null,
    dirty: false,
    error: null,
    canUndo: false,
    canRedo: false,
  });
  useTimelineStore.setState(QUIET_TIMELINE);
});

// ---------------------------------------------------------------------------
// What is clickable
// ---------------------------------------------------------------------------

describe("with no document open", () => {
  it("reports nothing available, rather than whatever the last document said", () => {
    expect(describeMenuState(NOTHING_HAPPENING, timelineFacts())).toEqual({
      has_project: false,
      dirty: false,
      can_undo: false,
      can_redo: false,
      has_selection: false,
      can_split: false,
      can_fit: false,
      has_clipboard: false,
      has_clips: false,
    });
  });

  it("does not inherit a stale selection from the project that was closed", () => {
    const state = describeMenuState(
      NOTHING_HAPPENING,
      timelineFacts({ selection: ["clip-1"], playhead: 2_000_000 }),
    );
    expect(state.has_selection).toBe(false);
    expect(state.can_split).toBe(false);
  });

  it("still offers the clipboard, which is not a fact about a document", () => {
    // Copy in one project, close it, open another, paste. The clipboard lives
    // in the session and outliving the document it came from is the point.
    const state = describeMenuState(NOTHING_HAPPENING, timelineFacts({ clipboard: 2 }));
    expect(state.has_clipboard).toBe(true);
    expect(state.has_project).toBe(false);
  });
});

describe("the clipboard and the selection", () => {
  it("counts a multi-selection as a selection, once, however many clips are in it", () => {
    const many = describeMenuState(
      documentFacts(),
      timelineFacts({ selection: ["clip-1", "clip-2"] }),
    );
    // Two ids, one of which is not in the document: what the bar needs to know
    // is only whether anything real is selected.
    expect(many.has_selection).toBe(true);
  });

  it("says whether the timeline has anything to select at all", () => {
    expect(describeMenuState(documentFacts(), timelineFacts()).has_clips).toBe(true);
    const empty = { ...documentFacts(), project: projectWithSegments() };
    expect(describeMenuState(empty, timelineFacts()).has_clips).toBe(false);
  });

  it("reports the clipboard as a bit, not as its contents", () => {
    expect(describeMenuState(documentFacts(), timelineFacts({ clipboard: 0 })).has_clipboard).toBe(
      false,
    );
    expect(describeMenuState(documentFacts(), timelineFacts({ clipboard: 3 })).has_clipboard).toBe(
      true,
    );
  });
});

describe("saving", () => {
  it("is only worth offering when there is something unwritten", () => {
    expect(describeMenuState(documentFacts({ dirty: false }), timelineFacts()).dirty).toBe(false);
    expect(describeMenuState(documentFacts({ dirty: true }), timelineFacts()).dirty).toBe(true);
  });
});

describe("undo and redo", () => {
  it("follow Rust's history rather than whether the document was touched", () => {
    // A document can be dirty with an empty undo stack: importing media does
    // not go through the history.
    const state = describeMenuState(
      documentFacts({ dirty: true, canUndo: false, canRedo: false }),
      timelineFacts(),
    );
    expect(state.can_undo).toBe(false);
    expect(state.can_redo).toBe(false);

    const afterEdit = describeMenuState(
      documentFacts({ dirty: true, canUndo: true, canRedo: false }),
      timelineFacts(),
    );
    expect(afterEdit.can_undo).toBe(true);
    expect(afterEdit.can_redo).toBe(false);
  });
});

describe("deleting a clip", () => {
  it("needs a selection", () => {
    expect(describeMenuState(documentFacts(), timelineFacts()).has_selection).toBe(false);
    expect(
      describeMenuState(documentFacts(), timelineFacts({ selection: ["clip-1"] })).has_selection,
    ).toBe(true);
  });

  it("needs the selected clip to still be in the document", () => {
    // The id outlives the clip when an undo removes it, and a Delete offered
    // for a clip that is no longer there is the exact "looks clickable, does
    // nothing" case.
    const state = describeMenuState(
      documentFacts(),
      timelineFacts({ selection: ["clip-that-was-deleted"] }),
    );
    expect(state.has_selection).toBe(false);
  });
});

describe("splitting a clip", () => {
  it("needs the playhead inside one", () => {
    expect(
      describeMenuState(documentFacts(), timelineFacts({ playhead: 2_000_000 })).can_split,
    ).toBe(true);
    expect(describeMenuState(documentFacts(), timelineFacts({ playhead: 500_000 })).can_split).toBe(
      false,
    );
  });

  it("is not offered on a clip's own edges, where the cut would produce nothing", () => {
    for (const playhead of [1_000_000, 3_000_000]) {
      expect(describeMenuState(documentFacts(), timelineFacts({ playhead })).can_split).toBe(false);
    }
  });

  it("does not depend on the selection: the playhead can be over another clip", () => {
    const state = describeMenuState(
      documentFacts(),
      timelineFacts({ playhead: 2_000_000, selection: [] }),
    );
    expect(state.has_selection).toBe(false);
    expect(state.can_split).toBe(true);
  });
});

describe("fitting the timeline", () => {
  it("is not offered for an empty one, where there is no span to fit", () => {
    const empty = projectWithSegments();
    expect(describeMenuState({ ...documentFacts(), project: empty }, timelineFacts()).can_fit).toBe(
      false,
    );
    expect(describeMenuState(documentFacts(), timelineFacts()).can_fit).toBe(true);
  });
});

describe("the state pushed to Rust", () => {
  it("is read from the stores and nowhere else", () => {
    useProjectStore.setState({ project: PROJECT, dirty: true, canUndo: true, canRedo: false });
    useTimelineStore.setState({ playhead: 2_000_000, selection: ["clip-1"] });

    expect(currentMenuState()).toEqual({
      has_project: true,
      dirty: true,
      can_undo: true,
      can_redo: false,
      has_selection: true,
      can_split: true,
      can_fit: true,
      has_clipboard: false,
      has_clips: true,
    });
  });

  it("compares by value, so a document replaced by an identical one is not resent", () => {
    const a = describeMenuState(documentFacts({ dirty: true }), timelineFacts());
    const b = describeMenuState(documentFacts({ dirty: true }), timelineFacts());
    expect(sameMenuState(a, b)).toBe(true);
    expect(sameMenuState(a, { ...a, dirty: false })).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Whether the app may close
// ---------------------------------------------------------------------------

describe("closing with nothing unsaved", () => {
  it("goes without asking", async () => {
    useProjectStore.setState({ project: PROJECT, path: "/home/me/cut.chukcut", dirty: false });
    ipc.handle("workspace_close_answer", null);

    await expect(answerCloseRequest()).resolves.toBe(true);
    expect(useWorkspaceStore.getState().discardPrompt).toBeNull();
    expect(ipc.lastCall("workspace_close_answer")).toEqual({ confirmed: true });
  });
});

describe("closing with unsaved changes", () => {
  beforeEach(() => {
    useProjectStore.setState({ project: PROJECT, path: "/home/me/cut.chukcut", dirty: true });
    ipc.handle("workspace_close_answer", null);
  });

  it("asks, and phrases the question as the thing being done", async () => {
    const decision = answerCloseRequest();
    await Promise.resolve();

    expect(useWorkspaceStore.getState().discardPrompt).toBe("closing chukcut");
    useWorkspaceStore.getState().answerDiscardPrompt("discard");
    await expect(decision).resolves.toBe(true);
    expect(ipc.lastCall("workspace_close_answer")).toEqual({ confirmed: true });
  });

  it("genuinely aborts on cancel — the window was never closed to begin with", async () => {
    const decision = answerCloseRequest();
    await Promise.resolve();
    useWorkspaceStore.getState().answerDiscardPrompt("cancel");

    await expect(decision).resolves.toBe(false);
    // Rust is holding the window open on this answer; `false` is what releases
    // it back to being an ordinary open window.
    expect(ipc.lastCall("workspace_close_answer")).toEqual({ confirmed: false });
  });

  it("saves first, and closes once the write has landed", async () => {
    ipc.handle("project_save", "/home/me/cut.chukcut");
    ipc.handle("workspace_recent_record", null);
    ipc.handle("workspace_recent_list", []);

    const decision = answerCloseRequest();
    await Promise.resolve();
    useWorkspaceStore.getState().answerDiscardPrompt("save");

    await expect(decision).resolves.toBe(true);
    expect(ipc.count("project_save")).toBe(1);
    expect(ipc.lastCall("workspace_close_answer")).toEqual({ confirmed: true });
  });

  it("stays open when the save it was told to do fails", async () => {
    ipc.fail("project_save", "the disk is full");

    const decision = answerCloseRequest();
    await Promise.resolve();
    useWorkspaceStore.getState().answerDiscardPrompt("save");

    // The work is still only in memory. Quitting here is the bug the prompt
    // exists to prevent.
    await expect(decision).resolves.toBe(false);
    expect(ipc.lastCall("workspace_close_answer")).toEqual({ confirmed: false });
    expect(useProjectStore.getState().error).toBe("the disk is full");
  });
});

// ---------------------------------------------------------------------------
// Acting on a click
// ---------------------------------------------------------------------------

const HANDLERS = {
  newProject: () => {},
  openProject: () => {},
  showExport: () => {},
  showShortcuts: () => {},
};

describe("an enabled item", () => {
  beforeEach(() => {
    useProjectStore.setState({ project: PROJECT, path: "/home/me/cut.chukcut", dirty: true });
  });

  it("saves through the same path as Ctrl+S", async () => {
    ipc.handle("project_save", "/home/me/cut.chukcut");
    ipc.handle("workspace_recent_record", null);
    ipc.handle("workspace_recent_list", []);

    runMenuAction(MENU_IDS.save, HANDLERS);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(ipc.lastCall("project_save")).toEqual({ path: "/home/me/cut.chukcut" });
    expect(useProjectStore.getState().dirty).toBe(false);
  });

  it("undoes through the same command the toolbar button uses", async () => {
    ipc.handle("timeline_undo", makeEditResponse(PROJECT, { can_undo: false }));

    runMenuAction(MENU_IDS.undo, HANDLERS);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(ipc.count("timeline_undo")).toBe(1);
  });

  it("splits at the playhead, on the clip the razor would have cut", async () => {
    useTimelineStore.setState({ playhead: 2_000_000, selection: [] });
    ipc.handle("timeline_split", makeEditResponse(PROJECT));

    runMenuAction(MENU_IDS.split, HANDLERS);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(ipc.lastCall("timeline_split")).toEqual({ segmentId: "clip-1", at: 2_000_000 });
  });

  it("deletes the selected clip and clears the selection with it", async () => {
    useTimelineStore.setState({ selection: ["clip-1"] });
    ipc.handle("timeline_apply_many", makeEditResponse(projectWithSegments()));

    runMenuAction(MENU_IDS.delete, HANDLERS);
    await new Promise((resolve) => setTimeout(resolve, 0));

    // One clip, so one command and no batch wrapper — a selection of one has
    // to stay on exactly the path it was always on.
    expect(ipc.lastCall("timeline_apply_many")?.commands).toMatchObject([
      { type: "remove_segment", track_id: "track-video" },
    ]);
    expect(useTimelineStore.getState().selection).toEqual([]);
  });

  it("copies the selection onto the clipboard without touching the document", () => {
    useTimelineStore.setState({ selection: ["clip-1"] });

    runMenuAction(MENU_IDS.copy, HANDLERS);

    expect(useTimelineStore.getState().clipboard).toHaveLength(1);
    // Copying is not an edit: nothing crosses the boundary and nothing lands on
    // the undo stack.
    expect(ipc.log).toEqual([]);
  });

  it("cuts by copying and then deleting, in one undo step", async () => {
    useTimelineStore.setState({ selection: ["clip-1"] });
    ipc.handle("timeline_apply_many", makeEditResponse(projectWithSegments()));

    runMenuAction(MENU_IDS.cut, HANDLERS);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(useTimelineStore.getState().clipboard).toHaveLength(1);
    expect(ipc.count("timeline_apply_many")).toBe(1);
    expect(useTimelineStore.getState().selection).toEqual([]);
  });

  it("pastes at the playhead and selects what landed", async () => {
    useTimelineStore.setState({ selection: ["clip-1"] });
    runMenuAction(MENU_IDS.copy, HANDLERS);
    useTimelineStore.setState({ playhead: 8_000_000 });
    ipc.handle("timeline_apply", makeEditResponse(PROJECT));

    runMenuAction(MENU_IDS.paste, HANDLERS);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(ipc.lastCall("timeline_apply")?.command).toMatchObject({
      type: "insert_segment",
      segment: { target_range: { start: 8_000_000, duration: 2_000_000 } },
    });
    // Finding what you pasted is half the feature.
    expect(useTimelineStore.getState().selection).toHaveLength(1);
    expect(useTimelineStore.getState().selection[0]).not.toBe("clip-1");
  });

  it("selects every clip on every unlocked lane", () => {
    runMenuAction(MENU_IDS.selectAll, HANDLERS);
    expect(useTimelineStore.getState().selection).toEqual(["clip-1"]);
  });

  it("zooms the timeline by the toolbar's own step", () => {
    const before = useTimelineStore.getState().zoom;

    runMenuAction(MENU_IDS.zoomIn, HANDLERS);
    expect(useTimelineStore.getState().zoom).toBeCloseTo(before * 1.6, 12);

    runMenuAction(MENU_IDS.zoomOut, HANDLERS);
    expect(useTimelineStore.getState().zoom).toBeCloseTo(before, 12);
  });

  it("hands the dialog items to whoever mounted them", () => {
    const opened: string[] = [];
    const handlers = {
      newProject: () => opened.push("new"),
      openProject: () => opened.push("open"),
      showExport: () => opened.push("export"),
      showShortcuts: () => opened.push("shortcuts"),
    };

    runMenuAction(MENU_IDS.newProject, handlers);
    runMenuAction(MENU_IDS.openProject, handlers);
    runMenuAction(MENU_IDS.export, handlers);
    runMenuAction(MENU_IDS.shortcuts, handlers);

    expect(opened).toEqual(["new", "open", "export", "shortcuts"]);
  });
});

describe("an item whose state has moved on since the menu opened", () => {
  it("does nothing rather than sending an edit Rust would reject", async () => {
    useProjectStore.setState({ project: PROJECT, dirty: true });
    // Grey when the bar was last synced, and the playhead has since left the
    // clip. Nothing should cross the boundary.
    useTimelineStore.setState({ playhead: 500_000, selection: [] });

    runMenuAction(MENU_IDS.split, HANDLERS);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(ipc.count("timeline_split")).toBe(0);
    expect(ipc.unhandled).toEqual([]);
  });
});

describe("an item Rust owns", () => {
  it("goes back over the boundary rather than being guessed at here", async () => {
    // Quitting, and three things that open something outside the app. None of
    // them is possible from a webview, and none of them is in `runMenuAction`'s
    // switch — a branch here would be a second, silently different answer to
    // "who runs this".
    ipc.handle("workspace_menu_run", null);

    for (const id of RUST_OWNED_IDS) {
      runMenuAction(id, HANDLERS);
    }
    await settle();

    expect(ipc.calls("workspace_menu_run")).toEqual(RUST_OWNED_IDS.map((id) => ({ id })));
  });

  it("is exactly the four the About box, the logs, the docs and Quit need", () => {
    // `menu::is_ours` on the Rust side is the same list and has its own test.
    // An id in neither list is an item that does nothing at all.
    expect([...RUST_OWNED_IDS].sort()).toEqual(
      [MENU_IDS.quit, MENU_IDS.logs, MENU_IDS.docs, MENU_IDS.about].sort(),
    );
  });
});

// ---------------------------------------------------------------------------
// The keys the bar used to own alone
// ---------------------------------------------------------------------------

describe("the accelerators nothing else in the app binds", () => {
  const press = (key: string, modifiers: Partial<KeyboardEventInit> = {}) =>
    acceleratorAction(new KeyboardEvent("keydown", { key, ctrlKey: true, ...modifiers }));

  it("routes the five the native menu used to swallow", () => {
    expect(press("i")).toBe(MENU_IDS.importMedia);
    expect(press("q")).toBe(MENU_IDS.quit);
    expect(press("=")).toBe(MENU_IDS.zoomIn);
    expect(press("-")).toBe(MENU_IDS.zoomOut);
    expect(press("0")).toBe(MENU_IDS.zoomFit);
    // `+` is what a shifted `=` reports, and it is what people press for
    // "zoom in" without thinking about it.
    expect(press("+", { shiftKey: true })).toBe(MENU_IDS.zoomIn);
  });

  it("claims nothing that another handler already binds", () => {
    // Every one of these has an owner next to the thing it acts on — the
    // timeline binds the clipboard four and undo, the preview binds F, App
    // binds save and export. Two handlers claiming a key both fire.
    for (const key of ["c", "x", "v", "d", "a", "z", "y", "s", "n", "o", "e"]) {
      expect(press(key)).toBeNull();
    }
  });

  it("ignores a bare key, and lets Alt and Ctrl+Shift+I past", () => {
    expect(acceleratorAction(new KeyboardEvent("keydown", { key: "i" }))).toBeNull();
    expect(press("i", { altKey: true })).toBeNull();
    // Ctrl+Shift+I is the inspector on every desktop webview.
    expect(press("i", { shiftKey: true })).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Keeping the drawn bar in step
// ---------------------------------------------------------------------------

describe("the bar Rust hands back", () => {
  const BAR = [
    {
      title: "File",
      entries: [
        {
          kind: "item" as const,
          id: MENU_IDS.save,
          label: "Save",
          accelerator: "Ctrl+S",
          enabled: true,
          unavailable_reason: null,
        },
      ],
    },
  ];

  it("is asked for from the current state and given to whoever draws it", async () => {
    ipc.handle("workspace_menu_describe", BAR);
    const drawn: unknown[] = [];

    const stop = installMenu((sections) => drawn.push(sections));
    await settle();
    stop();

    expect(ipc.lastCall("workspace_menu_describe")).toEqual({ state: currentMenuState() });
    expect(drawn).toEqual([BAR]);
  });

  it("is asked for again when the document changes, and not when it has not", async () => {
    ipc.handle("workspace_menu_describe", BAR);
    const stop = installMenu(() => {});
    await settle();
    expect(ipc.count("workspace_menu_describe")).toBe(1);

    // A pointer move over the lanes writes the playhead many times a second and
    // none of it changes a single item. Without the comparison this would be an
    // IPC round trip per frame.
    useTimelineStore.setState({ playhead: 10 });
    useTimelineStore.setState({ playhead: 20 });
    await settle();
    expect(ipc.count("workspace_menu_describe")).toBe(1);

    useProjectStore.setState({ project: PROJECT, dirty: true });
    await settle();
    expect(ipc.count("workspace_menu_describe")).toBe(2);

    stop();
  });

  it("keeps the last bar it drew when the round trip fails", async () => {
    ipc.handle("workspace_menu_describe", BAR);
    const drawn: unknown[] = [];
    const stop = installMenu((sections) => drawn.push(sections));
    await settle();

    ipc.fail("workspace_menu_describe", "the engine is busy");
    useProjectStore.setState({ project: PROJECT, dirty: true });
    await settle();

    // An empty menu bar looks like the app has lost its menus. A slightly stale
    // one still opens, and every item in it also has a button or a key.
    expect(drawn).toEqual([BAR]);

    // And the failed state is not remembered as drawn, so the next change asks
    // again rather than deduplicating against a bar that was never shown.
    ipc.handle("workspace_menu_describe", BAR);
    useProjectStore.setState({ dirty: false });
    await settle();
    expect(drawn).toEqual([BAR, BAR]);

    stop();
  });

  it("stops asking once it is torn down", async () => {
    ipc.handle("workspace_menu_describe", BAR);
    const stop = installMenu(() => {});
    await settle();
    stop();

    useProjectStore.setState({ project: PROJECT, dirty: true });
    await settle();

    expect(ipc.count("workspace_menu_describe")).toBe(1);
  });
});

/** Let every promise the call chain queued resolve. */
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}
