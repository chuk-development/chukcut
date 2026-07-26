import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  MAX_WAVEFORM_BUCKETS,
  MIN_WAVEFORM_BUCKETS,
  needsFetch,
  waveformResolution,
} from "@/modules/media/lib/waveform";
import { type IpcHarness, installIpc } from "@/test/ipc";

async function freshWaveforms() {
  vi.resetModules();
  const module = await import("@/modules/media/lib/waveform");
  return module.useWaveformStore;
}

/** What Rust answers with: extremes and RMS per bucket, at the resolution asked for. */
function answer(buckets: number) {
  return {
    buckets,
    duration: 10_000_000,
    min: new Array(buckets).fill(-0.5),
    max: new Array(buckets).fill(0.5),
    rms: new Array(buckets).fill(0.2),
  };
}

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
});

afterEach(() => {
  ipc.restore();
});

describe("the resolution ladder", () => {
  it("never asks for less than a strip worth drawing", () => {
    expect(waveformResolution(0)).toBe(MIN_WAVEFORM_BUCKETS);
    expect(waveformResolution(1)).toBe(MIN_WAVEFORM_BUCKETS);
    expect(waveformResolution(MIN_WAVEFORM_BUCKETS)).toBe(MIN_WAVEFORM_BUCKETS);
  });

  it("rounds up to the next rung rather than to the pixel", () => {
    // The point of the ladder: a zoom gesture that sweeps 520 → 900 pixels asks
    // for one resolution, not four hundred.
    expect(waveformResolution(513)).toBe(1024);
    expect(waveformResolution(900)).toBe(1024);
    expect(waveformResolution(1024)).toBe(1024);
    expect(waveformResolution(1025)).toBe(2048);
  });

  it("stops at a resolution no screen can draw", () => {
    expect(waveformResolution(1_000_000)).toBe(MAX_WAVEFORM_BUCKETS);
  });
});

describe("deciding whether to fetch", () => {
  const entry = (over: Partial<{ requested: number; status: "pending" | "ready" | "failed" }>) => ({
    data: null,
    requested: 1024,
    status: "ready" as const,
    ...over,
  });

  it("fetches when nothing is known about the file", () => {
    expect(needsFetch(undefined, 512)).toBe(true);
  });

  it("does not fetch when the data on hand is already finer than the pixels", () => {
    // This is the short-circuit that keeps zooming out free: 1024 buckets drawn
    // into 512 pixels is downsampling, which is arithmetic, not a decode.
    expect(needsFetch(entry({ requested: 1024 }), 512)).toBe(false);
    expect(needsFetch(entry({ requested: 1024 }), 1024)).toBe(false);
  });

  it("fetches when the pixels have outrun the data", () => {
    expect(needsFetch(entry({ requested: 1024 }), 2048)).toBe(true);
  });

  it("does not fetch again while a finer request is already in flight", () => {
    expect(needsFetch(entry({ requested: 4096, status: "pending" }), 4096)).toBe(false);
  });

  it("never retries a file whose audio could not be read", () => {
    expect(needsFetch(entry({ requested: 512, status: "failed" }), 16_384)).toBe(false);
  });
});

describe("the waveform cache", () => {
  it("asks Rust for the rung, not for the pixel count", async () => {
    const useWaveformStore = await freshWaveforms();
    ipc.handle("media_waveform", answer(1024));

    useWaveformStore.getState().request("/media/a.mp3", 700);

    await vi.waitFor(() =>
      expect(useWaveformStore.getState().entries["/media/a.mp3"].status).toBe("ready"),
    );
    expect(ipc.lastCall("media_waveform")).toEqual({ path: "/media/a.mp3", buckets: 1024 });
    expect(useWaveformStore.getState().entries["/media/a.mp3"].data?.buckets).toBe(1024);
  });

  it("is pending before the first answer and keeps nothing to draw", async () => {
    const useWaveformStore = await freshWaveforms();
    ipc.handle("media_waveform", () => new Promise(() => {}));

    useWaveformStore.getState().request("/media/a.mp3", 512);

    const entry = useWaveformStore.getState().entries["/media/a.mp3"];
    expect(entry.status).toBe("pending");
    expect(entry.data).toBeNull();
  });

  it("does not go back to Rust when the zoom asks for less than it already has", async () => {
    const useWaveformStore = await freshWaveforms();
    ipc.handle("media_waveform", answer(4096));

    useWaveformStore.getState().request("/media/a.mp3", 4000);
    await vi.waitFor(() =>
      expect(useWaveformStore.getState().entries["/media/a.mp3"].status).toBe("ready"),
    );

    // Zooming out, then back in to less than what is cached.
    useWaveformStore.getState().request("/media/a.mp3", 300);
    useWaveformStore.getState().request("/media/a.mp3", 2000);
    useWaveformStore.getState().request("/media/a.mp3", 4096);

    expect(ipc.count("media_waveform")).toBe(1);
    expect(useWaveformStore.getState().entries["/media/a.mp3"].data?.buckets).toBe(4096);
  });

  it("goes back to Rust once the zoom has outrun the data", async () => {
    const useWaveformStore = await freshWaveforms();
    ipc.handle("media_waveform", (payload) => answer(payload.buckets as number));

    useWaveformStore.getState().request("/media/a.mp3", 500);
    await vi.waitFor(() =>
      expect(useWaveformStore.getState().entries["/media/a.mp3"].data?.buckets).toBe(512),
    );

    useWaveformStore.getState().request("/media/a.mp3", 5000);
    await vi.waitFor(() =>
      expect(useWaveformStore.getState().entries["/media/a.mp3"].data?.buckets).toBe(8192),
    );
    expect(ipc.count("media_waveform")).toBe(2);
  });

  it("keeps the coarse picture on screen while the finer one decodes", async () => {
    const useWaveformStore = await freshWaveforms();
    let release: (value: unknown) => void = () => {};
    ipc.handle("media_waveform", (payload) =>
      payload.buckets === 512 ? answer(512) : new Promise((resolve) => (release = resolve)),
    );

    useWaveformStore.getState().request("/media/a.mp3", 500);
    await vi.waitFor(() =>
      expect(useWaveformStore.getState().entries["/media/a.mp3"].data?.buckets).toBe(512),
    );

    useWaveformStore.getState().request("/media/a.mp3", 5000);

    // Blanking the clip on every zoom step would be a worse picture than a
    // slightly soft one.
    const entry = useWaveformStore.getState().entries["/media/a.mp3"];
    expect(entry.status).toBe("pending");
    expect(entry.data?.buckets).toBe(512);
    release(answer(8192));
  });

  it("collapses a burst of zoom steps into one decode at the finest of them", async () => {
    const useWaveformStore = await freshWaveforms();
    let unblock: (value: unknown) => void = () => {};
    const first = new Promise((resolve) => {
      unblock = resolve;
    });
    ipc.handle("media_waveform", (payload, index) =>
      index === 0 ? first : answer(payload.buckets as number),
    );

    // A drag on the zoom slider, with the first decode still running.
    useWaveformStore.getState().request("/media/a.mp3", 500);
    useWaveformStore.getState().request("/media/a.mp3", 1500);
    useWaveformStore.getState().request("/media/a.mp3", 3000);
    useWaveformStore.getState().request("/media/a.mp3", 9000);
    unblock(answer(512));

    await vi.waitFor(() =>
      expect(useWaveformStore.getState().entries["/media/a.mp3"].data?.buckets).toBe(16_384),
    );
    // Four zoom steps, two decodes: the one already running and one for the
    // finest of the rest. Not four.
    expect(ipc.count("media_waveform")).toBe(2);
  });

  it("marks a file with no readable audio failed, and stops asking", async () => {
    const useWaveformStore = await freshWaveforms();
    ipc.fail("media_waveform", "/media/silent.mp4 has no audio stream");

    useWaveformStore.getState().request("/media/silent.mp4", 512);
    await vi.waitFor(() =>
      expect(useWaveformStore.getState().entries["/media/silent.mp4"].status).toBe("failed"),
    );

    useWaveformStore.getState().request("/media/silent.mp4", 8192);
    expect(ipc.count("media_waveform")).toBe(1);
    expect(useWaveformStore.getState().entries["/media/silent.mp4"].data).toBeNull();
  });

  it("treats an answer with no buckets in it as a failure", async () => {
    const useWaveformStore = await freshWaveforms();
    ipc.handle("media_waveform", { buckets: 0, duration: 0, min: [], max: [], rms: [] });

    useWaveformStore.getState().request("/media/a.mp3", 512);

    await vi.waitFor(() =>
      expect(useWaveformStore.getState().entries["/media/a.mp3"].status).toBe("failed"),
    );
  });

  it("ignores an empty path instead of asking Rust about it", async () => {
    const useWaveformStore = await freshWaveforms();
    ipc.handle("media_waveform", answer(512));

    useWaveformStore.getState().request("", 512);

    expect(ipc.count("media_waveform")).toBe(0);
  });
});
