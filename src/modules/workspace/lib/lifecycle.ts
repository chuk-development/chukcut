/**
 * New, open, save, save-as — and the prompt that stands in front of the ones
 * that would throw work away.
 *
 * This lives in `workspace` rather than in `project` because it is about the
 * *session*, not the document: which file is open, whether it has been written
 * since the last edit, what the recent list should say afterwards. The project
 * module owns the document itself and knows nothing about any of that.
 *
 * Every function here answers `true` when the thing happened and `false` when
 * it did not — including when the user cancelled, which is not an error and
 * must not put a message on screen. Real failures are pushed into the project
 * store's error slot, which the header already renders.
 */

import { openFileDialog, PROJECT_FILTERS, saveFileDialog } from "@/lib/dialog";
import {
  type NewProjectOptions,
  projectNew,
  projectOpen,
  projectSave,
} from "@/modules/project/lib/api";
import { describeError, useProjectStore } from "@/modules/project/store";
import { basename } from "@/modules/project/types";
import { useWorkspaceStore } from "@/modules/workspace/store";

/**
 * Stand in front of anything that would discard unsaved changes.
 *
 * Returns whether to go ahead. A clean document goes ahead silently — a
 * confirmation nobody needed is how people learn to dismiss confirmations
 * without reading them.
 *
 * `intent` completes the sentence "Save your changes before you …", so it reads
 * as "opening another project", not "Open".
 */
export async function guardUnsaved(intent: string): Promise<boolean> {
  if (!useProjectStore.getState().dirty) return true;

  const choice = await useWorkspaceStore.getState().askBeforeDiscarding(intent);
  if (choice === "cancel") return false;
  if (choice === "discard") return true;
  // "Save" only clears the way if the save actually landed: a cancelled file
  // dialog or a failed write must not then throw the work away.
  return saveProject();
}

/**
 * Write the project out.
 *
 * With no path — a project that has never been saved — this prompts for one,
 * which makes Save and Save-as the same function with one flag between them.
 */
export async function saveProject(options: { promptForPath?: boolean } = {}): Promise<boolean> {
  const { project, path } = useProjectStore.getState();
  if (!project) return false;

  try {
    let target = path ?? null;
    if (options.promptForPath || !target) {
      target = await saveFileDialog({
        title: options.promptForPath ? "Save project as" : "Save project",
        filters: PROJECT_FILTERS,
        defaultPath: `${project.name || "Untitled"}.chukcut`,
      });
      if (!target) return false;
    }

    const written = await projectSave(target);
    useProjectStore.getState().markSaved(written);
    await useWorkspaceStore.getState().recordRecent(written, project.name);
    return true;
  } catch (error) {
    useProjectStore.getState().setError(describeError(error));
    return false;
  }
}

/** Create a project and make it the open one. Does not guard; the caller does. */
export async function createProject(options: NewProjectOptions): Promise<boolean> {
  try {
    useProjectStore.getState().loadProject(await projectNew(options), null);
    return true;
  } catch (error) {
    useProjectStore.getState().setError(describeError(error));
    return false;
  }
}

/**
 * Open a specific file. Does not guard; the caller does.
 *
 * A failure here is usually the file having moved since the recent list was
 * built, so the entry is dropped rather than left to fail again on the next
 * click.
 */
export async function openProjectAt(path: string): Promise<boolean> {
  const workspace = useWorkspaceStore.getState();
  try {
    const project = await projectOpen(path);
    useProjectStore.getState().loadProject(project, path);
    await workspace.recordRecent(path, project.name || basename(path));
    return true;
  } catch (error) {
    workspace.dropRecent(path);
    useProjectStore.getState().setError(describeError(error));
    return false;
  }
}

/** The Open… item: a file dialog, then whatever it chose. Does not guard. */
export async function chooseAndOpenProject(): Promise<boolean> {
  let chosen: string | undefined;
  try {
    [chosen] = await openFileDialog({ title: "Open project", filters: PROJECT_FILTERS });
  } catch (error) {
    useProjectStore.getState().setError(describeError(error));
    return false;
  }
  if (!chosen) return false;
  return openProjectAt(chosen);
}

/** Close the document and return to the start screen. Does not guard. */
export function closeProject(): void {
  useProjectStore.setState({
    project: null,
    path: null,
    status: "ready",
    dirty: false,
    canUndo: false,
    canRedo: false,
    undoLabel: null,
    redoLabel: null,
  });
}
