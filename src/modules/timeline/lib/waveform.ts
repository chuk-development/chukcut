/**
 * Turning a material's waveform into the columns of one clip.
 *
 * The buckets Rust sends span the **whole file**, in source time. A clip shows
 * a window into that file, at a speed, at a zoom — so the mapping from a
 * horizontal pixel to a range of buckets is the only interesting piece of
 * arithmetic in the waveform, and it is the piece that breaks silently: get it
 * wrong and the picture still looks like a waveform, it is just not this clip's.
 *
 * Kept pure and out of the component so it can be checked at several zoom
 * levels without a canvas.
 */

import type { WaveformData } from "@/modules/media/lib/api";
import { waveformResolution } from "@/modules/media/lib/waveform";
import type { Micros } from "@/modules/project/types";

/** One drawn column: the extremes it has to span, and the body inside them. */
export interface WaveformColumn {
  /** Pixels from the clip's left edge. */
  x: number;
  /** `-1..0`. */
  min: number;
  /** `0..1`. */
  max: number;
  /** `0..1`. Zero when Rust sent no RMS, which draws as an outline. */
  rms: number;
}

/**
 * How much of the source one timeline pixel covers, expressed the other way up:
 * pixels per microsecond of *source*.
 *
 * A clip `d` long on the timeline is `d × zoom` pixels wide and shows `d ×
 * speed` of source, so the source density is `zoom / speed` and the clip's own
 * duration cancels out. A clip at double speed draws half as much waveform per
 * pixel, which is exactly what "the audio goes past twice as fast" looks like.
 */
export function sourceDensity(zoom: number, speed: number): number {
  return zoom / (speed > 0 ? speed : 1);
}

/**
 * How many buckets the whole material needs for this clip to be drawn one
 * column per pixel.
 *
 * Asked for the *material*, not for the clip, because the cache is keyed by
 * material: two clips from the same file at different speeds share one strip,
 * and the finer of their two demands is the one that gets fetched.
 */
export function bucketsForClip(options: {
  materialDuration: Micros;
  zoom: number;
  speed: number;
}): number {
  const { materialDuration, zoom } = options;
  if (materialDuration <= 0 || zoom <= 0) return waveformResolution(0);
  return waveformResolution(materialDuration * sourceDensity(zoom, options.speed));
}

export interface ColumnOptions {
  data: WaveformData;
  /** Length of the whole source file, which the buckets span. */
  materialDuration: Micros;
  /** `segment.source_range.start` — where in the file this clip begins. */
  sourceStart: Micros;
  speed: number;
  /** Pixels per timeline microsecond. */
  zoom: number;
  /** First pixel to draw, measured from the clip's left edge. */
  fromPx: number;
  /** How many pixels to draw. */
  widthPx: number;
  /** Pixels per column. One is the point; two halves the work at high zoom. */
  step?: number;
}

/**
 * The columns for a slice of a clip.
 *
 * Each column covers the buckets its pixel spans, so zooming out aggregates —
 * `min` and `max` take the extremes, and the body takes the root mean square of
 * the squares rather than their average, because RMS is a quadratic mean and
 * averaging two RMS values is not the RMS of the pair.
 *
 * When the data is coarser than the pixels, several columns land in one bucket
 * and repeat it. That is honest: a held value says "this is all we know", where
 * interpolating would invent detail that was never measured.
 */
export function waveformColumns(options: ColumnOptions): WaveformColumn[] {
  const { data, materialDuration, sourceStart, zoom, fromPx, widthPx } = options;
  const step = Math.max(1, Math.floor(options.step ?? 1));
  const speed = options.speed > 0 ? options.speed : 1;

  if (data.buckets <= 0 || materialDuration <= 0 || zoom <= 0 || widthPx <= 0) return [];

  const perBucket = materialDuration / data.buckets;
  const last = data.buckets - 1;
  const columns: WaveformColumn[] = [];

  for (let x = fromPx; x < fromPx + widthPx; x += step) {
    const from = sourceStart + (x / zoom) * speed;
    const to = sourceStart + ((x + step) / zoom) * speed;

    const first = clamp(Math.floor(from / perBucket), 0, last);
    // `ceil - 1` rather than `floor`, so a column ending exactly on a boundary
    // does not claim the bucket after it.
    const until = clamp(Math.max(first, Math.ceil(to / perBucket) - 1), 0, last);

    let min = 0;
    let max = 0;
    let sumSquares = 0;
    for (let bucket = first; bucket <= until; bucket++) {
      if (data.min[bucket] < min) min = data.min[bucket];
      if (data.max[bucket] > max) max = data.max[bucket];
      if (data.rms) sumSquares += data.rms[bucket] * data.rms[bucket];
    }

    const spanned = until - first + 1;
    columns.push({
      x,
      min,
      max,
      rms: data.rms ? Math.sqrt(sumSquares / spanned) : 0,
    });
  }

  return columns;
}

function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value));
}
