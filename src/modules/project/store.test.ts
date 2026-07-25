import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { describeError, runEdit, useProjectStore } from "@/modules/project/store";
import { findSegment } from "@/modules/project/types";
import { timelineApply, timelineUndo } from "@/modules/timeline/lib/api";
import { makeEditResponse, makeProject, makeSegment, projectWithSegments } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  useProjectStore.setState({
    project: null,
    path: null,
    status: "loading",
    error: null,
    dirty: false,
    canUndo: false,
    canRedo: false,
    undoLabel: null,
    redoLabel: null,
  });
});

afterEach(() => {
  ipc.restore();
});

describe("the document is replaced, never merged", () => {
  it("drops a segment that Rust's answer no longer contains", () => {
    const before = projectWithSegments(makeSegment("a"), makeSegment("b"));
    useProjectStore.getState().loadProject(before);

    // Rust deleted "a". A store that merged would still be holding it.
    const after = projectWithSegments(makeSegment("b"));
    useProjectStore.getState().applyEditResponse(makeEditResponse(after));

    const project = useProjectStore.getState().project;
    expect(project).not.toBeNull();
    expect(findSegment(project!, "a")).toBeNull();
    expect(findSegment(project!, "b")).not.toBeNull();
    expect(project!.tracks[0].segments).toHaveLength(1);
  });

  it("drops a whole track that Rust's answer no longer contains", () => {
    useProjectStore.getState().loadProject(projectWithSegments(makeSegment("a")));
    useProjectStore.getState().applyEditResponse(makeEditResponse(makeProject()));

    expect(useProjectStore.getState().project?.tracks).toEqual([]);
  });

  it("takes Rust's field value even when the local one is newer", () => {
    const local = makeProject({ name: "renamed in the UI", updated_at: 9_999_999 });
    useProjectStore.getState().loadProject(local);

    useProjectStore
      .getState()
      .applyEditResponse(makeEditResponse(makeProject({ name: "Untitled" })));

    expect(useProjectStore.getState().project?.name).toBe("Untitled");
    expect(useProjectStore.getState().project?.updated_at).toBe(1_700_000_000_000);
  });

  it("stores the response's document by reference, so identity is a usable change signal", () => {
    // The preview watcher restarts on `state.project !== previous.project`.
    // Two documents that happen to be deep-equal still have to look like a
    // change, or an edit that reverts a value would leave a stale preview.
    const first = makeProject();
    const second = makeProject();
    useProjectStore.getState().loadProject(first);
    useProjectStore.getState().applyEditResponse(makeEditResponse(second));

    expect(useProjectStore.getState().project).toBe(second);
    expect(useProjectStore.getState().project).not.toBe(first);
    expect(second).toEqual(first);
  });

  it("carries Rust's history state across with the document", () => {
    useProjectStore.getState().loadProject(makeProject());
    expect(useProjectStore.getState().canUndo).toBe(false);

    useProjectStore.getState().applyEditResponse({
      project: makeProject(),
      can_undo: true,
      can_redo: true,
      undo_label: "Trim clip",
      redo_label: "Split clip",
    });

    const state = useProjectStore.getState();
    expect(state.canUndo).toBe(true);
    expect(state.canRedo).toBe(true);
    expect(state.undoLabel).toBe("Trim clip");
    expect(state.redoLabel).toBe("Split clip");
    expect(state.dirty).toBe(true);
  });

  it("resets the history when a project is opened, because the new document has none", () => {
    useProjectStore.getState().applyEditResponse(makeEditResponse(makeProject()));
    useProjectStore.getState().loadProject(makeProject(), "/tmp/a.chukcut");

    const state = useProjectStore.getState();
    expect(state.canUndo).toBe(false);
    expect(state.canRedo).toBe(false);
    expect(state.undoLabel).toBeNull();
    expect(state.dirty).toBe(false);
    expect(state.path).toBe("/tmp/a.chukcut");
  });

  it("leaves the history alone when only the material pool was refreshed", () => {
    // Importing media is not undoable, so a refresh must not claim the undo
    // stack moved.
    useProjectStore.getState().applyEditResponse(
      makeEditResponse(makeProject(), {
        can_undo: true,
        undo_label: "Insert clip",
      }),
    );

    const refreshed = makeProject({ name: "with new media" });
    useProjectStore.getState().refreshDocument(refreshed);

    const state = useProjectStore.getState();
    expect(state.project).toBe(refreshed);
    expect(state.canUndo).toBe(true);
    expect(state.undoLabel).toBe("Insert clip");
  });
});

describe("runEdit", () => {
  it("replaces the document with the one the command answered with", async () => {
    const before = projectWithSegments(makeSegment("a"), makeSegment("b"));
    useProjectStore.getState().loadProject(before);

    const after = projectWithSegments(makeSegment("b"));
    ipc.handle("timeline_apply", makeEditResponse(after));

    const ok = await runEdit(() =>
      timelineApply({
        type: "remove_segment",
        track_id: "track-video",
        segment: makeSegment("a"),
        index: 0,
      }),
    );

    expect(ok).toBe(true);
    expect(useProjectStore.getState().project).toBe(after);
    expect(useProjectStore.getState().error).toBeNull();
  });

  it("keeps the document untouched when Rust rejects the edit", async () => {
    const before = projectWithSegments(makeSegment("a"));
    useProjectStore.getState().loadProject(before);
    ipc.fail("timeline_apply", "target range is occupied");

    const ok = await runEdit(() =>
      timelineApply({
        type: "remove_segment",
        track_id: "track-video",
        segment: makeSegment("a"),
        index: 0,
      }),
    );

    expect(ok).toBe(false);
    // A rejected drag must leave the clip exactly where it was.
    expect(useProjectStore.getState().project).toBe(before);
    expect(useProjectStore.getState().error).toBe("target range is occupied");
  });

  it("shows Rust's sentence verbatim rather than an exception's toString", async () => {
    useProjectStore.getState().loadProject(makeProject());
    ipc.fail("timeline_undo", "cannot read /media/a.mp4: no such file");

    await runEdit(timelineUndo);

    expect(useProjectStore.getState().error).toBe("cannot read /media/a.mp4: no such file");
  });

  it("clears a previous error once an edit succeeds", async () => {
    useProjectStore.getState().loadProject(makeProject());
    useProjectStore.getState().setError("something went wrong earlier");
    ipc.handle("timeline_undo", makeEditResponse(makeProject()));

    await runEdit(timelineUndo);

    expect(useProjectStore.getState().error).toBeNull();
  });
});

describe("describeError", () => {
  it("passes a Rust string through untouched", () => {
    expect(describeError("target range is occupied")).toBe("target range is occupied");
  });

  it("unwraps an Error rather than printing [object Object]", () => {
    expect(describeError(new Error("boom"))).toBe("boom");
  });

  it("still says something for a value that is neither", () => {
    expect(describeError({ code: 7 })).toBe("[object Object]");
    expect(describeError(null)).toBe("null");
  });
});
