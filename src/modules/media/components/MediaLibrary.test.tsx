import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { makeMaterial, makeProject } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

async function mountLibrary() {
  vi.resetModules();
  const { MediaLibrary } = await import("@/modules/media/components/MediaLibrary");
  const { useMediaStore } = await import("@/modules/media/store");
  return { view: render(<MediaLibrary />), useMediaStore };
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  // Answers with a job id and never sends a terminal batch: these tests are
  // about the library, not about what ends up on a tile.
  ipc.handle("media_thumbnails", "job-1");
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

  it("shows a tile for a file that imported", async () => {
    ipc.handle("plugin:dialog|open", ["/media/clip.mp4"]);
    ipc.handle("project_import_media", makeMaterial("v1", { name: "clip.mp4" }));
    await mountLibrary();

    fireEvent.click(screen.getByRole("button", { name: "Import" }));

    expect(await screen.findByText("clip.mp4")).toBeInTheDocument();
    expect(screen.queryByText("No media imported")).toBeNull();
    expect(screen.getByText("1 item")).toBeInTheDocument();
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
    ipc.handle("plugin:dialog|open", ["/media/clip.mp4", "/media/song.mp3"]);
    ipc.handle("project_import_media", (payload) =>
      payload.path === "/media/song.mp3"
        ? makeMaterial("a1", { kind: "audio", name: "song.mp3", path: "/media/song.mp3" })
        : makeMaterial("v1", { name: "clip.mp4" }),
    );
    await mountLibrary();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    expect(await screen.findByText("clip.mp4")).toBeInTheDocument();

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
