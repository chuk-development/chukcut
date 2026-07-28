/**
 * What each clip needs to know about the file behind it, gathered once.
 *
 * The document is server state and is replaced wholesale after every edit, so
 * this is built once per document rather than looked up per clip per render.
 * Two reasons, and the second is the important one:
 *
 * - A linear search through the pool for every clip on every render is
 *   quadratic in a project that has one material per clip.
 * - The object a clip receives has to be **reference-stable** between renders,
 *   or `React.memo` on the clip is worthless and a pointer move repaints the
 *   whole timeline. Building a fresh descriptor inside the render was exactly
 *   that bug.
 *
 * Every pool category is in the index — including text, which has no file —
 * so that a clip whose lookup misses can read the miss for what it is: its
 * material was removed from the pool, and the clip is offline.
 */

import type { Id, Project } from "@/modules/project/types";
import type { ClipMaterial } from "@/modules/timeline/components/Segment";

const DEFAULT_ASPECT = 16 / 9;

export function buildMaterialIndex(
  project: Project | null,
  missingPaths: readonly string[] = [],
): Map<Id, ClipMaterial> {
  const index = new Map<Id, ClipMaterial>();
  if (!project) return index;
  const missing = new Set(missingPaths);

  for (const video of project.materials.videos) {
    // Coded dimensions; a rotated clip reports its display size swapped.
    const rotated = Math.abs(video.rotation % 180) === 90;
    const width = rotated ? video.height : video.width;
    const height = rotated ? video.width : video.height;
    index.set(video.id, {
      path: video.path,
      aspect: height > 0 ? width / height : DEFAULT_ASPECT,
      duration: video.duration,
      hasAudio: video.has_audio,
      audioOnly: false,
      missing: missing.has(video.path),
    });
  }

  for (const audio of project.materials.audios) {
    index.set(audio.id, {
      path: audio.path,
      aspect: DEFAULT_ASPECT,
      duration: audio.duration,
      hasAudio: true,
      audioOnly: true,
      missing: missing.has(audio.path),
    });
  }

  for (const image of project.materials.images) {
    index.set(image.id, {
      path: image.path,
      aspect: image.height > 0 ? image.width / image.height : DEFAULT_ASPECT,
      // A still has no length of its own: it stretches to whatever the clip
      // needs, so it can never be trimmed to a limit and has no strip to span.
      duration: 0,
      hasAudio: false,
      audioOnly: false,
      missing: missing.has(image.path),
    });
  }

  // Text has no file — nothing to decode, nothing to go missing — but the
  // entry has to exist: `material === null` is the clip's "my material was
  // removed from the pool" signal, and a title must never read as offline.
  for (const text of project.materials.texts) {
    index.set(text.id, {
      path: null,
      aspect: DEFAULT_ASPECT,
      duration: 0,
      hasAudio: false,
      audioOnly: false,
      missing: false,
    });
  }

  return index;
}
