/**
 * What a multi-clip edit sends across the boundary.
 *
 * The shape matters more than it looks. A selection is one gesture and has to
 * be one undo step, which means one call carrying one command per clip — not a
 * call per clip, and not a `composite` this side built, because Rust will not
 * expand the link partners of a composite (see `compose_edits` in
 * `timeline/ops.rs`). These assertions are on the wire payload for that reason:
 * a refactor that quietly went back to one call per clip would leave the app
 * working and the undo stack wrong.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useProjectStore } from "@/modules/project/store";
import {
  copySegments,
  deleteSegments,
  duplicateSegments,
  moveSegments,
  pasteEntries,
  trimSegments,
} from "@/modules/timeline/lib/edits";
import { makeEditResponse, makeProject, makeSegment, makeTrack, range } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

const SECOND = 1_000_000;

const PROJECT = makeProject({
  // A pool that actually holds the clips' material: a paste checks, because a
  // clip whose file this document has never heard of is what the cross-project
  // case has to handle rather than paste broken.
  materials: {
    videos: ["a", "b", "c"].map((id) => ({
      id: `material-${id}`,
      path: `/media/${id}.mp4`,
      width: 1920,
      height: 1080,
      duration: 10 * SECOND,
      fps: 30,
      has_audio: false,
      rotation: 0,
    })),
    audios: [],
    images: [],
    texts: [],
    links: [],
    transitions: [],
    extras: {},
  },
  tracks: [
    makeTrack("video-1", {
      segments: [
        makeSegment("a", { target_range: range(0, SECOND) }),
        makeSegment("b", { target_range: range(SECOND, SECOND) }),
        makeSegment("c", { target_range: range(2 * SECOND, SECOND) }),
      ],
    }),
    makeTrack("audio-1", { kind: "audio", name: "Audio 1" }),
  ],
});

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  ipc.handle("timeline_apply_many", makeEditResponse(PROJECT));
  ipc.handle("timeline_apply", makeEditResponse(PROJECT));
});

afterEach(() => {
  ipc.restore();
  useProjectStore.setState({ project: null, error: null });
});

describe("deleting a selection", () => {
  it("is one call carrying one removal per clip", async () => {
    await deleteSegments(PROJECT, ["a", "c"]);

    expect(ipc.count("timeline_apply_many")).toBe(1);
    const payload = ipc.lastCall("timeline_apply_many");
    expect(payload?.label).toBe("Delete clips");
    expect(payload?.commands).toMatchObject([
      { type: "remove_segment", track_id: "video-1", segment: { id: "a" } },
      { type: "remove_segment", track_id: "video-1", segment: { id: "c" } },
    ]);
  });

  it("carries the whole segment and its index, so one undo puts it all back", async () => {
    await deleteSegments(PROJECT, ["b"]);
    const [command] = (ipc.lastCall("timeline_apply_many")?.commands ?? []) as {
      index: number;
      segment: { id: string; source_range: unknown };
    }[];
    expect(command.index).toBe(1);
    expect(command.segment.source_range).toEqual({ start: 0, duration: SECOND });
  });

  it("skips ids the document has already lost rather than failing the batch", async () => {
    // An undo takes a clip away and leaves its id in the selection. The rest of
    // the selection is still there to delete.
    await deleteSegments(PROJECT, ["a", "gone"]);
    expect(ipc.lastCall("timeline_apply_many")?.commands).toHaveLength(1);
  });

  it("sends nothing at all when none of the ids are real", async () => {
    await expect(deleteSegments(PROJECT, ["gone"])).resolves.toBe(false);
    expect(ipc.count("timeline_apply_many")).toBe(0);
  });
});

describe("moving and trimming a selection", () => {
  it("sends one move per clip, and nothing for a link partner", async () => {
    // The partners are Rust's to expand — sending them here as well would move
    // them twice. What crosses is exactly what the user dragged.
    await moveSegments([
      {
        segmentId: "a",
        fromTrackId: "video-1",
        toTrackId: "video-1",
        fromStart: 0,
        toStart: 5 * SECOND,
      },
      {
        segmentId: "b",
        fromTrackId: "video-1",
        toTrackId: "video-1",
        fromStart: SECOND,
        toStart: 6 * SECOND,
      },
    ]);

    expect(ipc.lastCall("timeline_apply_many")?.label).toBe("Move clips");
    expect(ipc.lastCall("timeline_apply_many")?.commands).toEqual([
      {
        type: "move_segment",
        segment_id: "a",
        from_track: "video-1",
        to_track: "video-1",
        from_start: 0,
        to_start: 5 * SECOND,
      },
      {
        type: "move_segment",
        segment_id: "b",
        from_track: "video-1",
        to_track: "video-1",
        from_start: SECOND,
        to_start: 6 * SECOND,
      },
    ]);
  });

  it("names one clip in the singular, so the undo menu reads honestly", async () => {
    await moveSegments([
      {
        segmentId: "a",
        fromTrackId: "video-1",
        toTrackId: "video-1",
        fromStart: 0,
        toStart: SECOND,
      },
    ]);
    expect(ipc.lastCall("timeline_apply_many")?.label).toBe("Move clip");
  });

  it("sends both ranges on both sides of every trim", async () => {
    await trimSegments([
      {
        segmentId: "a",
        beforeTarget: range(0, SECOND),
        beforeSource: range(0, SECOND),
        afterTarget: range(0, SECOND / 2),
        afterSource: range(0, SECOND / 2),
      },
    ]);
    expect(ipc.lastCall("timeline_apply_many")?.commands).toEqual([
      {
        type: "trim_segment",
        segment_id: "a",
        before_target: { start: 0, duration: SECOND },
        before_source: { start: 0, duration: SECOND },
        after_target: { start: 0, duration: SECOND / 2 },
        after_source: { start: 0, duration: SECOND / 2 },
      },
    ]);
  });
});

describe("cut, copy and paste", () => {
  it("pastes a batch as one composite, so it is one undo step", async () => {
    const clipboard = copySegments(PROJECT, ["a", "b"]);
    const ids = await pasteEntries(PROJECT, clipboard, 10 * SECOND);

    expect(ids).toHaveLength(2);
    const command = ipc.lastCall("timeline_apply")?.command as {
      type: string;
      commands: { type: string }[];
    };
    expect(command.type).toBe("composite");
    expect(command.commands.every((part) => part.type === "insert_segment")).toBe(true);
    expect(ipc.count("timeline_apply")).toBe(1);
  });

  it("mints new ids rather than pasting the ones it copied", async () => {
    const clipboard = copySegments(PROJECT, ["a"]);
    const ids = await pasteEntries(PROJECT, clipboard, 10 * SECOND);
    // Two clips with one id makes every later edit aimed at one of them land on
    // the other; the document validator calls it an error.
    expect(ids[0]).not.toBe("a");
    expect(ipc.lastCall("timeline_apply")?.command).toMatchObject({
      type: "insert_segment",
      segment: { id: ids[0] },
    });
  });

  it("survives the project it was copied from being closed, and brings the file over", async () => {
    const clipboard = copySegments(PROJECT, ["a"]);
    useProjectStore.setState({ project: null });

    // A different document, which has never heard of `material-a`. A material
    // id means something inside one project and nothing in the next, so the
    // paste has to import the *file* and take the id this project gives it —
    // otherwise the pasted clip references a material that is not there, which
    // `validate()` calls an error and the renderer has nothing to draw for.
    const elsewhere = makeProject({ tracks: [makeTrack("video-9")] });
    const adopted = makeProject({
      materials: {
        videos: [
          {
            id: "material-here",
            path: "/media/a.mp4",
            width: 1920,
            height: 1080,
            duration: 10 * SECOND,
            fps: 30,
            has_audio: false,
            rotation: 0,
          },
        ],
        audios: [],
        images: [],
        texts: [],
        links: [],
        transitions: [],
        extras: {},
      },
      tracks: [makeTrack("video-9")],
    });
    ipc.handle("project_import_media", {
      id: "material-here",
      kind: "video",
      name: "a.mp4",
      path: "/media/a.mp4",
      duration: 10 * SECOND,
      width: 1920,
      height: 1080,
      has_audio: false,
    });
    ipc.handle("project_get", adopted);

    const ids = await pasteEntries(elsewhere, clipboard, 0);

    expect(ipc.lastCall("project_import_media")).toEqual({ path: "/media/a.mp4" });
    expect(ids).toHaveLength(1);
    expect(ipc.lastCall("timeline_apply")).toMatchObject({
      command: {
        type: "insert_segment",
        track_id: "video-9",
        segment: { material_id: "material-here" },
      },
    });
  });

  it("imports nothing when the material is already in the pool", async () => {
    // The ordinary case — copy and paste inside one project — must not cost an
    // IPC round trip per clip.
    await pasteEntries(PROJECT, copySegments(PROJECT, ["a", "b"]), 10 * SECOND);
    expect(ipc.count("project_import_media")).toBe(0);
  });

  it("reports nothing pasted when the edit is refused", async () => {
    ipc.fail("timeline_apply", "target range is occupied");
    const ids = await pasteEntries(PROJECT, copySegments(PROJECT, ["a"]), 0);
    expect(ids).toEqual([]);
    expect(useProjectStore.getState().error).toBe("target range is occupied");
  });

  it("duplicates a selection as one edit", async () => {
    await duplicateSegments(PROJECT, ["a", "b"]);
    const command = ipc.lastCall("timeline_apply")?.command as {
      type: string;
      label: string;
      commands: unknown[];
    };
    expect(command.type).toBe("composite");
    expect(command.label).toBe("Duplicate");
    expect(command.commands).toHaveLength(2);
  });
});
