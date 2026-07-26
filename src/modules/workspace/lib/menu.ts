/**
 * The webview's half of the native menu bar.
 *
 * The bar itself is built in Rust (`src-tauri/src/modules/workspace/menu.rs`),
 * because a menu drawn in HTML is not a menu — it does not live in the window
 * decoration, it does not follow the desktop's theme, and it cannot own a
 * keyboard accelerator. What Rust cannot do is know whether the document is
 * dirty or whether a clip is selected, so this file is the seam:
 *
 * - **Up:** the Zustand stores are the only authority on document state, and
 *   [`describeMenuState`] turns them into the handful of facts Rust needs. There is
 *   no second copy anywhere; the menu is pushed on every store change and Rust
 *   holds nothing but the enabled flags it derived.
 * - **Down:** a click arrives as the `menu://action` event carrying an item id,
 *   and [`runMenuAction`] does the same thing the existing button or shortcut
 *   would have done. Every branch calls code that already existed — the menu
 *   adds no behaviour of its own, which is what keeps it from drifting away
 *   from the rest of the app.
 *
 * Quitting is the exception and it is deliberate. `menu://close-requested` is
 * raised for both File → Quit *and* the window's close button, the window has
 * already been prevented from closing when it arrives, and it stays open until
 * [`answerCloseRequest`] says otherwise. That ordering is the whole reason
 * Cancel can genuinely abort: a close that has begun cannot be recalled.
 */

import { listen } from "@tauri-apps/api/event";

import { openFileDialog, VIDEO_FILTERS } from "@/lib/dialog";
import { useMediaStore } from "@/modules/media/store";
import { toggleFullscreen } from "@/modules/preview/lib/fullscreen";
import { describeError, useProjectStore } from "@/modules/project/store";
import type { Id, Micros, Project } from "@/modules/project/types";
import { projectDuration } from "@/modules/project/types";
import {
  copySegments,
  deleteSegments,
  duplicateSegments,
  pasteEntries,
  redo,
  segmentUnderPlayhead,
  splitAt,
  undo,
} from "@/modules/timeline/lib/edits";
import { allSelectableIds, liveSelection } from "@/modules/timeline/lib/selection";
import { soleSelection, useTimelineStore } from "@/modules/timeline/store";
import { workspaceCloseAnswer, workspaceMenuSync } from "@/modules/workspace/lib/api";
import { guardUnsaved, saveProject } from "@/modules/workspace/lib/lifecycle";
import type { MenuState } from "@/modules/workspace/types";

/** Rust raises this when an item it does not own itself is clicked. */
export const MENU_ACTION_EVENT = "menu://action";

/** Raised for both File → Quit and the window's close button. */
export const CLOSE_REQUESTED_EVENT = "menu://close-requested";

/** The item ids, spelled exactly as `menu.rs` spells them. */
export const MENU_IDS = {
  newProject: "file.new",
  openProject: "file.open",
  save: "file.save",
  saveAs: "file.save_as",
  importMedia: "file.import",
  export: "file.export",
  undo: "edit.undo",
  redo: "edit.redo",
  cut: "edit.cut",
  copy: "edit.copy",
  paste: "edit.paste",
  duplicate: "edit.duplicate",
  selectAll: "edit.select_all",
  delete: "edit.delete",
  split: "edit.split",
  zoomIn: "view.zoom_in",
  zoomOut: "view.zoom_out",
  zoomFit: "view.zoom_fit",
  fullscreen: "view.fullscreen",
  shortcuts: "help.shortcuts",
} as const;

/** The document half of the state, as the project store holds it. */
export interface DocumentFacts {
  project: Project | null;
  dirty: boolean;
  canUndo: boolean;
  canRedo: boolean;
}

/** The view half, as the timeline store holds it. */
export interface TimelineFacts {
  playhead: Micros;
  /** Every selected clip. Rust only needs to know whether there are any. */
  selection: readonly Id[];
  /** How many clips are on the clipboard. Nothing else about it crosses. */
  clipboard: number;
}

/**
 * Everything the menu needs, derived rather than tracked.
 *
 * Pure so the enable/disable behaviour can be tested without a window, a menu
 * or a running backend — which is the only way to test it at all, since the
 * bar itself is native and has no DOM.
 */
export function describeMenuState(document: DocumentFacts, timeline: TimelineFacts): MenuState {
  const { project } = document;
  if (!project) {
    // Nothing is open. Every fact about a document is false, rather than stale
    // from the one that was closed — except the clipboard, which is not a fact
    // about a document at all: it survives closing one, and Paste is exactly
    // what the user reaches for after opening the next.
    return {
      has_project: false,
      dirty: false,
      can_undo: false,
      can_redo: false,
      has_selection: false,
      can_split: false,
      can_fit: false,
      has_clipboard: timeline.clipboard > 0,
      has_clips: false,
    };
  }

  // A selection that survived the clip being deleted would light up a Delete
  // that then does nothing, so the ids have to still be in the document.
  const selected = timeline.selection.filter((id) => findsSegment(project, id));

  // The same question the razor asks, so Split is grey exactly when pressing C
  // would have printed "Nothing to split". With several clips selected there is
  // no preferred one, which is what the razor does too.
  const target = segmentUnderPlayhead(
    project,
    timeline.playhead,
    selected.length === 1 ? selected[0] : null,
  );

  return {
    has_project: true,
    dirty: document.dirty,
    can_undo: document.canUndo,
    can_redo: document.canRedo,
    has_selection: selected.length > 0,
    can_split: target !== null,
    // Fitting an empty timeline zooms to an arbitrary default, which is not
    // what "fit" means. Nothing on the timeline, nothing to fit.
    can_fit: projectDuration(project) > 0,
    has_clipboard: timeline.clipboard > 0,
    has_clips: project.tracks.some((track) => track.segments.length > 0),
  };
}

function findsSegment(project: Project, id: Id): boolean {
  return project.tracks.some((track) => track.segments.some((segment) => segment.id === id));
}

/** Read the stores and describe them. */
export function currentMenuState(): MenuState {
  const project = useProjectStore.getState();
  const timeline = useTimelineStore.getState();
  return describeMenuState(
    {
      project: project.project,
      dirty: project.dirty,
      canUndo: project.canUndo,
      canRedo: project.canRedo,
    },
    {
      playhead: timeline.playhead,
      selection: timeline.selection,
      clipboard: timeline.clipboard.length,
    },
  );
}

export function sameMenuState(a: MenuState, b: MenuState): boolean {
  return (
    a.has_project === b.has_project &&
    a.dirty === b.dirty &&
    a.can_undo === b.can_undo &&
    a.can_redo === b.can_redo &&
    a.has_selection === b.has_selection &&
    a.can_split === b.can_split &&
    a.can_fit === b.can_fit &&
    a.has_clipboard === b.has_clipboard &&
    a.has_clips === b.has_clips
  );
}

// ---------------------------------------------------------------------------
// Acting on a click
// ---------------------------------------------------------------------------

/**
 * The items whose action is a dialog. Everything else this file can do on its
 * own; a dialog is React state and belongs to whatever mounted it.
 */
export interface MenuHandlers {
  /** Guarded upstream, exactly as the header bar's own button is. */
  newProject: () => void;
  openProject: () => void;
  showExport: () => void;
  showShortcuts: () => void;
}

/**
 * The toolbar's own zoom step, so the menu and the buttons move the timeline by
 * the same amount.
 */
const ZOOM_STEP = 1.6;

/**
 * Do what the item says.
 *
 * Ids that are not listed here are handled in Rust — the about box, the log
 * directory, quitting — and never reach the webview.
 */
export function runMenuAction(id: string, handlers: MenuHandlers): void {
  switch (id) {
    case MENU_IDS.newProject:
      handlers.newProject();
      break;
    case MENU_IDS.openProject:
      handlers.openProject();
      break;
    case MENU_IDS.save:
      void saveProject();
      break;
    case MENU_IDS.saveAs:
      void saveProject({ promptForPath: true });
      break;
    case MENU_IDS.importMedia:
      void importMedia();
      break;
    case MENU_IDS.export:
      handlers.showExport();
      break;
    case MENU_IDS.undo:
      void undo();
      break;
    case MENU_IDS.redo:
      void redo();
      break;
    case MENU_IDS.cut:
      void cutSelection();
      break;
    case MENU_IDS.copy:
      copySelection();
      break;
    case MENU_IDS.paste:
      void pasteClipboard();
      break;
    case MENU_IDS.duplicate:
      void duplicateSelection();
      break;
    case MENU_IDS.selectAll:
      useTimelineStore.getState().selectMany(allSelectableIds(useProjectStore.getState().project));
      break;
    case MENU_IDS.delete:
      void deleteSelectedClips();
      break;
    case MENU_IDS.split:
      void splitUnderPlayhead();
      break;
    case MENU_IDS.zoomIn:
      useTimelineStore.getState().zoomBy(ZOOM_STEP);
      break;
    case MENU_IDS.zoomOut:
      useTimelineStore.getState().zoomBy(1 / ZOOM_STEP);
      break;
    case MENU_IDS.zoomFit:
      fitTimeline();
      break;
    case MENU_IDS.fullscreen:
      void toggleFullscreen();
      break;
    case MENU_IDS.shortcuts:
      handlers.showShortcuts();
      break;
    default:
      // An id Rust knows and this side does not. Silence would make the item
      // look broken to the user and fine to us.
      console.warn(`no handler for the menu item "${id}"`);
      break;
  }
}

/** The same import the media library's button runs. */
async function importMedia(): Promise<void> {
  try {
    const paths = await openFileDialog({
      title: "Import media",
      multiple: true,
      filters: VIDEO_FILTERS,
    });
    await useMediaStore.getState().importPaths(paths);
  } catch (error) {
    useProjectStore.getState().setError(describeError(error));
  }
}

/**
 * The selected clips that are still in the document.
 *
 * The menu is synced from the same facts, so this being empty here means the
 * document moved on between the bar being drawn and the item being clicked.
 */
function selectedClips(): { project: Project; ids: Id[] } | null {
  const project = useProjectStore.getState().project;
  if (!project) return null;
  const ids = liveSelection(project, useTimelineStore.getState().selection);
  return ids.length > 0 ? { project, ids } : null;
}

async function deleteSelectedClips(): Promise<void> {
  const found = selectedClips();
  if (!found) return;
  useTimelineStore.getState().select(null);
  await deleteSegments(found.project, found.ids);
}

/** Copying takes nothing from the document, so it is not an edit and not undoable. */
function copySelection(): void {
  const found = selectedClips();
  if (!found) return;
  useTimelineStore.getState().setClipboard(copySegments(found.project, found.ids));
}

async function cutSelection(): Promise<void> {
  const found = selectedClips();
  if (!found) return;
  useTimelineStore.getState().setClipboard(copySegments(found.project, found.ids));
  useTimelineStore.getState().select(null);
  await deleteSegments(found.project, found.ids);
}

async function pasteClipboard(): Promise<void> {
  const project = useProjectStore.getState().project;
  const { clipboard, playhead } = useTimelineStore.getState();
  if (!project || clipboard.length === 0) return;
  const pasted = await pasteEntries(project, clipboard, playhead);
  // What landed becomes the selection: a paste onto a lane that was off screen
  // is otherwise indistinguishable from nothing having happened.
  if (pasted.length > 0) useTimelineStore.getState().selectMany(pasted);
}

async function duplicateSelection(): Promise<void> {
  const found = selectedClips();
  if (!found) return;
  await duplicateSegments(found.project, found.ids);
}

async function splitUnderPlayhead(): Promise<void> {
  const project = useProjectStore.getState().project;
  if (!project) return;
  const { playhead, selection } = useTimelineStore.getState();
  const target = segmentUnderPlayhead(project, playhead, soleSelection(selection));
  // The item is grey when there is no target, so reaching this is a race
  // against the playhead rather than a user mistake, and a message would be
  // about something they had already stopped doing.
  if (!target) return;
  await splitAt(target.segmentId, playhead);
}

/**
 * Zoom so the whole timeline fits the viewport.
 *
 * The timeline owns this geometry and does the same thing for its toolbar
 * button, but the calculation needs the viewport's width and the menu handler
 * lives outside React. The lane viewport carries `data-slot="timeline-viewport"`
 * for exactly this kind of reach — 48px is the same right-hand breathing room
 * the toolbar's own fit leaves.
 */
export function fitTimeline(): void {
  const project = useProjectStore.getState().project;
  if (!project) return;
  const span = projectDuration(project);
  const viewport = document.querySelector<HTMLElement>('[data-slot="timeline-viewport"]');
  const width = viewport?.clientWidth ?? 0;
  if (span <= 0 || width <= 0) return;

  useTimelineStore.getState().setZoom((width - 48) / span);
  if (viewport) viewport.scrollLeft = 0;
}

// ---------------------------------------------------------------------------
// Quitting
// ---------------------------------------------------------------------------

/**
 * Answer the close request Rust is holding the window open for.
 *
 * Returns whether the app is going. The guard is `guardUnsaved`, the same one
 * New Project and Open Project run, so all three ask the same question and
 * honour the same three answers — including "Save", which only lets the close
 * through if the write actually landed.
 */
export async function answerCloseRequest(): Promise<boolean> {
  const proceed = await guardUnsaved("closing chukcut");
  try {
    await workspaceCloseAnswer(proceed);
  } catch (error) {
    // Rust is holding the window open waiting for this. Failing silently would
    // leave a window that cannot be shut, so it is worth saying.
    useProjectStore.getState().setError(describeError(error));
  }
  return proceed;
}

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

/**
 * Keep the bar in step with the stores and route its clicks. Returns the
 * teardown.
 *
 * Pushed rather than polled, and deduplicated: an edit replaces the whole
 * document and a pointer move over the lanes writes the playhead many times a
 * second, so without the comparison this would be an IPC call per frame.
 */
export function installNativeMenu(handlers: MenuHandlers): () => void {
  let last: MenuState | null = null;

  const push = () => {
    const next = currentMenuState();
    if (last && sameMenuState(last, next)) return;
    last = next;
    // A menu that fails to update is a cosmetic problem; an error banner about
    // it is not. The next change tries again.
    void workspaceMenuSync(next).catch(() => {});
  };

  push();
  const unsubscribes = [useProjectStore.subscribe(push), useTimelineStore.subscribe(push)];

  let cancelled = false;
  const listeners: Array<() => void> = [];
  // The subscription can resolve after the effect that started it was torn
  // down, which leaves a listener nothing will ever remove.
  const keep = (off: () => void) => {
    if (cancelled) off();
    else listeners.push(off);
  };
  // No event plumbing means no native menu, which the app has to survive: every
  // item in the bar has a button or a keyboard shortcut behind it as well.
  const ignore = () => {};

  listen<{ id: string }>(MENU_ACTION_EVENT, (event) => runMenuAction(event.payload.id, handlers))
    .then(keep)
    .catch(ignore);
  listen(CLOSE_REQUESTED_EVENT, () => void answerCloseRequest())
    .then(keep)
    .catch(ignore);

  return () => {
    cancelled = true;
    for (const off of unsubscribes) off();
    for (const off of listeners) off();
  };
}
