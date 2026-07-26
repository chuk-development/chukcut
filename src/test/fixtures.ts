/**
 * Documents and responses shaped like the ones Rust sends.
 *
 * Written as literals rather than builders wherever the shape itself is the
 * thing under test — a factory that spreads a partial can hide a renamed field,
 * which is precisely the bug class these fixtures exist to catch. The builders
 * below only fill in the parts a test does not care about.
 */

import type {
  ExportOptions,
  ExportPreset,
  ExportProgress,
  HwEncoder,
} from "@/modules/export/lib/api";
import type { PreviewInfo } from "@/modules/preview/lib/api";
import type {
  ImportedMaterial,
  Project,
  Segment,
  TimeRange,
  Track,
  Transform,
} from "@/modules/project/types";
import type { EditResponse } from "@/modules/timeline/lib/api";

export const IDENTITY_TRANSFORM: Transform = {
  position: [0, 0],
  scale: [1, 1],
  rotation: 0,
  opacity: 1,
  flip_h: false,
  flip_v: false,
};

export function range(start: number, duration: number): TimeRange {
  return { start, duration };
}

export function makeSegment(id: string, overrides: Partial<Segment> = {}): Segment {
  return {
    id,
    material_id: `material-${id}`,
    target_range: range(0, 1_000_000),
    source_range: range(0, 1_000_000),
    render_index: 0,
    speed: 1,
    volume: 1,
    transform: { ...IDENTITY_TRANSFORM },
    crop: null,
    extras: [],
    keyframes: [],
    ...overrides,
  };
}

export function makeTrack(id: string, overrides: Partial<Track> = {}): Track {
  return {
    id,
    kind: "video",
    name: "Video 1",
    segments: [],
    muted: false,
    locked: false,
    hidden: false,
    volume: 1,
    ...overrides,
  };
}

export function makeProject(overrides: Partial<Project> = {}): Project {
  return {
    id: "project-1",
    schema_version: 1,
    name: "Untitled",
    created_at: 1_700_000_000_000,
    updated_at: 1_700_000_000_000,
    canvas: { width: 1080, height: 1920, background: [0, 0, 0, 1] },
    fps: 30,
    materials: {
      videos: [],
      audios: [],
      images: [],
      texts: [],
      links: [],
      transitions: [],
      extras: {},
    },
    tracks: [],
    ...overrides,
  };
}

/** A project with one video track holding `segments`, plus a matching material for each. */
export function projectWithSegments(...segments: Segment[]): Project {
  return makeProject({
    materials: {
      videos: segments.map((segment) => ({
        id: segment.material_id,
        path: `/media/${segment.material_id}.mp4`,
        width: 1920,
        height: 1080,
        duration: 10_000_000,
        fps: 30,
        has_audio: true,
        rotation: 0,
      })),
      audios: [],
      images: [],
      texts: [],
      links: [],
      transitions: [],
      extras: {},
    },
    tracks: [makeTrack("track-video", { segments })],
  });
}

export function makeEditResponse(
  project: Project,
  overrides: Partial<EditResponse> = {},
): EditResponse {
  return {
    project,
    can_undo: true,
    can_redo: false,
    undo_label: "Move clip",
    redo_label: null,
    ...overrides,
  };
}

export function makePreviewInfo(overrides: Partial<PreviewInfo> = {}): PreviewInfo {
  return {
    session: 1,
    width: 540,
    height: 960,
    quality: 80,
    fps: 30,
    duration: 10_000_000,
    position: 0,
    frame: 0,
    playing: false,
    frameUrl: "chukcut-frame://preview/1",
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Export
//
// Written out rather than built, for the same reason as the document above:
// these are the bytes `export_presets` answers with, and a builder that spread a
// partial could hide `total_frames` turning into `totalFrames`.
// ---------------------------------------------------------------------------

export const YOUTUBE_1080P: ExportPreset = {
  id: "youtube_1080p",
  label: "YouTube 1080p",
  description: "1920x1080, H.264, high quality upload",
  width: 1920,
  height: 1080,
  fps: { num: 30, den: 1 },
  video_codec: "h264",
  quality: { kind: "crf", value: 20 },
  audio_codec: "aac",
  audio_bitrate: 384_000,
  sample_rate: 48_000,
  container: "mp4",
};

export const YOUTUBE_4K: ExportPreset = {
  id: "youtube_4k",
  label: "YouTube 4K",
  description: "3840x2160, H.265, high quality upload",
  width: 3840,
  height: 2160,
  fps: { num: 30, den: 1 },
  video_codec: "h265",
  quality: { kind: "crf", value: 22 },
  audio_codec: "aac",
  audio_bitrate: 384_000,
  sample_rate: 48_000,
  container: "mp4",
};

/** The escape hatch. Rust fills width/height/fps in from the open project. */
export const CUSTOM_PRESET: ExportPreset = {
  id: "custom",
  label: "Custom",
  description: "Project canvas and frame rate",
  width: 1080,
  height: 1920,
  fps: { num: 30, den: 1 },
  video_codec: "h264",
  quality: { kind: "crf", value: 20 },
  audio_codec: "aac",
  audio_bitrate: 192_000,
  sample_rate: 48_000,
  container: "mp4",
};

export const NVENC_H264: HwEncoder = {
  id: "nvenc_h264",
  accel: "nvenc",
  codec: "h264",
  encoder_name: "h264_nvenc",
  label: "H.264 (NVIDIA NVENC)",
  available: true,
  usable: true,
  note: null,
};

/** Present in the build and backed by a device, but not drivable yet. */
export const VAAPI_H264: HwEncoder = {
  id: "vaapi_h264",
  accel: "vaapi",
  codec: "h264",
  encoder_name: "h264_vaapi",
  label: "H.264 (VAAPI)",
  available: true,
  usable: false,
  note: "VAAPI encoding needs a hardware frame pool, which is not wired up yet",
};

export function makeExportOptions(overrides: Partial<ExportOptions> = {}): ExportOptions {
  return {
    presets: [YOUTUBE_1080P, YOUTUBE_4K, CUSTOM_PRESET],
    hardware: [],
    default_preset_id: "custom",
    ...overrides,
  };
}

export function makeProgress(overrides: Partial<ExportProgress> = {}): ExportProgress {
  return {
    job_id: "job-1",
    stage: "encoding",
    frame: 0,
    total_frames: 900,
    fraction: 0,
    fps: 0,
    elapsed_seconds: 0,
    remaining_seconds: null,
    output_path: null,
    message: null,
    ...overrides,
  };
}

export function makeMaterial(
  id: string,
  overrides: Partial<ImportedMaterial> = {},
): ImportedMaterial {
  return {
    id,
    kind: "video",
    name: `${id}.mp4`,
    path: `/media/${id}.mp4`,
    duration: 4_000_000,
    width: 1920,
    height: 1080,
    has_audio: true,
    ...overrides,
  };
}
