/**
 * Typed wrappers around the `media_*` commands.
 *
 * `media_thumbnails` returns paths into the on-disk thumbnail cache. A webview
 * cannot load a bare filesystem path, so each one goes through
 * `convertFileSrc` before it reaches an `<img>` — that is what the asset
 * protocol is for.
 */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";

import type { MediaInfo } from "@/modules/media/types";

export function mediaProbe(path: string): Promise<MediaInfo> {
  return invoke<MediaInfo>("media_probe", { path });
}

/**
 * A strip of `count` thumbnails, `height` pixels tall, spread across the file
 * in timeline order.
 *
 * Synchronous on the Rust side and slow on a cold cache, because it decodes.
 * Never call this from a render; go through the thumbnail store, which queues
 * the work and publishes the result when it lands.
 */
export async function mediaThumbnails(
  path: string,
  count: number,
  height: number,
): Promise<string[]> {
  const files = await invoke<string[]>("media_thumbnails", { path, count, height });
  return files.map((file) => convertFileSrc(file));
}

/** Normalized peaks, one per bucket. One bucket per horizontal pixel of the lane is the intended shape. */
export function mediaWaveform(path: string, buckets: number): Promise<number[]> {
  return invoke<number[]>("media_waveform", { path, buckets });
}
