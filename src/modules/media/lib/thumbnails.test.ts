import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { frameAt } from "@/modules/media/lib/thumbnails";
import { type IpcHarness, installIpc } from "@/test/ipc";

async function freshThumbnails() {
  vi.resetModules();
  const module = await import("@/modules/media/lib/thumbnails");
  return module.useThumbnailStore;
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
});

afterEach(() => {
  ipc.restore();
});

describe("the filmstrip queue", () => {
  it("decodes one file at a time, however many clips mount at once", async () => {
    const useThumbnailStore = await freshThumbnails();
    const gates: (() => void)[] = [];
    let inFlight = 0;
    let peak = 0;
    ipc.handle("media_thumbnails", () => {
      inFlight++;
      peak = Math.max(peak, inFlight);
      return new Promise<string[]>((resolve) => {
        gates.push(() => {
          inFlight--;
          resolve(["/cache/0.jpg"]);
        });
      });
    });

    const { request } = useThumbnailStore.getState();
    request("/media/a.mp4");
    request("/media/b.mp4");
    request("/media/c.mp4");

    // `media_thumbnails` blocks the Rust side while it decodes; three at once
    // would freeze the backend for as long as all three take.
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    gates[0]();
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(2));
    gates[1]();
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(3));
    gates[2]();

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().status["/media/c.mp4"]).toBe("ready"),
    );
    expect(peak).toBe(1);
  });

  it("asks once however many times a clip re-renders", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", ["/cache/0.jpg"]);

    for (let i = 0; i < 20; i++) useThumbnailStore.getState().request("/media/a.mp4");

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().status["/media/a.mp4"]).toBe("ready"),
    );
    expect(ipc.count("media_thumbnails")).toBe(1);
  });

  it("never retries a file that could not be decoded", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.fail("media_thumbnails", "no video stream");

    useThumbnailStore.getState().request("/media/broken.mp4");
    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().status["/media/broken.mp4"]).toBe("failed"),
    );

    useThumbnailStore.getState().request("/media/broken.mp4");
    useThumbnailStore.getState().request("/media/broken.mp4");

    // Retrying on every re-render would decode a broken file forever.
    expect(ipc.count("media_thumbnails")).toBe(1);
    expect(useThumbnailStore.getState().strips["/media/broken.mp4"]).toBeUndefined();
  });

  it("treats an empty strip as a failure rather than a strip of nothing", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", []);

    useThumbnailStore.getState().request("/media/a.mp4");

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().status["/media/a.mp4"]).toBe("failed"),
    );
  });

  it("ignores an empty path instead of asking Rust about it", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", ["/cache/0.jpg"]);

    useThumbnailStore.getState().request("");

    expect(ipc.count("media_thumbnails")).toBe(0);
    expect(useThumbnailStore.getState().status[""]).toBeUndefined();
  });

  it("keeps a failed file from blocking the ones queued behind it", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", (payload) =>
      payload.path === "/media/broken.mp4"
        ? Promise.reject("no video stream")
        : ["/cache/0.jpg", "/cache/1.jpg"],
    );

    useThumbnailStore.getState().request("/media/broken.mp4");
    useThumbnailStore.getState().request("/media/good.mp4");

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().status["/media/good.mp4"]).toBe("ready"),
    );
    expect(useThumbnailStore.getState().strips["/media/good.mp4"]).toHaveLength(2);
  });
});

describe("picking a frame out of a strip", () => {
  const strip = ["a", "b", "c", "d", "e"];

  it("indexes by position in the source file, not by position on the timeline", () => {
    // A clip showing seconds 8–12 of a one-minute file must not show frame 0.
    expect(frameAt(strip, 0, 60_000_000)).toBe("a");
    expect(frameAt(strip, 30_000_000, 60_000_000)).toBe("c");
    expect(frameAt(strip, 60_000_000, 60_000_000)).toBe("e");
  });

  it("clamps rather than falling off either end", () => {
    expect(frameAt(strip, -5_000_000, 60_000_000)).toBe("a");
    expect(frameAt(strip, 900_000_000, 60_000_000)).toBe("e");
  });

  it("falls back to the first frame for material with no knowable length", () => {
    // Stills and titles stretch to whatever the segment needs.
    expect(frameAt(strip, 4_000_000, 0)).toBe("a");
  });

  it("returns nothing rather than crashing when there is no strip yet", () => {
    expect(frameAt([], 1_000_000, 60_000_000)).toBe("");
  });
});
