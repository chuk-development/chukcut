/**
 * What `media_probe` answers with.
 *
 * Mirrors `src-tauri/src/modules/media/probe.rs`. The library does not use this
 * to import — that goes through `project_import_media`, which returns the pool
 * entry directly — but the probe is what tells you *why* a file was rejected,
 * and it carries per-stream detail the pool deliberately does not keep.
 */

import type { Micros } from "@/modules/project/types";

export interface VideoStreamInfo {
  index: number;
  /** Coded dimensions, before display rotation is applied. */
  width: number;
  height: number;
  /** Dimensions as the user expects to see them, rotation applied. */
  display_width: number;
  display_height: number;
  fps: number;
  codec: string;
  /** Degrees clockwise, one of 0/90/180/270. */
  rotation: number;
  duration: Micros;
}

export interface AudioStreamInfo {
  index: number;
  sample_rate: number;
  channels: number;
  codec: string;
  duration: Micros;
}

export interface MediaInfo {
  path: string;
  /** Demuxer short name, e.g. `"matroska,webm"`. */
  format: string;
  /** Longest stream, or 0 when the container declares none. */
  duration: Micros;
  file_size: number;
  has_video: boolean;
  has_audio: boolean;
  video: VideoStreamInfo | null;
  audio: AudioStreamInfo | null;
}
