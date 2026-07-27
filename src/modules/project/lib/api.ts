/**
 * Typed wrappers around the `project_*` commands.
 *
 * Components call these, never `invoke` directly — one place to change when a
 * signature moves, one place to look when a call misbehaves.
 */

import { invoke } from "@tauri-apps/api/core";

import type { ImportedMaterial, Project, Rgba, ValidationIssue } from "@/modules/project/types";
import type { EditResponse } from "@/modules/timeline/lib/api";

export interface NewProjectOptions {
  name: string;
  width: number;
  height: number;
  fps: number;
}

export function projectNew(options: NewProjectOptions): Promise<Project> {
  const { name, width, height, fps } = options;
  return invoke<Project>("project_new", { name, width, height, fps });
}

export function projectOpen(path: string): Promise<Project> {
  return invoke<Project>("project_open", { path });
}

/** `path` is optional; without it Rust reuses the path the project was opened from. */
export function projectSave(path?: string): Promise<string> {
  return invoke<string>("project_save", { path: path ?? null });
}

/** Mirrors `project::configure::ProjectConfig`, field for field. */
export interface ProjectConfig {
  name: string;
  width: number;
  height: number;
  fps: number;
  /** Linear RGBA 0..1, exactly as the canvas stores its background. */
  background: Rgba;
}

/**
 * Change the project's name, canvas, frame rate or background, as one undoable
 * step on the same stack as every timeline edit.
 *
 * The answer is an `EditResponse` on purpose: the caller routes it through the
 * same `applyEditResponse` every timeline edit uses. Changing `fps` re-times
 * nothing — every time in the document is microseconds, so the rate is
 * presentation and export, never position.
 */
export function projectConfigure(config: ProjectConfig): Promise<EditResponse> {
  return invoke<EditResponse>("project_configure", { config });
}

export function projectGet(): Promise<Project | null> {
  return invoke<Project | null>("project_get");
}

export function projectValidate(): Promise<ValidationIssue[]> {
  return invoke<ValidationIssue[]>("project_validate");
}

export function projectPath(): Promise<string | null> {
  return invoke<string | null>("project_path");
}

/**
 * Probe a file and add it to the project's material pool.
 *
 * Importing the same path twice returns the existing material, and importing is
 * not undoable — a material with no segment referencing it is inert, and having
 * Ctrl+Z empty the media panel after a cut would be its own kind of bug.
 *
 * This mutates the document without returning it, so callers refresh the store
 * afterwards (`refreshDocument`); the pool is what segment insertion resolves
 * `material_id` against.
 */
export function projectImportMedia(path: string): Promise<ImportedMaterial> {
  return invoke<ImportedMaterial>("project_import_media", { path });
}
