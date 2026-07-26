import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Strip } from "@/modules/media/lib/thumbnails";
import { frameAt } from "@/modules/media/lib/thumbnails";
import { type IpcHarness, installIpc } from "@/test/ipc";

async function freshThumbnails() {
  vi.resetModules();
  const module = await import("@/modules/media/lib/thumbnails");
  return module.useThumbnailStore;
}

/** The strip as it stands, holes and all. */
function tilesOf(strips: Record<string, Strip>, path: string) {
  return strips[path]?.tiles ?? [];
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
});

afterEach(() => {
  ipc.restore();
});

/** What Rust sends for a batch of tiles. */
function batch(over: Record<string, unknown> = {}) {
  return {
    job_id: "job-1",
    total: 12,
    produced: 0,
    tiles: [],
    complete: false,
    cancelled: false,
    error: null,
    ...over,
  };
}

/** One tile, at the slot in the strip it belongs to. */
function tile(index: number, name = `${index}.jpg`) {
  return { index, at: index * 1_000_000, path: `/cache/${name}` };
}

/**
 * Let the deferred abandonment run.
 *
 * `release` cancels on a timer so that React's unmount-then-remount does not
 * cancel a decode that is about to be wanted again; a test asserting that a
 * cancel did *not* happen has to get past that timer or it proves nothing.
 */
function settle() {
  return new Promise((resolve) => setTimeout(resolve, 1));
}

/** The terminal message every job ends with, whatever happened. */
function terminal(over: Record<string, unknown> = {}) {
  return batch({ complete: true, ...over });
}

describe("the filmstrip queue", () => {
  it("decodes one file at a time, however many clips mount at once", async () => {
    const useThumbnailStore = await freshThumbnails();
    let inFlight = 0;
    let peak = 0;
    ipc.handle("media_thumbnails", (_payload, index) => {
      inFlight++;
      peak = Math.max(peak, inFlight);
      return `job-${index}`;
    });

    const { request } = useThumbnailStore.getState();
    request("/media/a.mp4");
    request("/media/b.mp4");
    request("/media/c.mp4");

    // Decoding a strip is slow; three at once would put three decoders on the
    // machine at the same moment as playback. The job only ends when its
    // terminal batch arrives, not when the command answers.
    for (const path of ["/media/a.mp4", "/media/b.mp4", "/media/c.mp4"]) {
      await vi.waitFor(() => expect(ipc.lastCall("media_thumbnails")?.path).toBe(path));
      inFlight--;
      ipc.channel("media_thumbnails").emit(terminal({ produced: 1, tiles: [tile(0)] }));
    }

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/c.mp4"].status).toBe("ready"),
    );
    expect(ipc.count("media_thumbnails")).toBe(3);
    expect(peak).toBe(1);
  });

  it("asks once however many times a clip re-renders", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    for (let i = 0; i < 20; i++) useThumbnailStore.getState().request("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    ipc.channel("media_thumbnails").emit(terminal({ tiles: [tile(0)] }));

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].status).toBe("ready"),
    );
    expect(ipc.count("media_thumbnails")).toBe(1);
  });

  it("never retries a file that could not be decoded", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    useThumbnailStore.getState().request("/media/broken.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    // The failure is a field on the terminal batch. The command itself
    // succeeded — it answered with a job id long before the decoder got there.
    ipc.channel("media_thumbnails").emit(terminal({ error: "no video stream" }));

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/broken.mp4"].status).toBe("failed"),
    );
    expect(useThumbnailStore.getState().strips["/media/broken.mp4"].error).toBe("no video stream");

    useThumbnailStore.getState().request("/media/broken.mp4");
    useThumbnailStore.getState().request("/media/broken.mp4");

    // Retrying on every re-render would decode a broken file forever.
    expect(ipc.count("media_thumbnails")).toBe(1);
  });

  it("treats a job that produced nothing as a failure rather than a strip of nothing", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    useThumbnailStore.getState().request("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    ipc.channel("media_thumbnails").emit(terminal());

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].status).toBe("failed"),
    );
  });

  it("does not stall the queue when a stubbed command answers with no job id", async () => {
    const useThumbnailStore = await freshThumbnails();
    // No id means nothing was started and no terminal batch is coming. Waiting
    // for one would block every file behind it for the rest of the session.
    ipc.handle("media_thumbnails", null);

    useThumbnailStore.getState().request("/media/a.mp4");
    useThumbnailStore.getState().request("/media/b.mp4");

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/b.mp4"].status).toBe("failed"),
    );
    expect(ipc.count("media_thumbnails")).toBe(2);
  });

  it("ignores an empty path instead of asking Rust about it", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    useThumbnailStore.getState().request("");

    expect(ipc.count("media_thumbnails")).toBe(0);
    expect(useThumbnailStore.getState().strips[""]).toBeUndefined();
  });

  it("keeps a failed file from blocking the ones queued behind it", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", (_payload, index) => `job-${index}`);

    useThumbnailStore.getState().request("/media/broken.mp4");
    useThumbnailStore.getState().request("/media/good.mp4");

    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    ipc.channel("media_thumbnails").emit(terminal({ error: "no video stream" }));
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(2));
    ipc.channel("media_thumbnails").emit(terminal({ tiles: [tile(0), tile(1)] }));

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/good.mp4"].status).toBe("ready"),
    );
    expect(useThumbnailStore.getState().strips["/media/good.mp4"].received).toBe(2);
  });

  it("reports a command that refused outright, which is not the decode failing", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.fail("media_thumbnails", "count must be at least one");

    useThumbnailStore.getState().request("/media/a.mp4");

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].error).toBe(
        "count must be at least one",
      ),
    );
    expect(useThumbnailStore.getState().strips["/media/a.mp4"].status).toBe("failed");
  });
});

describe("tiles streaming in", () => {
  it("shows the tiles it has before the rest arrive", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    useThumbnailStore.getState().request("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    expect(useThumbnailStore.getState().strips["/media/a.mp4"].status).toBe("pending");

    ipc.channel("media_thumbnails").emit(batch({ produced: 2, tiles: [tile(0), tile(1)] }));

    // The point of streaming: two tiles are drawable while ten are still
    // decoding, rather than the clip staying flat until the last one lands.
    await vi.waitFor(() => {
      const strip = useThumbnailStore.getState().strips["/media/a.mp4"];
      expect(strip.received).toBe(2);
      expect(strip.status).toBe("partial");
    });

    ipc.channel("media_thumbnails").emit(terminal({ produced: 2 }));
    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].status).toBe("ready"),
    );
  });

  it("puts a tile where it belongs however late it arrives", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    useThumbnailStore.getState().request("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));

    const channel = ipc.channel("media_thumbnails");
    // Workers finish in whatever order they finish, and a cached tile arrives
    // immediately while its neighbours are still being decoded.
    channel.emit(batch({ tiles: [tile(8), tile(9)] }));
    channel.emit(batch({ tiles: [tile(0)] }));
    channel.emit(batch({ tiles: [tile(4)] }));

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].received).toBe(4),
    );

    // Position in the strip is position in the file. A tile that arrived third
    // must not end up third.
    const tiles = tilesOf(useThumbnailStore.getState().strips, "/media/a.mp4");
    expect(tiles[0]).toContain("0.jpg");
    expect(tiles[4]).toContain("4.jpg");
    expect(tiles[8]).toContain("8.jpg");
    expect(tiles[9]).toContain("9.jpg");
    expect(tiles[1]).toBeNull();
    expect(tiles[5]).toBeNull();
  });

  it("takes tiles out of order within a single batch", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    useThumbnailStore.getState().request("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    ipc.channel("media_thumbnails").emit(batch({ tiles: [tile(7), tile(2), tile(11)] }));

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].received).toBe(3),
    );
    const tiles = tilesOf(useThumbnailStore.getState().strips, "/media/a.mp4");
    expect(tiles[2]).toContain("2.jpg");
    expect(tiles[7]).toContain("7.jpg");
    expect(tiles[11]).toContain("11.jpg");
  });

  it("counts a slot once when two batches overlap", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    useThumbnailStore.getState().request("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));

    const channel = ipc.channel("media_thumbnails");
    channel.emit(batch({ tiles: [tile(0), tile(1)] }));
    channel.emit(batch({ tiles: [tile(1, "1b.jpg"), tile(2)] }));

    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].received).toBe(3),
    );
    expect(tilesOf(useThumbnailStore.getState().strips, "/media/a.mp4")[1]).toContain("1b.jpg");
  });

  it("keeps the tiles that arrived before the decode gave up", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    useThumbnailStore.getState().request("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    const channel = ipc.channel("media_thumbnails");
    channel.emit(batch({ tiles: [tile(0)] }));
    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].received).toBe(1),
    );

    channel.emit(terminal({ produced: 1, error: "the file was truncated halfway through" }));

    // Half a filmstrip beats none: the clip keeps what it got and stops asking.
    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].status).toBe("ready"),
    );
    expect(useThumbnailStore.getState().strips["/media/a.mp4"].received).toBe(1);
  });

  it("grows the strip when Rust decides on more tiles than were asked for", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-1");

    useThumbnailStore.getState().request("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    ipc.channel("media_thumbnails").emit(batch({ total: 15, tiles: [tile(14)] }));

    await vi.waitFor(() =>
      expect(tilesOf(useThumbnailStore.getState().strips, "/media/a.mp4")).toHaveLength(15),
    );
  });
});

describe("a strip nobody is looking at any more", () => {
  it("cancels the decode when the last clip using the file goes", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-7");
    ipc.handle("media_thumbnails_cancel", true);

    useThumbnailStore.getState().retain("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));

    useThumbnailStore.getState().release("/media/a.mp4");

    // A decode whose only product is pixels for a lane that no longer exists is
    // work taken from playback.
    await vi.waitFor(() =>
      expect(ipc.lastCall("media_thumbnails_cancel")).toEqual({ jobId: "job-7" }),
    );
    expect(useThumbnailStore.getState().strips["/media/a.mp4"]).toBeUndefined();
  });

  it("keeps decoding while any other clip still wants it", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-7");
    ipc.handle("media_thumbnails_cancel", true);

    // Two clips cut from the same file share one strip.
    useThumbnailStore.getState().retain("/media/a.mp4");
    useThumbnailStore.getState().retain("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));

    useThumbnailStore.getState().release("/media/a.mp4");
    await settle();

    expect(ipc.count("media_thumbnails_cancel")).toBe(0);
    expect(useThumbnailStore.getState().strips["/media/a.mp4"]).toBeDefined();
  });

  it("survives the unmount-and-remount React does in development", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-7");
    ipc.handle("media_thumbnails_cancel", true);

    useThumbnailStore.getState().retain("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));

    // StrictMode: effect, cleanup, effect. Cancelling in the middle of that
    // would be a bug that only exists in development.
    useThumbnailStore.getState().release("/media/a.mp4");
    useThumbnailStore.getState().retain("/media/a.mp4");
    await settle();

    expect(ipc.count("media_thumbnails_cancel")).toBe(0);
    expect(ipc.count("media_thumbnails")).toBe(1);
  });

  it("keeps a strip that already finished, rather than decoding it twice", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", "job-7");
    ipc.handle("media_thumbnails_cancel", true);

    useThumbnailStore.getState().retain("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));
    ipc.channel("media_thumbnails").emit(terminal({ produced: 1, tiles: [tile(0)] }));
    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].status).toBe("ready"),
    );

    useThumbnailStore.getState().release("/media/a.mp4");
    await settle();

    // A finished strip is a handful of URLs. Throwing it away would mean
    // decoding the file again the next time a clip from it appears.
    expect(ipc.count("media_thumbnails_cancel")).toBe(0);
    expect(useThumbnailStore.getState().strips["/media/a.mp4"].status).toBe("ready");
  });

  it("lets the queue move on once the cancelled job reports back", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", (_payload, index) => `job-${index}`);
    ipc.handle("media_thumbnails_cancel", true);

    useThumbnailStore.getState().retain("/media/a.mp4");
    useThumbnailStore.getState().request("/media/b.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));

    useThumbnailStore.getState().release("/media/a.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails_cancel")).toBe(1));
    // Rust answers a cancellation with a terminal batch like any other ending.
    ipc.channel("media_thumbnails").emit(terminal({ cancelled: true }));

    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(2));
    expect(ipc.lastCall("media_thumbnails")?.path).toBe("/media/b.mp4");
  });

  it("drops a file still waiting its turn without troubling Rust", async () => {
    const useThumbnailStore = await freshThumbnails();
    ipc.handle("media_thumbnails", (_payload, index) => `job-${index}`);
    ipc.handle("media_thumbnails_cancel", true);

    useThumbnailStore.getState().request("/media/a.mp4");
    useThumbnailStore.getState().retain("/media/b.mp4");
    await vi.waitFor(() => expect(ipc.count("media_thumbnails")).toBe(1));

    // B never started, so there is no job to cancel — just a queue entry to
    // forget.
    useThumbnailStore.getState().release("/media/b.mp4");
    await settle();
    expect(ipc.count("media_thumbnails_cancel")).toBe(0);
    expect(useThumbnailStore.getState().strips["/media/b.mp4"]).toBeUndefined();

    ipc.channel("media_thumbnails").emit(terminal({ tiles: [tile(0)] }));
    await vi.waitFor(() =>
      expect(useThumbnailStore.getState().strips["/media/a.mp4"].status).toBe("ready"),
    );
    expect(ipc.count("media_thumbnails")).toBe(1);
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

  it("says so when the tile for that position has not decoded", () => {
    // The filmstrip draws a placeholder in that slot. Substituting a frame from
    // elsewhere in the file would show the user a frame that is not there.
    expect(frameAt(["a", null, "c"], 30_000_000, 60_000_000)).toBe("");
  });

  it("walks outward to the nearest tile when the caller would rather have one", () => {
    expect(frameAt(["a", null, "c"], 30_000_000, 60_000_000, true)).toBe("a");
    expect(frameAt([null, null, "c"], 0, 60_000_000, true)).toBe("c");
    expect(frameAt([null, null, null], 0, 60_000_000, true)).toBe("");
  });
});
