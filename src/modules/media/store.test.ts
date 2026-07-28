/**
 * Importing and removing media.
 *
 * The media store and the thumbnail queue are module-level singletons with a
 * sticky per-path status, so each test reloads the module graph rather than
 * trying to scrub that state clean.
 *
 * The library rows themselves live in the *document* — `libraryItems` over the
 * pool — so these tests assert on the project store where the old ones
 * asserted on a session list. That session list was the bug: a saved and
 * reopened project had a full pool and an empty panel.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  makeEditResponse,
  makeMaterial,
  makeProject,
  makeSegment,
  makeTrack,
} from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

async function freshMedia() {
  vi.resetModules();
  const { useMediaStore } = await import("@/modules/media/store");
  const { useThumbnailStore } = await import("@/modules/media/lib/thumbnails");
  const { useProjectStore } = await import("@/modules/project/store");
  const { libraryItems } = await import("@/modules/media/lib/library");
  return { useMediaStore, useThumbnailStore, useProjectStore, libraryItems };
}

/** Rust answers a probe from the path, the way the real command does. */
function probeByPath(byPath: Record<string, ReturnType<typeof makeMaterial>>) {
  return (payload: Record<string, unknown>) => {
    const material = byPath[payload.path as string];
    if (!material) throw new Error(`the test did not script a probe for ${payload.path}`);
    return material;
  };
}

/** A project whose pool holds one video, with `clips` segments referencing it. */
function projectWithVideo(clips = 0) {
  const project = makeProject();
  project.materials.videos.push({
    id: "v1",
    path: "/media/v1.mp4",
    width: 1920,
    height: 1080,
    duration: 4_000_000,
    fps: 30,
    has_audio: true,
    rotation: 0,
  });
  const segments = Array.from({ length: clips }, (_, i) =>
    makeSegment(`s${i}`, {
      material_id: "v1",
      target_range: { start: i * 1_000_000, duration: 1_000_000 },
      source_range: { start: 0, duration: 1_000_000 },
    }),
  );
  project.tracks.push(makeTrack("t1", { segments }));
  return project;
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  ipc.handle("media_thumbnails", "job-1");
  ipc.handle("media_missing_files", []);
});

afterEach(() => {
  ipc.restore();
});

describe("importing", () => {
  it("imports every path and the library sees them through the pool", async () => {
    const { useMediaStore, useProjectStore, libraryItems } = await freshMedia();
    const video = makeMaterial("v1", { path: "/media/v1.mp4" });
    const song = makeMaterial("a1", { kind: "audio", path: "/media/a1.mp3", name: "a1.mp3" });
    ipc.handle(
      "project_import_media",
      probeByPath({ "/media/v1.mp4": video, "/media/a1.mp3": song }),
    );
    // What Rust's pool looks like after both imports — the library renders
    // this, not a session list.
    const refreshed = makeProject();
    refreshed.materials.videos.push({
      id: "v1",
      path: "/media/v1.mp4",
      width: 1920,
      height: 1080,
      duration: 4_000_000,
      fps: 30,
      has_audio: true,
      rotation: 0,
    });
    refreshed.materials.audios.push({
      id: "a1",
      path: "/media/a1.mp3",
      duration: 4_000_000,
      sample_rate: 48_000,
      channels: 2,
    });
    ipc.handle("project_get", refreshed);

    const imported = await useMediaStore.getState().importPaths(["/media/v1.mp4", "/media/a1.mp3"]);

    expect(imported.map((item) => item.id)).toEqual(["v1", "a1"]);
    expect(useMediaStore.getState().error).toBeNull();
    expect(useMediaStore.getState().importing).toBe(false);
    expect(libraryItems(useProjectStore.getState().project).map((item) => item.id)).toEqual([
      "v1",
      "a1",
    ]);
  });

  it("asks for a filmstrip for the video and not for the song", async () => {
    const { useMediaStore } = await freshMedia();
    const video = makeMaterial("v1", { path: "/media/v1.mp4" });
    const song = makeMaterial("a1", { kind: "audio", path: "/media/a1.mp3" });
    ipc.handle(
      "project_import_media",
      probeByPath({ "/media/v1.mp4": video, "/media/a1.mp3": song }),
    );
    ipc.handle("project_get", makeProject());

    await useMediaStore.getState().importPaths(["/media/v1.mp4", "/media/a1.mp3"]);

    // Decoding a strip is expensive and an audio tile has nowhere to put one.
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    expect(ipc.lastCall("media_thumbnails")).toMatchObject({ path: "/media/v1.mp4" });
  });

  it("re-reads the document, because the material pool changed under it", async () => {
    const { useMediaStore, useProjectStore } = await freshMedia();
    const refreshed = makeProject({ name: "now with media" });
    ipc.handle("project_import_media", makeMaterial("v1"));
    ipc.handle("project_get", refreshed);
    useProjectStore.getState().loadProject(makeProject());

    await useMediaStore.getState().importPaths(["/media/v1.mp4"]);

    // Inserting a segment resolves `material_id` against the pool, so the UI
    // has to have seen the new material before it can place a clip.
    expect(ipc.count("project_get")).toBe(1);
    expect(useProjectStore.getState().project).toBe(refreshed);
  });

  it("leaves the undo stack alone, because importing is not undoable", async () => {
    const { useMediaStore, useProjectStore } = await freshMedia();
    ipc.handle("project_import_media", makeMaterial("v1"));
    ipc.handle("project_get", makeProject());
    useProjectStore.setState({ canUndo: true, undoLabel: "Insert clip" });

    await useMediaStore.getState().importPaths(["/media/v1.mp4"]);

    expect(useProjectStore.getState().canUndo).toBe(true);
    expect(useProjectStore.getState().undoLabel).toBe("Insert clip");
  });
});

describe("when a file cannot be probed", () => {
  it("ends with Rust's sentence, not an exception", async () => {
    const { useMediaStore } = await freshMedia();
    ipc.fail("project_import_media", "cannot read /media/broken.mp4: no such file");

    const imported = await useMediaStore.getState().importPaths(["/media/broken.mp4"]);

    expect(imported).toEqual([]);
    const state = useMediaStore.getState();
    expect(state.error).toBe("cannot read /media/broken.mp4: no such file");
    // A stuck spinner would make the Import button dead for the session.
    expect(state.importing).toBe(false);
  });

  it("does not re-read the document when nothing was imported", async () => {
    const { useMediaStore } = await freshMedia();
    ipc.fail("project_import_media", "unsupported format");

    await useMediaStore.getState().importPaths(["/media/broken.xyz"]);

    expect(ipc.count("project_get")).toBe(0);
  });

  it("still imports the three files that were fine", async () => {
    const { useMediaStore } = await freshMedia();
    ipc.handle("project_import_media", (payload) => {
      if (payload.path === "/media/broken.mp4") {
        return Promise.reject("cannot read /media/broken.mp4: no such file");
      }
      return makeMaterial(String(payload.path).slice(7, -4), { path: String(payload.path) });
    });
    ipc.handle("project_get", makeProject());

    const imported = await useMediaStore
      .getState()
      .importPaths(["/media/a.mp4", "/media/broken.mp4", "/media/b.mp4", "/media/c.mp4"]);

    expect(imported.map((item) => item.id)).toEqual(["a", "b", "c"]);
    expect(useMediaStore.getState().error).toBe("cannot read /media/broken.mp4: no such file");
  });

  it("does nothing at all for an empty selection", async () => {
    const { useMediaStore } = await freshMedia();

    const imported = await useMediaStore.getState().importPaths([]);

    expect(imported).toEqual([]);
    expect(ipc.log).toEqual([]);
    expect(useMediaStore.getState().importing).toBe(false);
  });
});

describe("removing a material from the project", () => {
  it("sends an undoable remove_material edit carrying the whole pool entry", async () => {
    const { useMediaStore, useProjectStore } = await freshMedia();
    const project = projectWithVideo(2);
    useProjectStore.getState().loadProject(project);
    const after = makeProject({ name: "after removal" });
    ipc.handle("timeline_apply", makeEditResponse(after, { undo_label: "Remove media" }));

    await useMediaStore.getState().removeFromProject("v1");

    // The whole material plus its index, so Rust can undo exactly and refuse
    // a stale gesture — and the clips are deliberately not in the payload:
    // removing a library entry never deletes a cut.
    expect(ipc.lastCall("timeline_apply")).toMatchObject({
      command: {
        type: "remove_material",
        index: 0,
        material: { kind: "video", id: "v1", path: "/media/v1.mp4" },
      },
    });
    // The response replaces the document and moves the undo state, like every
    // other edit.
    expect(useProjectStore.getState().project).toBe(after);
    expect(useProjectStore.getState().undoLabel).toBe("Remove media");
  });

  it("asks Rust for nothing when the id is not in the pool", async () => {
    const { useMediaStore, useProjectStore } = await freshMedia();
    useProjectStore.getState().loadProject(makeProject());

    await useMediaStore.getState().removeFromProject("ghost");

    expect(ipc.count("timeline_apply")).toBe(0);
  });

  it("surfaces a refusal instead of throwing", async () => {
    const { useMediaStore, useProjectStore } = await freshMedia();
    useProjectStore.getState().loadProject(projectWithVideo(0));
    ipc.fail("timeline_apply", "that media is no longer where this edit expected it");

    await useMediaStore.getState().removeFromProject("v1");

    expect(useProjectStore.getState().error).toBe(
      "that media is no longer where this edit expected it",
    );
  });
});

describe("the missing-on-disk check", () => {
  it("asks Rust about every pool file and keeps the ones that are gone", async () => {
    const { useMediaStore, useProjectStore } = await freshMedia();
    useProjectStore.getState().loadProject(projectWithVideo(0));
    ipc.handle("media_missing_files", ["/media/v1.mp4"]);

    await useMediaStore.getState().refreshMissing();

    expect(ipc.lastCall("media_missing_files")).toMatchObject({ paths: ["/media/v1.mp4"] });
    expect(useMediaStore.getState().missingPaths).toEqual(["/media/v1.mp4"]);
  });

  it("keeps the same array reference when the answer has not changed", async () => {
    // Memos on the timeline key off this reference; a fresh but equal array
    // every check would repaint every clip for nothing.
    const { useMediaStore, useProjectStore } = await freshMedia();
    useProjectStore.getState().loadProject(projectWithVideo(0));
    ipc.handle("media_missing_files", ["/media/v1.mp4"]);

    await useMediaStore.getState().refreshMissing();
    const first = useMediaStore.getState().missingPaths;
    await useMediaStore.getState().refreshMissing();

    expect(useMediaStore.getState().missingPaths).toBe(first);
  });

  it("keeps the last answer when the check itself fails", async () => {
    const { useMediaStore, useProjectStore } = await freshMedia();
    useProjectStore.getState().loadProject(projectWithVideo(0));
    ipc.handle("media_missing_files", ["/media/v1.mp4"]);
    await useMediaStore.getState().refreshMissing();

    ipc.fail("media_missing_files", "the media task failed");
    await useMediaStore.getState().refreshMissing();

    // Advisory, not authoritative: a failed round trip must not flash the
    // whole library offline or silently clear a true answer.
    expect(useMediaStore.getState().missingPaths).toEqual(["/media/v1.mp4"]);
  });

  it("answers empty without asking when the pool has no files", async () => {
    const { useMediaStore, useProjectStore } = await freshMedia();
    useProjectStore.getState().loadProject(makeProject());

    await useMediaStore.getState().refreshMissing();

    expect(ipc.count("media_missing_files")).toBe(0);
    expect(useMediaStore.getState().missingPaths).toEqual([]);
  });
});
