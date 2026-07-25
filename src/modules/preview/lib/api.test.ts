import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { fetchFrame, proxyResolution } from "@/modules/preview/lib/api";

/** A `Response` with only the parts `fetchFrame` looks at. */
function respondWith(status: number): Response {
  return {
    status,
    ok: status >= 200 && status < 300,
    blob: async () => new Blob(["jpeg bytes"]),
  } as unknown as Response;
}

let fetchMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  fetchMock = vi.fn();
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal(
    "createImageBitmap",
    vi.fn(async () => ({ close: vi.fn() }) as unknown),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("fetching a frame", () => {
  it("asks the frame protocol for the session's base URL plus the frame number", async () => {
    fetchMock.mockResolvedValue(respondWith(200));

    await fetchFrame("chukcut-frame://preview/4", 91);

    expect(fetchMock).toHaveBeenCalledWith("chukcut-frame://preview/4/91");
  });

  it("calls a 410 `gone`, because the session behind it was superseded", async () => {
    fetchMock.mockResolvedValue(respondWith(410));

    // Not an error: a newer frame from the current session is already coming,
    // and telling the user about it would fire on every scrub.
    expect(await fetchFrame("chukcut-frame://preview/4", 1)).toEqual({ status: "gone" });
  });

  it("calls a 404 `pending`, because the frame is simply not encoded yet", async () => {
    fetchMock.mockResolvedValue(respondWith(404));

    expect(await fetchFrame("chukcut-frame://preview/4", 1)).toEqual({ status: "pending" });
  });

  it("reports any other bad status with the frame number in it", async () => {
    fetchMock.mockResolvedValue(respondWith(500));

    expect(await fetchFrame("chukcut-frame://preview/4", 7)).toEqual({
      status: "error",
      message: "frame 7: 500",
    });
  });

  it("decodes a 200 into a bitmap", async () => {
    fetchMock.mockResolvedValue(respondWith(200));

    const result = await fetchFrame("chukcut-frame://preview/4", 3);

    expect(result.status).toBe("ok");
  });

  it("turns a transport failure into a result rather than a rejection", async () => {
    fetchMock.mockRejectedValue(new Error("protocol handler is gone"));

    // The paint effect has no catch; a throw here would take the panel down.
    expect(await fetchFrame("chukcut-frame://preview/4", 3)).toEqual({
      status: "error",
      message: "protocol handler is gone",
    });
  });
});

describe("choosing a proxy resolution", () => {
  it("follows the table in the pipeline doc", () => {
    expect(proxyResolution({ width: 640, height: 360 })).toEqual({ width: 640, height: 360 });
    expect(proxyResolution({ width: 1080, height: 1080 })).toEqual({ width: 720, height: 720 });
    expect(proxyResolution({ width: 1920, height: 1080 })).toEqual({ width: 960, height: 540 });
    expect(proxyResolution({ width: 3840, height: 2160 })).toEqual({ width: 1080, height: 608 });
  });

  it("caps the long edge whichever way round the canvas is", () => {
    // A vertical editor's whole point: 1080×1920 must proxy like 1920×1080 does.
    expect(proxyResolution({ width: 1080, height: 1920 })).toEqual({ width: 540, height: 960 });
    expect(proxyResolution({ width: 1920, height: 1080 })).toEqual({ width: 960, height: 540 });
  });

  it("keeps the aspect ratio within a rounding error", () => {
    for (const canvas of [
      { width: 1080, height: 1920 },
      { width: 1920, height: 1080 },
      { width: 2560, height: 1440 },
      { width: 1440, height: 1080 },
      { width: 4096, height: 2160 },
    ]) {
      const proxy = proxyResolution(canvas);
      const wanted = canvas.width / canvas.height;
      expect(Math.abs(proxy.width / proxy.height - wanted)).toBeLessThan(0.01);
    }
  });

  it("never produces a zero-sized texture", () => {
    expect(proxyResolution({ width: 1, height: 1 })).toEqual({ width: 2, height: 2 });
  });
});
