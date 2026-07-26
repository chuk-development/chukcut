import { describe, expect, it } from "vitest";

import type { WaveformData } from "@/modules/media/lib/api";
import { MIN_WAVEFORM_BUCKETS } from "@/modules/media/lib/waveform";
import { bucketsForClip, sourceDensity, waveformColumns } from "@/modules/timeline/lib/waveform";

const SECOND = 1_000_000;

/**
 * Eight buckets over eight seconds — one per second — with a distinct value in
 * each, so a column that reads the wrong bucket is visible in the assertion
 * rather than merely plausible.
 */
function ramp(): WaveformData {
  const values = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
  return {
    buckets: 8,
    min: Float32Array.from(values.map((v) => -v)),
    max: Float32Array.from(values),
    rms: Float32Array.from(values.map((v) => v / 2)),
    duration: 8 * SECOND,
  };
}

const CLIP = {
  data: ramp(),
  materialDuration: 8 * SECOND,
  sourceStart: 0,
  speed: 1,
};

describe("how much source a pixel covers", () => {
  it("is the zoom, divided by how fast the clip plays", () => {
    expect(sourceDensity(1e-4, 1)).toBeCloseTo(1e-4);
    // At double speed a pixel covers twice as much source, so the same waveform
    // is drawn into half the width.
    expect(sourceDensity(1e-4, 2)).toBeCloseTo(5e-5);
  });

  it("treats a nonsense speed as normal rather than dividing by zero", () => {
    expect(sourceDensity(1e-4, 0)).toBeCloseTo(1e-4);
  });
});

describe("mapping buckets onto pixels", () => {
  it("gives one column per pixel, starting where it was asked to", () => {
    const columns = waveformColumns({ ...CLIP, zoom: 1e-6, fromPx: 3, widthPx: 4 });

    expect(columns.map((column) => column.x)).toEqual([3, 4, 5, 6]);
  });

  it("reads one bucket per column when a pixel is a second", () => {
    // 1e-6 px per micro is one pixel per second, and the material is one bucket
    // per second: the clip is eight pixels wide and reads the ramp straight off.
    const columns = waveformColumns({ ...CLIP, zoom: 1e-6, fromPx: 0, widthPx: 8 });

    expect(columns.map((column) => Number(column.max.toFixed(4)))).toEqual([
      0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8,
    ]);
    expect(columns.map((column) => Number(column.min.toFixed(4)))).toEqual([
      -0.1, -0.2, -0.3, -0.4, -0.5, -0.6, -0.7, -0.8,
    ]);
  });

  it("holds a bucket across several columns when the data is coarser than the pixels", () => {
    // Zoomed in four times: four pixels per second, one bucket per second.
    const columns = waveformColumns({ ...CLIP, zoom: 4e-6, fromPx: 0, widthPx: 8 });

    // A held value says "this is all that was measured"; interpolating would
    // invent detail that is not in the file.
    expect(columns.map((column) => Number(column.max.toFixed(4)))).toEqual([
      0.1, 0.1, 0.1, 0.1, 0.2, 0.2, 0.2, 0.2,
    ]);
  });

  it("takes the extremes of every bucket a column spans when zoomed out", () => {
    // One pixel per two seconds: each column covers two buckets.
    const columns = waveformColumns({ ...CLIP, zoom: 5e-7, fromPx: 0, widthPx: 4 });

    // Extremes, not averages: a drum hit smeared into the silence around it
    // stops lining up with what the user hears.
    expect(columns.map((column) => Number(column.max.toFixed(4)))).toEqual([0.2, 0.4, 0.6, 0.8]);
    expect(columns.map((column) => Number(column.min.toFixed(4)))).toEqual([
      -0.2, -0.4, -0.6, -0.8,
    ]);
  });

  it("aggregates the body as a quadratic mean, not an arithmetic one", () => {
    const columns = waveformColumns({ ...CLIP, zoom: 5e-7, fromPx: 0, widthPx: 1 });

    // Buckets 0 and 1 carry RMS 0.05 and 0.10. The RMS of the pair is
    // sqrt((0.05² + 0.10²) / 2) = 0.0790…, not their average of 0.075.
    expect(columns[0].rms).toBeCloseTo(Math.sqrt((0.05 ** 2 + 0.1 ** 2) / 2), 5);
  });

  it("reads from the part of the file the clip actually shows", () => {
    // A clip trimmed to start four seconds in must not restart the waveform.
    const columns = waveformColumns({
      ...CLIP,
      sourceStart: 4 * SECOND,
      zoom: 1e-6,
      fromPx: 0,
      widthPx: 4,
    });

    expect(columns.map((column) => Number(column.max.toFixed(4)))).toEqual([0.5, 0.6, 0.7, 0.8]);
  });

  it("advances through the source faster on a clip that plays faster", () => {
    // At double speed, one pixel is two seconds of source even though the zoom
    // says one — the same eight seconds of audio in four pixels.
    const columns = waveformColumns({ ...CLIP, speed: 2, zoom: 1e-6, fromPx: 0, widthPx: 4 });

    expect(columns.map((column) => Number(column.max.toFixed(4)))).toEqual([0.2, 0.4, 0.6, 0.8]);
  });

  it("draws several pixels per column when asked to", () => {
    const columns = waveformColumns({ ...CLIP, zoom: 1e-6, fromPx: 0, widthPx: 8, step: 2 });

    expect(columns.map((column) => column.x)).toEqual([0, 2, 4, 6]);
    expect(columns.map((column) => Number(column.max.toFixed(4)))).toEqual([0.2, 0.4, 0.6, 0.8]);
  });

  it("clamps rather than reading past either end of the strip", () => {
    const columns = waveformColumns({
      ...CLIP,
      sourceStart: 7 * SECOND,
      zoom: 1e-6,
      fromPx: 0,
      widthPx: 4,
    });

    // Past the end of the material there is nothing to read; repeating the last
    // bucket is better than an undefined that draws as NaN.
    expect(columns.map((column) => Number(column.max.toFixed(4)))).toEqual([0.8, 0.8, 0.8, 0.8]);
  });

  it("draws an outline and no body when Rust sent peaks only", () => {
    const columns = waveformColumns({
      ...CLIP,
      data: { ...ramp(), rms: null },
      zoom: 1e-6,
      fromPx: 0,
      widthPx: 2,
    });

    expect(columns.every((column) => column.rms === 0)).toBe(true);
    expect(columns[0].max).toBeCloseTo(0.1);
  });

  it("draws nothing rather than dividing by zero", () => {
    expect(waveformColumns({ ...CLIP, zoom: 0, fromPx: 0, widthPx: 8 })).toEqual([]);
    expect(
      waveformColumns({ ...CLIP, materialDuration: 0, zoom: 1e-6, fromPx: 0, widthPx: 8 }),
    ).toEqual([]);
    expect(waveformColumns({ ...CLIP, zoom: 1e-6, fromPx: 0, widthPx: 0 })).toEqual([]);
    expect(
      waveformColumns({
        ...CLIP,
        data: {
          buckets: 0,
          min: new Float32Array(),
          max: new Float32Array(),
          rms: null,
          duration: 0,
        },
        zoom: 1e-6,
        fromPx: 0,
        widthPx: 8,
      }),
    ).toEqual([]);
  });
});

describe("choosing a resolution for a clip", () => {
  it("asks for enough buckets to cover the whole file at this zoom", () => {
    // Ten minutes at 100 px/s is sixty thousand pixels, which is past the top
    // of the ladder — the cap is what stops a decode nobody can see.
    expect(bucketsForClip({ materialDuration: 600 * SECOND, zoom: 1e-4, speed: 1 })).toBe(16_384);
    // Eight seconds at 100 px/s is 800 pixels.
    expect(bucketsForClip({ materialDuration: 8 * SECOND, zoom: 1e-4, speed: 1 })).toBe(1024);
  });

  it("asks for less when the clip plays faster, because it draws narrower", () => {
    expect(bucketsForClip({ materialDuration: 8 * SECOND, zoom: 1e-4, speed: 2 })).toBe(512);
  });

  it("falls back to the coarsest rung for material with no length", () => {
    expect(bucketsForClip({ materialDuration: 0, zoom: 1e-4, speed: 1 })).toBe(
      MIN_WAVEFORM_BUCKETS,
    );
  });
});
