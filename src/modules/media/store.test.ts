/**
 * Importing media.
 *
 * The media store and the thumbnail queue are module-level singletons with a
 * sticky per-path status, so each test reloads the module graph rather than
 * trying to scrub that state clean.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { makeMaterial, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

async function freshMedia() {
  vi.resetModules();
  const { useMediaStore } = await import("@/modules/media/store");
  const { useThumbnailStore } = await import("@/modules/media/lib/thumbnails");
  const { useProjectStore } = await import("@/modules/project/store");
  return { useMediaStore, useThumbnailStore, useProjectStore };
}

/** Rust answers a probe from the path, the way the real command does. */
function probeByPath(byPath: Record<string, ReturnType<typeof makeMaterial>>) {
  return (payload: Record<string, unknown>) => {
    const material = byPath[payload.path as string];
    if (!material) throw new Error(`the test did not script a probe for ${payload.path}`);
    return material;
  };
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  ipc.handle("media_thumbnails", []);
});

afterEach(() => {
  ipc.restore();
});

describe("importing", () => {
  it("imports every path and keeps them in the order they were given", async () => {
    const { useMediaStore } = await freshMedia();
    const video = makeMaterial("v1", { path: "/media/v1.mp4" });
    const song = makeMaterial("a1", { kind: "audio", path: "/media/a1.mp3", name: "a1.mp3" });
    ipc.handle(
      "project_import_media",
      probeByPath({ "/media/v1.mp4": video, "/media/a1.mp3": song }),
    );
    ipc.handle("project_get", makeProject());

    const imported = await useMediaStore.getState().importPaths(["/media/v1.mp4", "/media/a1.mp3"]);

    expect(imported.map((item) => item.id)).toEqual(["v1", "a1"]);
    expect(useMediaStore.getState().items.map((item) => item.id)).toEqual(["v1", "a1"]);
    expect(useMediaStore.getState().error).toBeNull();
    expect(useMediaStore.getState().importing).toBe(false);
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

  it("does not add a second tile when the same file is imported again", async () => {
    const { useMediaStore } = await freshMedia();
    // Rust answers a repeat import with the material already in the pool.
    const video = makeMaterial("v1", { path: "/media/v1.mp4" });
    ipc.handle("project_import_media", video);
    ipc.handle("project_get", makeProject());

    await useMediaStore.getState().importPaths(["/media/v1.mp4"]);
    await useMediaStore.getState().importPaths(["/media/v1.mp4"]);

    expect(ipc.count("project_import_media")).toBe(2);
    expect(useMediaStore.getState().items).toHaveLength(1);
  });

  it("does not add a second tile when one call contains the same file twice", async () => {
    const { useMediaStore } = await freshMedia();
    ipc.handle("project_import_media", makeMaterial("v1", { path: "/media/v1.mp4" }));
    ipc.handle("project_get", makeProject());

    await useMediaStore.getState().importPaths(["/media/v1.mp4", "/media/v1.mp4"]);

    expect(useMediaStore.getState().items).toHaveLength(1);
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
  it("ends with an empty library and Rust's sentence, not an exception", async () => {
    const { useMediaStore } = await freshMedia();
    ipc.fail("project_import_media", "cannot read /media/broken.mp4: no such file");

    const imported = await useMediaStore.getState().importPaths(["/media/broken.mp4"]);

    expect(imported).toEqual([]);
    const state = useMediaStore.getState();
    expect(state.items).toEqual([]);
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

  it("keeps the imported files when only the document re-read failed", async () => {
    const { useMediaStore } = await freshMedia();
    ipc.handle("project_import_media", makeMaterial("v1"));
    ipc.fail("project_get", "the project lock is poisoned");

    await useMediaStore.getState().importPaths(["/media/v1.mp4"]);

    expect(useMediaStore.getState().items).toHaveLength(1);
    expect(useMediaStore.getState().error).toBe("the project lock is poisoned");
  });

  it("does nothing at all for an empty selection", async () => {
    const { useMediaStore } = await freshMedia();

    const imported = await useMediaStore.getState().importPaths([]);

    expect(imported).toEqual([]);
    expect(ipc.log).toEqual([]);
    expect(useMediaStore.getState().importing).toBe(false);
  });
});

describe("removing a library tile", () => {
  it("takes the row away without asking Rust to touch the pool", async () => {
    const { useMediaStore } = await freshMedia();
    ipc.handle("project_import_media", makeMaterial("v1"));
    ipc.handle("project_get", makeProject());
    await useMediaStore.getState().importPaths(["/media/v1.mp4"]);
    ipc.reset();

    useMediaStore.getState().remove("v1");

    // A clip on the timeline may still reference the material.
    expect(useMediaStore.getState().items).toEqual([]);
    expect(ipc.log).toEqual([]);
  });
});
