/**
 * The wire shapes, pinned.
 *
 * The boundary is deliberately inconsistent and the inconsistency is documented
 * rather than fixed: the project document and `EditCommand` keep Rust's
 * snake_case, while the preview structs carry `#[serde(rename_all =
 * "camelCase")]`. That has already cost one bug, so both conventions are nailed
 * down here — as literals that the TypeScript types must accept (a rename on
 * this side fails `tsc`) and as assertions on the bytes that actually cross
 * (a rename on Rust's side fails these tests the next time the fixtures are
 * refreshed against it).
 *
 * The samples below are hand-written to match serde's output, not derived from
 * the app's own builders. A fixture built by the code under test could not
 * catch the code under test being wrong.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { mediaThumbnails } from "@/modules/media/lib/api";
import {
  newPreviewChannel,
  type PreviewEvent,
  type PreviewInfo,
  previewSeek,
  previewStart,
} from "@/modules/preview/lib/api";
import { usePreviewStore } from "@/modules/preview/store";
import { projectImportMedia } from "@/modules/project/lib/api";
import { useProjectStore } from "@/modules/project/store";
import type { Project } from "@/modules/project/types";
import {
  type EditCommand,
  type EditResponse,
  timelineApply,
  timelineSplit,
} from "@/modules/timeline/lib/api";
import { IDENTITY_TRANSFORM, makePreviewInfo, makeSegment, makeTrack } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
});

afterEach(() => {
  ipc.restore();
});

// ---------------------------------------------------------------------------
// PreviewEvent — internally tagged, camelCase
// ---------------------------------------------------------------------------

/** Exactly what `serde` emits for `PreviewEvent`, one sample per variant. */
const PREVIEW_EVENTS = {
  ready: {
    type: "ready",
    session: 4,
    width: 540,
    height: 960,
    fps: 29.97,
    duration: 12_345_678,
  },
  position: { type: "position", session: 4, frame: 91, time: 3_036_303, playing: true },
  ended: { type: "ended", session: 4 },
  error: { type: "error", message: "decoder gave up on clip 3" },
} satisfies Record<string, PreviewEvent>;

describe("PreviewEvent is internally tagged", () => {
  it("carries the variant in a `type` field, not as an outer wrapper key", () => {
    for (const [name, event] of Object.entries(PREVIEW_EVENTS)) {
      expect(event.type).toBe(name);
      // Externally tagged would be `{ Position: { … } }` — the shape a plain
      // `#[derive(Serialize)]` on the enum would produce.
      expect(Object.keys(event)).not.toContain("Position");
      expect(Object.keys(event)).not.toContain("Ready");
    }
  });

  it("names its fields in camelCase, like the rest of the preview module", () => {
    for (const event of Object.values(PREVIEW_EVENTS)) {
      for (const key of Object.keys(event)) {
        expect(key).not.toContain("_");
      }
    }
  });

  it("arrives through a Channel unchanged, in the order Rust sent it", async () => {
    const received: PreviewEvent[] = [];
    const channel = newPreviewChannel();
    channel.onmessage = (event) => received.push(event);

    ipc.handle("preview_start", makePreviewInfo());
    await previewStart(channel);

    const wire = ipc.channel("preview_start");
    wire.emit(PREVIEW_EVENTS.ready);
    wire.emit(PREVIEW_EVENTS.position);
    wire.emit(PREVIEW_EVENTS.ended);

    expect(received).toEqual([PREVIEW_EVENTS.ready, PREVIEW_EVENTS.position, PREVIEW_EVENTS.ended]);
  });

  it("is held back and replayed in order when messages arrive out of order", async () => {
    const received: PreviewEvent[] = [];
    const channel = newPreviewChannel();
    channel.onmessage = (event) => received.push(event);

    ipc.handle("preview_start", makePreviewInfo());
    await previewStart(channel);

    const wire = ipc.channel("preview_start");
    wire.emitAt(1, PREVIEW_EVENTS.ended);
    expect(received).toEqual([]);
    wire.emitAt(0, PREVIEW_EVENTS.position);

    expect(received).toEqual([PREVIEW_EVENTS.position, PREVIEW_EVENTS.ended]);
  });
});

// ---------------------------------------------------------------------------
// PreviewInfo — camelCase
// ---------------------------------------------------------------------------

/** Exactly what `preview_start` answers with. */
const PREVIEW_INFO = {
  session: 4,
  width: 540,
  height: 960,
  quality: 80,
  fps: 29.97,
  duration: 12_345_678,
  position: 0,
  frame: 0,
  playing: false,
  frameUrl: "chukcut-frame://preview/4",
} satisfies PreviewInfo;

describe("PreviewInfo is camelCase", () => {
  it("spells the frame base URL `frameUrl`", () => {
    expect(Object.keys(PREVIEW_INFO)).toContain("frameUrl");
    expect(Object.keys(PREVIEW_INFO)).not.toContain("frame_url");
  });

  it("reaches the player store intact", async () => {
    ipc.handle("preview_start", PREVIEW_INFO);
    usePreviewStore.getState().applyInfo(await previewStart(newPreviewChannel()));

    const state = usePreviewStore.getState();
    expect(state.session).toBe(4);
    expect(state.frameUrl).toBe("chukcut-frame://preview/4");
    expect(state.fps).toBe(29.97);
    expect(state.duration).toBe(12_345_678);
  });
});

describe("the preview commands", () => {
  it("sends `preview_start` a channel, a time and an options slot", async () => {
    ipc.handle("preview_start", PREVIEW_INFO);
    await previewStart(newPreviewChannel(), 250_000);

    const payload = ipc.lastCall("preview_start");
    expect(Object.keys(payload ?? {}).sort()).toEqual(["onEvent", "options", "time"]);
    expect(payload?.time).toBe(250_000);
    expect(payload?.options).toBeNull();
    // A `Channel` crosses as its marker, which is how Rust finds the callback.
    expect(payload?.onEvent).toMatch(/^__CHANNEL__:\d+$/);
  });

  it("sends explicit nulls rather than omitting the optional arguments", async () => {
    // Tauri drops `undefined` on the way out, and a missing field is a
    // deserialization error against a non-Option parameter.
    ipc.handle("preview_start", PREVIEW_INFO);
    await previewStart(newPreviewChannel());

    expect(ipc.lastCall("preview_start")).toMatchObject({ time: null, options: null });
  });

  it("sends `preview_seek` a bare time", async () => {
    ipc.handle("preview_seek", PREVIEW_INFO);
    await previewSeek(750_000);

    expect(ipc.lastCall("preview_seek")).toEqual({ time: 750_000 });
  });
});

// ---------------------------------------------------------------------------
// EditCommand — internally tagged, snake_case
// ---------------------------------------------------------------------------

const SEGMENT = makeSegment("segment-1", { target_range: { start: 0, duration: 2_000_000 } });

const EDIT_COMMANDS: EditCommand[] = [
  { type: "add_track", track: makeTrack("track-1"), index: 0 },
  { type: "insert_segment", track_id: "track-1", segment: SEGMENT, index: 0 },
  { type: "remove_segment", track_id: "track-1", segment: SEGMENT, index: 0 },
  {
    type: "move_segment",
    segment_id: "segment-1",
    from_track: "track-1",
    to_track: "track-2",
    from_start: 0,
    to_start: 500_000,
  },
  {
    type: "trim_segment",
    segment_id: "segment-1",
    before_target: { start: 0, duration: 2_000_000 },
    before_source: { start: 0, duration: 2_000_000 },
    after_target: { start: 0, duration: 1_000_000 },
    after_source: { start: 0, duration: 1_000_000 },
  },
  {
    type: "set_transform",
    segment_id: "segment-1",
    before: IDENTITY_TRANSFORM,
    after: { ...IDENTITY_TRANSFORM, opacity: 0.5 },
  },
  { type: "set_speed", segment_id: "segment-1", before: 1, after: 2 },
  { type: "set_volume", segment_id: "segment-1", before: 1, after: 0.25 },
  {
    type: "set_track_flags",
    track_id: "track-1",
    before: { muted: false, locked: false, hidden: false, volume: 1 },
    after: { muted: true, locked: false, hidden: false, volume: 1 },
  },
];

function keysDeep(value: unknown, into: string[] = []): string[] {
  if (Array.isArray(value)) {
    for (const item of value) keysDeep(item, into);
  } else if (value !== null && typeof value === "object") {
    for (const [key, child] of Object.entries(value)) {
      into.push(key);
      keysDeep(child, into);
    }
  }
  return into;
}

describe("EditCommand is internally tagged and stays snake_case", () => {
  it("puts the variant name in `type`, in snake_case", async () => {
    ipc.handle("timeline_apply", { project: {}, can_undo: true, can_redo: false });
    for (const command of EDIT_COMMANDS) {
      await timelineApply(command);
      expect(ipc.lastCall("timeline_apply")?.command).toMatchObject({ type: command.type });
      expect(command.type).toMatch(/^[a-z_]+$/);
    }
  });

  it("never camelCases a field on the way out", async () => {
    ipc.handle("timeline_apply", { project: {}, can_undo: true, can_redo: false });

    for (const command of EDIT_COMMANDS) {
      await timelineApply(command);
      const wire = ipc.lastCall("timeline_apply")?.command;
      for (const key of keysDeep(wire)) {
        expect(key, `"${key}" in ${command.type} must not be camelCase`).toBe(key.toLowerCase());
      }
    }
  });

  it("wraps the command in a single `command` argument", async () => {
    ipc.handle("timeline_apply", { project: {}, can_undo: true, can_redo: false });
    await timelineApply(EDIT_COMMANDS[0]);

    expect(Object.keys(ipc.lastCall("timeline_apply") ?? {})).toEqual(["command"]);
  });

  it("carries both sides of the change, so Rust can invert it", async () => {
    ipc.handle("timeline_apply", { project: {}, can_undo: true, can_redo: false });
    await timelineApply({
      type: "set_transform",
      segment_id: "segment-1",
      before: IDENTITY_TRANSFORM,
      after: { ...IDENTITY_TRANSFORM, opacity: 0.5 },
    });

    expect(ipc.lastCall("timeline_apply")?.command).toEqual({
      type: "set_transform",
      segment_id: "segment-1",
      before: {
        position: [0, 0],
        scale: [1, 1],
        rotation: 0,
        opacity: 1,
        flip_h: false,
        flip_v: false,
      },
      after: {
        position: [0, 0],
        scale: [1, 1],
        rotation: 0,
        opacity: 0.5,
        flip_h: false,
        flip_v: false,
      },
    });
  });

  it("names command *arguments* camelCase even though command *fields* are snake_case", async () => {
    // Tauri maps camelCase argument names onto snake_case Rust parameters. That
    // rule applies to the arguments of the `#[tauri::command]`, not to the
    // fields of a struct inside them — which is exactly the trap.
    ipc.handle("timeline_split", { project: {}, can_undo: true, can_redo: false });
    await timelineSplit("segment-1", 1_500_000);

    expect(ipc.lastCall("timeline_split")).toEqual({ segmentId: "segment-1", at: 1_500_000 });
  });
});

// ---------------------------------------------------------------------------
// The document and the edit response — snake_case
// ---------------------------------------------------------------------------

/** A document as `serde_json` writes it: no rename attributes, so Rust's own names. */
const WIRE_PROJECT = {
  id: "01J0000000000000000000",
  schema_version: 1,
  name: "Untitled",
  created_at: 1_700_000_000_000,
  updated_at: 1_700_000_000_001,
  canvas: { width: 1080, height: 1920, background: [0, 0, 0, 1] },
  fps: 30,
  materials: {
    videos: [
      {
        id: "material-1",
        path: "/media/a.mp4",
        width: 1920,
        height: 1080,
        duration: 8_000_000,
        fps: 30,
        has_audio: true,
        rotation: 90,
      },
    ],
    audios: [],
    images: [],
    texts: [],
    extras: {},
  },
  tracks: [
    {
      id: "track-1",
      kind: "video",
      name: "Video 1",
      segments: [
        {
          id: "segment-1",
          material_id: "material-1",
          target_range: { start: 0, duration: 2_000_000 },
          source_range: { start: 500_000, duration: 2_000_000 },
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
        },
      ],
      muted: false,
      locked: false,
      hidden: false,
      volume: 1,
    },
  ],
} satisfies Project;

const WIRE_EDIT_RESPONSE = {
  project: WIRE_PROJECT,
  can_undo: true,
  can_redo: false,
  undo_label: "Trim clip",
  redo_label: null,
} satisfies EditResponse;

describe("the project document keeps Rust's names", () => {
  it("is snake_case throughout, with no camelCase key anywhere in the tree", () => {
    for (const key of keysDeep(WIRE_PROJECT)) {
      expect(key, `"${key}" must not be camelCase`).toBe(key.toLowerCase());
    }
  });

  it("lands in the store without a translation step", async () => {
    ipc.handle("timeline_undo", WIRE_EDIT_RESPONSE);
    const { timelineUndo } = await import("@/modules/timeline/lib/api");
    useProjectStore.getState().applyEditResponse(await timelineUndo());

    const project = useProjectStore.getState().project;
    expect(project?.schema_version).toBe(1);
    expect(project?.tracks[0].segments[0].material_id).toBe("material-1");
    expect(project?.tracks[0].segments[0].source_range).toEqual({
      start: 500_000,
      duration: 2_000_000,
    });
    expect(useProjectStore.getState().undoLabel).toBe("Trim clip");
  });
});

describe("the media and project commands", () => {
  it("asks `project_import_media` for one path at a time", async () => {
    ipc.handle("project_import_media", {
      id: "material-1",
      kind: "video",
      name: "a.mp4",
      path: "/media/a.mp4",
      duration: 8_000_000,
      width: 1920,
      height: 1080,
      has_audio: true,
    });

    await projectImportMedia("/media/a.mp4");

    expect(ipc.lastCall("project_import_media")).toEqual({ path: "/media/a.mp4" });
  });

  it("turns the thumbnail cache's file paths into URLs the webview can load", async () => {
    ipc.handle("media_thumbnails", ["/cache/a-0.jpg", "/cache/a-1.jpg"]);

    const urls = await mediaThumbnails("/media/a.mp4", 12, 72);

    expect(ipc.lastCall("media_thumbnails")).toEqual({
      path: "/media/a.mp4",
      count: 12,
      height: 72,
    });
    // A bare filesystem path in an <img src> loads nothing.
    for (const url of urls) expect(url).toMatch(/^asset:\/\//);
  });
});
