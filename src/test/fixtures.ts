/**
 * Documents and responses shaped like the ones Rust sends.
 *
 * Written as literals rather than builders wherever the shape itself is the
 * thing under test — a factory that spreads a partial can hide a renamed field,
 * which is precisely the bug class these fixtures exist to catch. The builders
 * below only fill in the parts a test does not care about.
 */

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
    materials: { videos: [], audios: [], images: [], texts: [], extras: {} },
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
