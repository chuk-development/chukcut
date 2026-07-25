/**
 * Native file dialogs.
 *
 * The Rust side registers `tauri_plugin_dialog`, but its JS companion package
 * (`@tauri-apps/plugin-dialog`) is not in `package.json` and adding a dependency
 * is outside this work. The plugin's commands are reachable directly — that
 * package is a thin typed wrapper over exactly these invokes — so this file is
 * the wrapper instead.
 *
 * The capability file still has to grant `dialog:allow-open` / `dialog:allow-save`
 * before these succeed at runtime.
 */

import { invoke } from "@tauri-apps/api/core";

export interface DialogFilter {
  name: string;
  /** Extensions without the leading dot. */
  extensions: string[];
}

export interface OpenDialogOptions {
  title?: string;
  filters?: DialogFilter[];
  multiple?: boolean;
  directory?: boolean;
  defaultPath?: string;
}

export interface SaveDialogOptions {
  title?: string;
  filters?: DialogFilter[];
  defaultPath?: string;
}

export async function openFileDialog(options: OpenDialogOptions): Promise<string[]> {
  const result = await invoke<string | string[] | null>("plugin:dialog|open", {
    options: {
      title: options.title,
      filters: options.filters,
      multiple: options.multiple ?? false,
      directory: options.directory ?? false,
      defaultPath: options.defaultPath,
      recursive: false,
    },
  });
  if (result === null) return [];
  return Array.isArray(result) ? result : [result];
}

export async function saveFileDialog(options: SaveDialogOptions): Promise<string | null> {
  return invoke<string | null>("plugin:dialog|save", {
    options: {
      title: options.title,
      filters: options.filters,
      defaultPath: options.defaultPath,
    },
  });
}

export const VIDEO_FILTERS: DialogFilter[] = [
  {
    name: "Media",
    extensions: [
      "mp4",
      "mov",
      "mkv",
      "webm",
      "avi",
      "m4v",
      "mp3",
      "wav",
      "flac",
      "aac",
      "m4a",
      "png",
      "jpg",
      "jpeg",
      "webp",
      "gif",
    ],
  },
];

export const PROJECT_FILTERS: DialogFilter[] = [
  { name: "chukcut project", extensions: ["chukcut"] },
];
