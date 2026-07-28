import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Project } from "@/modules/project/types";
import {
  makeEditResponse,
  makeMaterial,
  makeProject,
  makeSegment,
  makeTrack,
} from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

/**
 * Mount the library, optionally over an already-open project.
 *
 * The library renders the project's material pool, so most tests hand it a
 * document rather than scripting imports — that is the architecture under
 * test: what is in the pool is what is on the panel.
 */
async function mountLibrary(project?: Project) {
  vi.resetModules();
  const { MediaLibrary } = await import("@/modules/media/components/MediaLibrary");
  const { useMediaStore } = await import("@/modules/media/store");
  const { useProjectStore } = await import("@/modules/project/store");
  if (project) useProjectStore.getState().loadProject(project);
  return { view: render(<MediaLibrary />), useMediaStore, useProjectStore };
}

/** A pool holding one video, one song and one still, plus `clips` referencing the video. */
function poolProject(clips = 0): Project {
  const project = makeProject();
  project.materials.videos.push({
    id: "v1",
    path: "/media/clip.mp4",
    width: 1920,
    height: 1080,
    duration: 4_000_000,
    fps: 30,
    has_audio: true,
    rotation: 0,
  });
  project.materials.audios.push({
    id: "a1",
    path: "/media/song.mp3",
    duration: 4_000_000,
    sample_rate: 48_000,
    channels: 2,
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
  // Answers with a job id and never sends a terminal batch: these tests are
  // about the library, not about what ends up on a tile.
  ipc.handle("media_thumbnails", "job-1");
  ipc.handle("media_missing_files", []);
  ipc.handle("project_get", makeProject());
});

afterEach(() => {
  ipc.restore();
});

describe("the media library", () => {
  it("offers a way in when it is empty", async () => {
    await mountLibrary();

    expect(screen.getByText("No media imported")).toBeInTheDocument();
  });

  it("renders the pool of a project it never saw imported — the reopen case", async () => {
    // The regression this architecture fixes: importing wrote the pool and
    // saving persisted it, but the panel only knew the session's own imports,
    // so a reopened project showed an empty library over a full pool.
    await mountLibrary(poolProject());

    expect(screen.getByText("clip.mp4")).toBeInTheDocument();
    expect(screen.getByText("song.mp3")).toBeInTheDocument();
    expect(screen.getByText("2 items")).toBeInTheDocument();
    expect(ipc.count("project_import_media")).toBe(0);
  });

  it("shows a tile for a file that imported", async () => {
    ipc.handle("plugin:dialog|open", ["/media/clip.mp4"]);
    ipc.handle("project_import_media", makeMaterial("v1", { name: "clip.mp4" }));
    // The tiles come from the re-read document, which is how the tile appears:
    // Rust put the material in the pool, the frontend re-read it.
    ipc.handle("project_get", poolProject());
    await mountLibrary();

    fireEvent.click(screen.getByRole("button", { name: "Import" }));

    expect(await screen.findByText("clip.mp4")).toBeInTheDocument();
    expect(screen.queryByText("No media imported")).toBeNull();
  });

  it("stays on the empty state and shows why when the probe fails", async () => {
    ipc.handle("plugin:dialog|open", ["/media/broken.mp4"]);
    ipc.fail("project_import_media", "cannot read /media/broken.mp4: no such file");
    await mountLibrary();

    fireEvent.click(screen.getByRole("button", { name: "Import" }));

    // A crash here loses the panel; the file being unreadable is normal.
    expect(
      await screen.findByText("cannot read /media/broken.mp4: no such file"),
    ).toBeInTheDocument();
    expect(screen.getByText("No media imported")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import" })).toBeEnabled();
  });

  it("shows the dialog's own failure rather than swallowing it", async () => {
    ipc.fail("plugin:dialog|open", "dialog:allow-open is not granted");
    await mountLibrary();

    fireEvent.click(screen.getByRole("button", { name: "Import" }));

    expect(await screen.findByText("dialog:allow-open is not granted")).toBeInTheDocument();
  });

  it("imports nothing when the dialog is cancelled", async () => {
    ipc.handle("plugin:dialog|open", null);
    await mountLibrary();

    fireEvent.click(screen.getByRole("button", { name: "Import" }));

    await waitFor(() => expect(ipc.count("plugin:dialog|open")).toBe(1));
    expect(ipc.count("project_import_media")).toBe(0);
    expect(screen.getByText("No media imported")).toBeInTheDocument();
  });

  it("keeps the audio tab to audio", async () => {
    await mountLibrary(poolProject());

    await userEvent.setup().click(screen.getByRole("tab", { name: "Audio" }));

    const audioPanel = await screen.findByRole("tabpanel");
    expect(within(audioPanel).getByText("song.mp3")).toBeInTheDocument();
    expect(within(audioPanel).queryByText("clip.mp4")).toBeNull();
  });

  it("lets the user dismiss the error once they have read it", async () => {
    ipc.handle("plugin:dialog|open", ["/media/broken.mp4"]);
    ipc.fail("project_import_media", "unsupported format");
    await mountLibrary();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    const message = await screen.findByText("unsupported format");

    fireEvent.click(message);

    await waitFor(() => expect(screen.queryByText("unsupported format")).toBeNull());
  });
});

describe("removing media from the project", () => {
  it("sends the undoable remove_material edit, never a session-only removal", async () => {
    const project = poolProject(2);
    const after = makeProject({ name: "after removal" });
    ipc.handle("timeline_apply", makeEditResponse(after, { undo_label: "Remove media" }));
    const { useProjectStore } = await mountLibrary(project);

    fireEvent.click(screen.getByRole("button", { name: "Remove clip.mp4" }));

    await waitFor(() => expect(ipc.count("timeline_apply")).toBe(1));
    expect(ipc.lastCall("timeline_apply")).toMatchObject({
      command: {
        type: "remove_material",
        index: 0,
        material: { kind: "video", id: "v1", path: "/media/clip.mp4" },
      },
    });
    // The document is replaced like after any edit, which is also what makes
    // the tile disappear — the pool no longer holds it.
    await waitFor(() => expect(useProjectStore.getState().project).toBe(after));
  });

  it("says in the context menu how many clips will go offline", async () => {
    await mountLibrary(poolProject(2));

    fireEvent.contextMenu(screen.getByRole("button", { name: /clip\.mp4 — drag/ }));

    expect(
      await screen.findByText("Remove from project — 2 clips will go offline"),
    ).toBeInTheDocument();
  });

  it("offers a plain removal for a material no clip uses", async () => {
    await mountLibrary(poolProject(0));

    fireEvent.contextMenu(screen.getByRole("button", { name: /song\.mp3 — drag/ }));

    expect(await screen.findByText("Remove from project")).toBeInTheDocument();
  });
});

describe("a file gone from disk", () => {
  it("shows the card in the missing state", async () => {
    ipc.handle("media_missing_files", ["/media/clip.mp4"]);
    await mountLibrary(poolProject());

    // The check runs on mount; the card flips as soon as the answer lands.
    expect(await screen.findByText("missing on disk")).toBeInTheDocument();
    expect(screen.getByLabelText("File missing on disk")).toBeInTheDocument();
    // The healthy card is untouched.
    expect(document.querySelectorAll('[data-slot="media-item"][data-missing]')).toHaveLength(1);
  });
});
