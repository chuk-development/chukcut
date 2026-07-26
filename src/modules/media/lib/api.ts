/**
 * Typed wrappers around the `media_*` commands.
 *
 * `media_thumbnails` returns paths into the on-disk thumbnail cache. A webview
 * cannot load a bare filesystem path, so each one goes through
 * `convertFileSrc` before it reaches an `<img>` — that is what the asset
 * protocol is for.
 *
 * ## The two streaming commands
 *
 * `media_thumbnails` and `media_waveform` each have exactly **one** call site
 * here and one reader — `readBatch` and `readWaveform` — that turns the wire
 * shape into what the rest of the frontend is written against. Nothing above
 * them knows what serde produced.
 *
 * - **`media_thumbnails(path, count, height, on_batch: Channel<ThumbnailBatch>)`**
 *   answers with a **job id, not a strip**: it returns as soon as the arguments
 *   are known to be sane, and the tiles arrive on the channel as they are
 *   produced. Each batch carries whole tiles with their own index, because
 *   workers finish out of order and a strip with cached tiles interleaved
 *   cannot be expressed as a range. Every job ends with exactly one message
 *   carrying `complete`, and **that message is where a failure is reported** —
 *   the call itself resolved long before the decoder got there, so a caller
 *   that only handles the promise never learns that anything went wrong.
 * - **`media_thumbnails_cancel(jobId)`** stops a running job. Dropping the
 *   channel also stops it, but only once Rust next tries to send; the explicit
 *   call is what stops a decode while the component is still alive.
 * - **`media_waveform(path, buckets)`** answers with
 *   `{ buckets, duration, min, max, rms }` — per-bucket extremes *and* RMS, at
 *   the resolution asked for, cached on disk so changing zoom re-reduces an
 *   array rather than re-reading the file. `min <= 0 <= max` is guaranteed by
 *   construction, so a column can be anchored to the centre line.
 */

import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";

import type { MediaInfo } from "@/modules/media/types";
import type { Micros } from "@/modules/project/types";

export function mediaProbe(path: string): Promise<MediaInfo> {
  return invoke<MediaInfo>("media_probe", { path });
}

// ---------------------------------------------------------------------------
// Thumbnails
// ---------------------------------------------------------------------------

/** One frame of a strip, at the position in the file it was taken from. */
export interface ThumbnailTile {
  /** Slot within the strip of `count`. */
  index: number;
  /** Where in the source file this frame is. */
  at: Micros;
  /** Webview-loadable URL for the cached image. */
  url: string;
}

/**
 * One instalment of a filmstrip.
 *
 * Whole tiles rather than a contiguous run: workers finish out of order, and a
 * strip whose cached tiles are interleaved with freshly decoded ones cannot be
 * expressed as a range.
 */
export interface ThumbnailBatch {
  jobId: string;
  /** How many tiles the finished strip will have. */
  total: number;
  /** Tiles produced so far, **cumulative across the job**. Not a completion flag. */
  produced: number;
  tiles: ThumbnailTile[];
  /** The last message of the job. Exactly one message per job carries it. */
  complete: boolean;
  /** The job stopped because it was asked to, which is not a failure. */
  cancelled: boolean;
  /** Why the job stopped early. Only ever set on the terminal message. */
  error: string | null;
}

/**
 * Start rendering a strip of `count` thumbnails, `height` pixels tall, and
 * return the job id to cancel it with.
 *
 * Resolves as soon as Rust has accepted the arguments — long before the tiles
 * exist. The strip arrives through `onBatch`, and so does the news that it will
 * not: **a decode failure is a field on the terminal batch, never a rejection
 * of this promise.** Only an argument the command refuses outright rejects here.
 *
 * Never call this from a render; go through the thumbnail store, which queues
 * the work, publishes each batch as it lands and owns the cancellation.
 */
export async function mediaThumbnails(
  path: string,
  count: number,
  height: number,
  onBatch: (batch: ThumbnailBatch) => void,
): Promise<string> {
  const channel = new Channel<unknown>();
  // Registered before the invoke: Rust spawns the job after answering, so the
  // first batch can be delivered before the job id gets back here.
  channel.onmessage = (message) => {
    const batch = readBatch(message);
    if (batch) onBatch(batch);
  };

  const jobId = await invoke<string>("media_thumbnails", { path, count, height, onBatch: channel });
  return typeof jobId === "string" ? jobId : "";
}

/**
 * Ask a running thumbnail job to stop.
 *
 * `false` means there was no such job, which is the ordinary outcome of a clip
 * unmounting just as its strip finished. Not an error, and not worth telling
 * the user about: it is a race with no loser.
 */
export function mediaThumbnailsCancel(jobId: string): Promise<boolean> {
  return invoke<boolean>("media_thumbnails_cancel", { jobId });
}

/** One channel message, as a batch. `null` for anything that is not one. */
function readBatch(raw: unknown): ThumbnailBatch | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const record = raw as Record<string, unknown>;

  const tiles: ThumbnailTile[] = [];
  if (Array.isArray(record.tiles)) {
    for (const entry of record.tiles) {
      if (!entry || typeof entry !== "object") continue;
      const tile = entry as Record<string, unknown>;
      if (typeof tile.path !== "string") continue;
      tiles.push({
        index: numberOr(tile.index, 0),
        at: numberOr(tile.at, 0),
        // A bare filesystem path in an <img src> loads nothing.
        url: convertFileSrc(tile.path),
      });
    }
  }

  const error = typeof record.error === "string" && record.error ? record.error : null;
  const complete = Boolean(record.complete);
  // A message with no tiles, no ending and nothing wrong in it says nothing.
  if (tiles.length === 0 && !complete && !error) return null;

  return {
    jobId: typeof record.job_id === "string" ? record.job_id : "",
    total: numberOr(record.total, 0),
    produced: numberOr(record.produced, 0),
    tiles,
    complete,
    cancelled: Boolean(record.cancelled),
    error,
  };
}

// ---------------------------------------------------------------------------
// Waveforms
// ---------------------------------------------------------------------------

/**
 * A file's audio, reduced to `buckets` equal slices.
 *
 * Typed arrays rather than plain ones: a minute of audio at frame-level zoom is
 * sixteen thousand buckets, three arrays of them per material, and every one of
 * those lives for as long as the material is on the timeline.
 */
export interface WaveformData {
  /** How many slices the material was cut into. */
  buckets: number;
  /** Most negative sample in each slice, `-1..0`. */
  min: Float32Array;
  /** Most positive sample in each slice, `0..1`. */
  max: Float32Array;
  /** Root-mean-square of each slice, `0..1`. Null when Rust sent peaks only. */
  rms: Float32Array | null;
  /** Length of the audio the buckets span. Zero when Rust did not say. */
  duration: Micros;
}

/**
 * Reduce a file's audio to `buckets` slices for waveform drawing.
 *
 * One bucket per horizontal pixel the material would occupy is the intended
 * call shape; asking for far more than can be drawn costs decode time and buys
 * nothing the user can see.
 */
export async function mediaWaveform(path: string, buckets: number): Promise<WaveformData> {
  return readWaveform(await invoke<unknown>("media_waveform", { path, buckets }));
}

const EMPTY_WAVEFORM: WaveformData = {
  buckets: 0,
  min: new Float32Array(0),
  max: new Float32Array(0),
  rms: null,
  duration: 0,
};

function readWaveform(raw: unknown): WaveformData {
  // The old signature: normalized peaks, one per bucket. An envelope symmetric
  // about zero is the honest reading — there is no RMS in there to draw, and
  // deriving one from the peak would paint a body nobody measured.
  if (Array.isArray(raw)) {
    const max = toFloats(raw);
    const min = new Float32Array(max.length);
    for (let i = 0; i < max.length; i++) {
      max[i] = Math.abs(max[i]);
      min[i] = -max[i];
    }
    return { buckets: max.length, min, max, rms: null, duration: 0 };
  }

  if (!raw || typeof raw !== "object") return EMPTY_WAVEFORM;
  const record = raw as Record<string, unknown>;
  const duration = numberOr(record.duration, 0);

  // Interleaved triples, if that is how a bucket arrives.
  if (Array.isArray(record.peaks) && !Array.isArray(record.min)) {
    const flat = toFloats(record.peaks);
    const buckets = Math.floor(flat.length / 3);
    const min = new Float32Array(buckets);
    const max = new Float32Array(buckets);
    const rms = new Float32Array(buckets);
    for (let i = 0; i < buckets; i++) {
      min[i] = flat[i * 3];
      max[i] = flat[i * 3 + 1];
      rms[i] = flat[i * 3 + 2];
    }
    return { buckets, min, max, rms, duration };
  }

  if (!Array.isArray(record.min) || !Array.isArray(record.max)) return EMPTY_WAVEFORM;
  const min = toFloats(record.min);
  const max = toFloats(record.max);
  const buckets = Math.min(min.length, max.length);
  const rms = Array.isArray(record.rms) ? toFloats(record.rms) : null;

  return {
    buckets,
    min: min.subarray(0, buckets),
    max: max.subarray(0, buckets),
    // A short RMS array is worse than none: it would draw a body for the first
    // part of the clip and nothing for the rest, which reads as a bug.
    rms: rms && rms.length >= buckets ? rms.subarray(0, buckets) : null,
    duration,
  };
}

function toFloats(values: unknown[]): Float32Array {
  const out = new Float32Array(values.length);
  for (let i = 0; i < values.length; i++) {
    const value = values[i];
    out[i] = typeof value === "number" && Number.isFinite(value) ? value : 0;
  }
  return out;
}

function numberOr(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}
