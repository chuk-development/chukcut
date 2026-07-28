/**
 * The library as a view of the project's material pool.
 *
 * The pool is the source of truth: import writes it, saving persists it, and
 * the timeline resolves `material_id` against it. The library therefore
 * *derives* its rows from the document rather than keeping a session list —
 * which is exactly the bug this replaces: a saved and reopened project had a
 * full pool and an empty panel, because the panel only knew what this session
 * had imported.
 */

import type { Id, ImportedMaterial, Project } from "@/modules/project/types";
import { basename } from "@/modules/project/types";
import type { EditCommand } from "@/modules/timeline/lib/api";

/** Every file-backed material in the pool, as library rows. Texts stay in their own tab. */
export function libraryItems(project: Project | null): ImportedMaterial[] {
  if (!project) return [];
  const items: ImportedMaterial[] = [];

  for (const video of project.materials.videos) {
    // Display dimensions, as `project_import_media` answers them: container
    // rotation applied, so a portrait phone clip reads as portrait.
    const rotated = Math.abs(video.rotation % 180) === 90;
    items.push({
      id: video.id,
      kind: "video",
      name: basename(video.path),
      path: video.path,
      duration: video.duration,
      width: rotated ? video.height : video.width,
      height: rotated ? video.width : video.height,
      has_audio: video.has_audio,
    });
  }
  for (const audio of project.materials.audios) {
    items.push({
      id: audio.id,
      kind: "audio",
      name: basename(audio.path),
      path: audio.path,
      duration: audio.duration,
      width: 0,
      height: 0,
      has_audio: true,
    });
  }
  for (const image of project.materials.images) {
    items.push({
      id: image.id,
      kind: "image",
      name: basename(image.path),
      path: image.path,
      duration: 0,
      width: image.width,
      height: image.height,
      has_audio: false,
    });
  }
  return items;
}

/** Every file path the pool references, for the missing-on-disk check. */
export function libraryPaths(project: Project | null): string[] {
  return libraryItems(project).map((item) => item.path);
}

/** How many clips on the timeline reference `materialId` — what "Remove" will send offline. */
export function materialUseCount(project: Project, materialId: Id): number {
  let count = 0;
  for (const track of project.tracks) {
    for (const segment of track.segments) {
      if (segment.material_id === materialId) count += 1;
    }
  }
  return count;
}

/**
 * The `remove_material` edit for a library row, or `null` when the id is not
 * a file-backed pool entry.
 *
 * The whole material and its index travel so Rust can undo exactly and refuse
 * a stale gesture; see `PoolMaterial` in `timeline/lib/api.ts`.
 */
export function removeMaterialCommand(project: Project, materialId: Id): EditCommand | null {
  const { videos, audios, images } = project.materials;

  const videoIndex = videos.findIndex((m) => m.id === materialId);
  if (videoIndex !== -1) {
    return {
      type: "remove_material",
      material: { kind: "video", ...videos[videoIndex] },
      index: videoIndex,
    };
  }
  const audioIndex = audios.findIndex((m) => m.id === materialId);
  if (audioIndex !== -1) {
    return {
      type: "remove_material",
      material: { kind: "audio", ...audios[audioIndex] },
      index: audioIndex,
    };
  }
  const imageIndex = images.findIndex((m) => m.id === materialId);
  if (imageIndex !== -1) {
    return {
      type: "remove_material",
      material: { kind: "image", ...images[imageIndex] },
      index: imageIndex,
    };
  }
  return null;
}
