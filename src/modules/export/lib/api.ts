/**
 * The export command surface.
 *
 * ## Casing
 *
 * Everything here is **snake_case**, unlike the preview module. The export
 * structs in `src-tauri/src/modules/export/` carry no `rename_all` attribute on
 * their fields, so serde writes Rust's own names: `output_path`, `preset_id`,
 * `include_audio`, `total_frames`. Only the *arguments* of the commands
 * themselves are camelCase, because Tauri maps those onto the snake_case
 * parameters — hence `onProgress` and `jobId` below and nothing else.
 *
 * Enum variants are a third case again: `#[serde(rename_all = "snake_case")]`
 * turns `MixingAudio` into `"mixing_audio"` and `VideoToolbox` into
 * `"video_toolbox"`.
 */

import { Channel, invoke } from "@tauri-apps/api/core";

/** An exact frame rate, `num / den`. 29.97 is 30000/1001, not a decimal. */
export interface Fps {
  num: number;
  den: number;
}

export type VideoCodec = "h264" | "h265" | "vp9" | "av1";
export type AudioCodec = "aac" | "opus" | "none";
export type Container = "mp4" | "mov" | "mkv" | "webm";

/**
 * Adjacently tagged: `#[serde(tag = "kind", content = "value")]`. Two modes,
 * not two spellings of one — CRF targets a look, bitrate targets a size.
 */
export type Quality = { kind: "crf"; value: number } | { kind: "bitrate"; value: number };

export type HwAccel =
  | "software"
  | "vaapi"
  | "qsv"
  | "nvenc"
  | "video_toolbox"
  | "amf"
  | "media_foundation";

export interface ExportPreset {
  id: string;
  label: string;
  description: string;
  width: number;
  height: number;
  fps: Fps;
  video_codec: VideoCodec;
  quality: Quality;
  audio_codec: AudioCodec;
  audio_bitrate: number;
  sample_rate: number;
  container: Container;
}

export interface HwEncoder {
  id: string;
  accel: HwAccel;
  codec: VideoCodec;
  encoder_name: string;
  label: string;
  /** In this FFmpeg build and backed by a device we can see. */
  available: boolean;
  /** Available *and* something the encoder path can drive today. */
  usable: boolean;
  /** Why it cannot be used, in Rust's own words. Present exactly when `usable` is false. */
  note: string | null;
}

/** Everything the dialog needs to draw itself, in one round trip. */
export interface ExportOptions {
  presets: ExportPreset[];
  /** Empty is the normal case. */
  hardware: HwEncoder[];
  default_preset_id: string;
}

/** Per-field changes on top of a preset. Anything absent keeps the preset's value. */
export interface ExportOverrides {
  width?: number;
  height?: number;
  /** A decimal; Rust snaps it back onto an exact fraction. */
  fps?: number;
  video_codec?: VideoCodec;
  quality?: Quality;
  audio_codec?: AudioCodec;
  audio_bitrate?: number;
  sample_rate?: number;
  container?: Container;
}

export interface ExportRequest {
  /** Rust corrects the extension to match the container. */
  output_path: string;
  preset_id: string | null;
  overrides: ExportOverrides | null;
  /** A `HwEncoder.id`. Null means software, which is the deliberate default. */
  hardware: string | null;
  include_audio: boolean;
}

export type ExportStage =
  | "preparing"
  | "mixing_audio"
  | "encoding"
  | "finalizing"
  | "done"
  | "cancelled"
  | "failed";

export interface ExportProgress {
  job_id: string;
  stage: ExportStage;
  /** Frames finished so far. */
  frame: number;
  total_frames: number;
  /** Already clamped to 0..1 by Rust. */
  fraction: number;
  /** Frames per second achieved, not the output frame rate. */
  fps: number;
  elapsed_seconds: number;
  /** Null until the first frame is done — an ETA from no samples is invented. */
  remaining_seconds: number | null;
  /** Set on the terminal message, so the UI can offer "show in folder". */
  output_path: string | null;
  /** User-facing prose on failure. Shown verbatim. */
  message: string | null;
}

export function exportPresets(): Promise<ExportOptions> {
  return invoke<ExportOptions>("export_presets");
}

/** Returns the job id. Resolves as soon as the settings are known to be valid. */
export function exportStart(
  request: ExportRequest,
  onProgress: Channel<ExportProgress>,
): Promise<string> {
  return invoke<string>("export_start", { request, onProgress });
}

/**
 * Ask a running export to stop.
 *
 * `false` means it had already finished, which is not an error: closing the
 * card on the last frame is a race nobody can win.
 */
export function exportCancel(jobId: string): Promise<boolean> {
  return invoke<boolean>("export_cancel", { jobId });
}

export function newProgressChannel(): Channel<ExportProgress> {
  return new Channel<ExportProgress>();
}
