/**
 * The project document, as it crosses IPC.
 *
 * These types mirror `src-tauri/src/modules/project/document.rs` field for
 * field. Serde is not configured to rename struct fields, so the JSON keys are
 * the Rust names — snake_case — and they stay snake_case here rather than being
 * prettified into camelCase. A translation layer would be one more place for
 * the two halves to drift apart.
 */

/** Microseconds. Every time value in the document, without exception. */
export type Micros = number;

export type Id = string;

export const MICROS_PER_SECOND = 1_000_000;

/** RGBA, linear, 0..1. */
export type Rgba = [number, number, number, number];
export type Vec2 = [number, number];

/** Half-open interval `[start, start + duration)`. */
export interface TimeRange {
  start: Micros;
  duration: Micros;
}

export interface CanvasConfig {
  width: number;
  height: number;
  background: Rgba;
}

export type MaterialKind = "video" | "audio" | "image" | "text";

export interface VideoMaterial {
  id: Id;
  path: string;
  width: number;
  height: number;
  duration: Micros;
  fps: number;
  has_audio: boolean;
  rotation: number;
}

export interface AudioMaterial {
  id: Id;
  path: string;
  duration: Micros;
  sample_rate: number;
  channels: number;
}

export interface ImageMaterial {
  id: Id;
  path: string;
  width: number;
  height: number;
}

export type TextAlign = "left" | "center" | "right";

export interface TextShadow {
  color: Rgba;
  offset: Vec2;
  blur: number;
}

export interface TextMaterial {
  id: Id;
  content: string;
  font_family: string;
  font_size: number;
  color: Rgba;
  bold: boolean;
  italic: boolean;
  align: TextAlign;
  stroke_width: number;
  stroke_color: Rgba;
  shadow: TextShadow | null;
  background: Rgba | null;
}

export interface MaterialPool {
  videos: VideoMaterial[];
  audios: AudioMaterial[];
  images: ImageMaterial[];
  texts: TextMaterial[];
  /** Effect / transition / animation parameter blocks, keyed by id. */
  extras: Record<Id, unknown>;
}

export type TrackKind = "video" | "audio" | "text" | "sticker" | "effect";

export interface Transform {
  /** Normalized canvas units: `[0,0]` is the centre, `1` is half a dimension. */
  position: Vec2;
  scale: Vec2;
  /** Degrees, clockwise. */
  rotation: number;
  opacity: number;
  flip_h: boolean;
  flip_v: boolean;
}

/** Source-space crop, as fractions of the source dimensions. */
export interface Crop {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export type AnimatableProperty =
  | "position_x"
  | "position_y"
  | "scale_x"
  | "scale_y"
  | "rotation"
  | "opacity"
  | "volume";

export type Easing = "hold" | "linear" | "ease_in" | "ease_out" | "ease_in_out";

export interface Keyframe {
  /** Relative to the segment start, not to the timeline. */
  time: Micros;
  value: number;
  easing: Easing;
}

export interface KeyframeTrack {
  property: AnimatableProperty;
  keyframes: Keyframe[];
}

export interface Segment {
  id: Id;
  material_id: Id;
  /** Where the segment sits on the timeline. */
  target_range: TimeRange;
  /** Which part of the material it shows. */
  source_range: TimeRange;
  render_index: number;
  speed: number;
  volume: number;
  transform: Transform;
  crop: Crop | null;
  extras: Id[];
  keyframes: KeyframeTrack[];
}

export interface Track {
  id: Id;
  kind: TrackKind;
  name: string;
  segments: Segment[];
  muted: boolean;
  locked: boolean;
  hidden: boolean;
  volume: number;
}

export interface Project {
  id: Id;
  schema_version: number;
  name: string;
  /** Unix millis. */
  created_at: number;
  updated_at: number;
  canvas: CanvasConfig;
  fps: number;
  materials: MaterialPool;
  tracks: Track[];
}

/**
 * What `project_import_media` answers with: enough to render a library tile and
 * to build the segment that will reference the material.
 *
 * `width`/`height` are *display* dimensions — container rotation has already
 * been applied, so the UI never has to know that a portrait phone video is
 * coded landscape.
 */
export interface ImportedMaterial {
  id: Id;
  kind: MaterialKind;
  /** File name without the directory. */
  name: string;
  path: string;
  /** Zero for stills. */
  duration: Micros;
  width: number;
  height: number;
  has_audio: boolean;
}

export type Severity = "warning" | "error";

export interface ValidationIssue {
  severity: Severity;
  message: string;
  subject_id: Id | null;
}

// ---------------------------------------------------------------------------
// Derived reads
//
// The document is server state and is never patched locally, so anything the UI
// needs that is not literally a field is computed here rather than cached.
// ---------------------------------------------------------------------------

export function rangeEnd(range: TimeRange): Micros {
  return range.start + range.duration;
}

export function projectDuration(project: Project): Micros {
  let end = 0;
  for (const track of project.tracks) {
    for (const segment of track.segments) {
      end = Math.max(end, rangeEnd(segment.target_range));
    }
  }
  return end;
}

export function findSegment(
  project: Project,
  segmentId: Id,
): { track: Track; segment: Segment; index: number } | null {
  for (const track of project.tracks) {
    const index = track.segments.findIndex((s) => s.id === segmentId);
    if (index !== -1) {
      return { track, segment: track.segments[index], index };
    }
  }
  return null;
}

/** What to write on a clip. Falls back to the material id so a broken reference is visible rather than blank. */
export function segmentLabel(project: Project, segment: Segment): string {
  const { videos, audios, images, texts } = project.materials;
  const video = videos.find((m) => m.id === segment.material_id);
  if (video) return basename(video.path);
  const audio = audios.find((m) => m.id === segment.material_id);
  if (audio) return basename(audio.path);
  const image = images.find((m) => m.id === segment.material_id);
  if (image) return basename(image.path);
  const text = texts.find((m) => m.id === segment.material_id);
  if (text) return text.content || "Text";
  return segment.material_id;
}

/** Where a material's file lives, for thumbnails. Text materials have no file. */
export function materialPath(project: Project, materialId: Id): string | null {
  const { videos, audios, images } = project.materials;
  return (
    videos.find((m) => m.id === materialId)?.path ??
    audios.find((m) => m.id === materialId)?.path ??
    images.find((m) => m.id === materialId)?.path ??
    null
  );
}

/** Display aspect ratio of a material, for sizing filmstrip tiles. */
export function materialAspect(project: Project, materialId: Id): number | null {
  const video = project.materials.videos.find((m) => m.id === materialId);
  if (video) {
    // Coded dimensions; a rotated clip reports its display size swapped.
    const rotated = Math.abs(video.rotation % 180) === 90;
    const width = rotated ? video.height : video.width;
    const height = rotated ? video.width : video.height;
    return height > 0 ? width / height : null;
  }
  const image = project.materials.images.find((m) => m.id === materialId);
  if (image && image.height > 0) return image.width / image.height;
  return null;
}

/** Full length of the source file, when that is knowable. */
export function materialDuration(project: Project, materialId: Id): Micros | null {
  const video = project.materials.videos.find((m) => m.id === materialId);
  if (video) return video.duration;
  const audio = project.materials.audios.find((m) => m.id === materialId);
  if (audio) return audio.duration;
  // Images and text are generated: they stretch to whatever the segment needs.
  return null;
}

export function basename(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}
