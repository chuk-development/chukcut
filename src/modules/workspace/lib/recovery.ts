/**
 * Crash recovery — the seam, not the mechanism.
 *
 * ============================ READ THIS FIRST ============================
 * Autosave is being built concurrently in Rust and, as of this writing, none of
 * the commands below is registered. Every call here degrades to "there is
 * nothing to say", so this file is inert rather than broken until they exist.
 * The frontend work is knowing *what to ask*, and that is what this records.
 *
 * ## What the Rust side already decided
 *
 * `project/autosave.rs` and `project/commands.rs` (both uncommitted while this
 * was written) chose to **apply** the working copy rather than offer it:
 * `project_get` restores `<config>/autosave.chukcut` into `AppState` on the
 * first call after launch, exactly once, and answers with it. The path the user
 * last saved to is remembered in a sibling `.path` file and becomes the open
 * path.
 *
 * That is a defensible choice and it is not the one this file was first written
 * against, so it changes what the frontend should ask for. If the document is
 * restored before anyone is asked, then "restore the recovered project?" is the
 * wrong question — the right one is **"do you know that what you are looking at
 * is not in your file yet?"**, because the restore is otherwise completely
 * silent: the header says the project is clean, the user closes the window, and
 * only the working copy ever held their afternoon.
 *
 * So there are two seams here, and which one lives depends on a decision Rust
 * has already half-made:
 *
 * 1. `restoredFromWorkingCopy()` — **the one that matches the code as it is.**
 *    Answers whether the document the app is showing came out of the working
 *    copy. The shell announces it and marks the document unsaved, so the next
 *    Ctrl+S puts it somewhere real.
 * 2. `pendingRecovery()` / `restoreRecovery()` / `discardRecovery()` — the
 *    offer-first design, where `project_get` answers `null` and the start
 *    screen asks. Kept because it is strictly better for the case the working
 *    copy is *corrupt or enormous or from a project the user deliberately
 *    abandoned*, and because it is the one behaviour a user can refuse.
 *
 * Implementing (1) is one command over state Rust already has. Implementing (2)
 * means `project_get` stops restoring. They are mutually exclusive; whichever
 * lands, delete the other half of this file.
 *
 * ## What is assumed either way
 *
 * - A stale working copy is Rust's problem: it is deleted when the app exits
 *   cleanly, so anything reported here is by definition a session that did not.
 * - Nothing here compares timestamps or decides what is newer. The frontend
 *   asks and renders; the engine owns the file.
 * =========================================================================
 */

import { invoke } from "@tauri-apps/api/core";

import { useProjectStore } from "@/modules/project/store";
import type { Project } from "@/modules/project/types";

/** What `workspace_recovery_status` should answer with, for seam (1). */
export interface RecoveryNotice {
  /** The document now open came from the working copy rather than from a file. */
  restored: boolean;
  /** The file it belonged to before the crash, if it had ever been saved. */
  path: string | null;
  /** Unix millis of the last working-copy write. */
  saved_at: number;
  /** Anything `migrate::load` had to repair on the way in. Worth showing. */
  warnings: string[];
}

/**
 * Did the document we are showing come back from a crash?
 *
 * Answers `null` for "no" and for "the engine cannot be asked yet", which are
 * the same thing to the shell. Never throws: an unavailable recovery status
 * must not stop the app from starting.
 */
export async function restoredFromWorkingCopy(): Promise<RecoveryNotice | null> {
  try {
    const notice = await invoke<RecoveryNotice | null>("workspace_recovery_status");
    return notice?.restored ? notice : null;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Seam (2): offer, do not apply
// ---------------------------------------------------------------------------

/** What `workspace_recovery_peek` should answer with. */
export interface RecoveredProject {
  /** The project's own name, for "Restore <name>?". */
  name: string;
  path: string | null;
  saved_at: number;
}

/** Is there work from a session that did not end cleanly, waiting to be claimed? */
export async function pendingRecovery(): Promise<RecoveredProject | null> {
  try {
    return await invoke<RecoveredProject | null>("workspace_recovery_peek");
  } catch {
    return null;
  }
}

/** What `workspace_recovery_restore` should answer with. Mirrors `autosave::Restored`. */
export interface RestoredProject {
  project: Project;
  /** The file it belonged to, so the next save goes back there. */
  path: string | null;
  warnings: string[];
}

/** Adopt the recovered document. Returns whether it worked. */
export async function restoreRecovery(): Promise<boolean> {
  try {
    const restored = await invoke<RestoredProject>("workspace_recovery_restore");
    useProjectStore.getState().loadProject(restored.project, restored.path);
    // Recovered work is by definition not in the user's own file yet, even when
    // we know which file that was — so it is unsaved, and the header says so
    // until they do something about it.
    useProjectStore.setState({ dirty: true });
    return true;
  } catch {
    return false;
  }
}

/** Throw the recovered work away. Not undoable. */
export async function discardRecovery(): Promise<void> {
  try {
    await invoke<void>("workspace_recovery_discard");
  } catch {
    // Nothing to discard, or no such command. Either way the banner goes.
  }
}
