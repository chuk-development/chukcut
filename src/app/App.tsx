import { AlertTriangleIcon, FileDownIcon, LifeBuoyIcon, Loader2Icon, XIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { HeaderBar } from "@/app/components/HeaderBar";
import { NewProjectDialog } from "@/app/components/NewProjectDialog";
import { PanelDivider } from "@/app/components/PanelDivider";
import { ResizeEdges } from "@/app/components/ResizeEdges";
import { TitleBar } from "@/app/components/TitleBar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { type FileDropEvent, listenForFileDrop } from "@/lib/fileDrop";
import { installTransportKeys } from "@/lib/shortcuts";
import { clamp } from "@/lib/time";
import { ExportDialog } from "@/modules/export/components/ExportDialog";
import { ExportProgressDock } from "@/modules/export/components/ExportProgressDock";
import { Inspector } from "@/modules/inspector/components/Inspector";
import { MediaLibrary } from "@/modules/media/components/MediaLibrary";
import { useMediaStore } from "@/modules/media/store";
import { Preview } from "@/modules/preview/components/Preview";
import { preview } from "@/modules/preview/lib/session";
import { projectGet, projectPath } from "@/modules/project/lib/api";
import { describeError, useProjectStore } from "@/modules/project/store";
import { Timeline } from "@/modules/timeline/components/Timeline";
import { resolveTimelineDrop } from "@/modules/timeline/lib/dropTarget";
import { insertMaterial, STILL_DURATION } from "@/modules/timeline/lib/edits";
import { useTimelineStore } from "@/modules/timeline/store";
import { ProjectSettingsDialog } from "@/modules/workspace/components/ProjectSettingsDialog";
import { SettingsDialog } from "@/modules/workspace/components/SettingsDialog";
import { ShortcutsDialog } from "@/modules/workspace/components/ShortcutsDialog";
import { StartScreen } from "@/modules/workspace/components/StartScreen";
import { UnsavedChangesDialog } from "@/modules/workspace/components/UnsavedChangesDialog";
import { chooseAndOpenProject, guardUnsaved, saveProject } from "@/modules/workspace/lib/lifecycle";
import {
  acceleratorAction,
  installMenu,
  type MenuHandlers,
  runMenuAction,
} from "@/modules/workspace/lib/menu";
import { type RecoveryNotice, restoredFromWorkingCopy } from "@/modules/workspace/lib/recovery";
import { useWorkspaceStore } from "@/modules/workspace/store";
import type { MenuSectionView } from "@/modules/workspace/types";

const LIBRARY_BOUNDS = [220, 560] as const;
const INSPECTOR_BOUNDS = [200, 520] as const;
const TIMELINE_BOUNDS = [160, 720] as const;

export function App() {
  const project = useProjectStore((s) => s.project);
  const status = useProjectStore((s) => s.status);
  const error = useProjectStore((s) => s.error);
  const setError = useProjectStore((s) => s.setError);
  const setStatus = useProjectStore((s) => s.setStatus);
  const loadProject = useProjectStore((s) => s.loadProject);

  const settings = useWorkspaceStore((s) => s.settings);
  const loadSettings = useWorkspaceStore((s) => s.loadSettings);

  const [libraryWidth, setLibraryWidth] = useState(320);
  const [inspectorWidth, setInspectorWidth] = useState(272);
  const [timelineHeight, setTimelineHeight] = useState(300);
  const [newProjectOpen, setNewProjectOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [projectSettingsOpen, setProjectSettingsOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [fileDragging, setFileDragging] = useState(false);
  const [recovered, setRecovered] = useState<RecoveryNotice | null>(null);
  /**
   * The menu bar as Rust last resolved it.
   *
   * Empty until the first answer lands, which is a frame or two: the bar's
   * contents are not a constant on this side, on purpose. `menu.rs` holds the
   * one table of items and gates, and a copy here to render "instantly" would be
   * a second one, free to drift.
   */
  const [menuSections, setMenuSections] = useState<MenuSectionView[]>([]);

  /**
   * Adopt whatever Rust already has open.
   *
   * If it has nothing, the start screen appears. An earlier version created an
   * "Untitled" project here instead, which meant the user never chose a canvas
   * and never found their previous work — the two things a launch screen exists
   * for.
   */
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const existing = await projectGet();
        if (cancelled) return;
        if (existing) {
          loadProject(existing, await projectPath());
          // The crash-recovery seam. Rust restores the working copy inside
          // `project_get`, so by the time we are here it has already happened
          // and the only useful question left is whether the user knows. Inert
          // until `workspace_recovery_status` exists; see `lib/recovery.ts`.
          const notice = await restoredFromWorkingCopy();
          if (!cancelled && notice) {
            setRecovered(notice);
            // It came out of a working copy, not out of their file. Saying it
            // is clean would be the whole trap.
            useProjectStore.setState({ dirty: true });
          }
          return;
        }
        setStatus("ready");
      } catch (caught) {
        if (cancelled) return;
        setStatus("error");
        setError(describeError(caught));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [loadProject, setError, setStatus]);

  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);

  // Snapping is also toggled by the S key for the session, so the timeline
  // store is the authority while the app runs and this only pushes the
  // persisted value into it.
  useEffect(() => {
    useTimelineStore.setState({ snapping: settings.snapping });
  }, [settings.snapping]);

  /**
   * Preview resolution and quality are read when a session opens, so a change
   * needs a new session to take effect. Skipped on the first run: settings
   * arriving from Rust is not the user changing anything, and restarting there
   * would throw away the session the player has just opened.
   */
  const previewKey = `${settings.preview_max_edge}/${settings.preview_full_quality}/${settings.preview_quality}`;
  const lastPreviewKey = useRef<string | null>(null);
  useEffect(() => {
    if (lastPreviewKey.current !== null && lastPreviewKey.current !== previewKey) {
      preview.restart();
    }
    lastPreviewKey.current = previewKey;
  }, [previewKey]);

  const startNewProject = useCallback(async () => {
    if (await guardUnsaved("starting a new project")) setNewProjectOpen(true);
  }, []);

  const openProject = useCallback(async () => {
    if (await guardUnsaved("opening another project")) await chooseAndOpenProject();
  }, []);

  /**
   * The four items whose action is a dialog. Everything else the menu does —
   * saving, undo, zoom, fullscreen — it drives through the same functions the
   * buttons and the shortcuts use, so the bar adds no behaviour of its own.
   */
  const menuHandlers: MenuHandlers = useMemo(
    () => ({
      newProject: () => void startNewProject(),
      openProject: () => void openProject(),
      showExport: () => setExportOpen(true),
      showShortcuts: () => setShortcutsOpen(true),
      showProjectSettings: () => setProjectSettingsOpen(true),
    }),
    [openProject, startNewProject],
  );

  /**
   * The menu bar.
   *
   * Mounted here because this is where the dialogs it opens live. What the bar
   * offers and which of it is clickable is still decided in Rust; this keeps
   * that answer in step with the stores and hands each redraw to `TitleBar`. See
   * `workspace/lib/menu.ts`.
   */
  useEffect(() => installMenu(setMenuSections), []);

  /**
   * J/K/L. Bound at the app level rather than in the preview panel because
   * they act on the playback session, which outlives any panel; the guard on
   * an open project is inside the handler. See `src/lib/shortcuts.ts` for why
   * J steps back a frame instead of reverse-playing.
   */
  useEffect(() => installTransportKeys(), []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const typing = Boolean(
        target && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)),
      );

      if (!(event.ctrlKey || event.metaKey)) {
        // The reference is reachable without a modifier, since anyone who needs
        // it by definition does not know the modifier ones either.
        if (!typing && (event.key === "?" || event.key === "F1")) {
          event.preventDefault();
          setShortcutsOpen(true);
        }
        return;
      }

      // The keys the bar advertises that nothing else in the app binds — import,
      // quit and the three zooms. They were the native menu's accelerators until
      // the native menu went; `acceleratorAction` says which ones and why.
      //
      // Not gated on `typing`, like the four below it and like the accelerators
      // these replace. None of them is a text-editing key, and a Ctrl+Q that
      // does nothing because the caret happens to be in the project-name box is
      // a worse surprise than one that quits.
      const accelerated = acceleratorAction(event);
      if (accelerated) {
        event.preventDefault();
        runMenuAction(accelerated, menuHandlers);
        return;
      }

      switch (event.key.toLowerCase()) {
        case "n":
          event.preventDefault();
          void startNewProject();
          break;
        case "o":
          event.preventDefault();
          void openProject();
          break;
        case "s":
          // Also stops the webview offering to save the page.
          event.preventDefault();
          void saveProject({ promptForPath: event.shiftKey });
          break;
        case "e":
          event.preventDefault();
          setExportOpen(true);
          break;
        case ",":
          event.preventDefault();
          setSettingsOpen(true);
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [menuHandlers, openProject, startNewProject]);

  /**
   * Files dragged in from the desktop.
   *
   * Import first, always — a segment cannot reference a material that is not in
   * the pool. Where the pointer was decides what happens next: over the
   * timeline the files also land on a lane, end to end from the drop point;
   * anywhere else they just join the library.
   */
  const handleFileDrop = useCallback(async ({ paths, clientX, clientY }: FileDropEvent) => {
    const target = resolveTimelineDrop(clientX, clientY);
    const materials = await useMediaStore.getState().importPaths(paths);
    if (!target || materials.length === 0) return;

    let at = target.at;
    for (const material of materials) {
      // Re-read every iteration: each insert replaces the document wholesale.
      const project = useProjectStore.getState().project;
      if (!project) break;
      const placed = await insertMaterial(project, material, at, target.trackId);
      if (!placed) break;
      at = placed.start + (material.duration > 0 ? material.duration : STILL_DURATION);
    }
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    listenForFileDrop({
      onEnter: () => setFileDragging(true),
      onLeave: () => setFileDragging(false),
      onDrop: (event) => void handleFileDrop(event),
    })
      .then((off) => {
        // The subscription can resolve after the effect was torn down.
        if (cancelled) off();
        else unlisten = off;
      })
      // Dropping files in from the desktop is a convenience on top of the
      // Import button. Losing it must not cost the user the editor.
      .catch(() => setFileDragging(false));

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [handleFileDrop]);

  const resizeLibrary = useCallback(
    (delta: number) =>
      setLibraryWidth((width) => clamp(width + delta, LIBRARY_BOUNDS[0], LIBRARY_BOUNDS[1])),
    [],
  );
  const resizeInspector = useCallback(
    (delta: number) =>
      setInspectorWidth((width) => clamp(width - delta, INSPECTOR_BOUNDS[0], INSPECTOR_BOUNDS[1])),
    [],
  );
  const resizeTimeline = useCallback(
    (delta: number) =>
      setTimelineHeight((height) => clamp(height - delta, TIMELINE_BOUNDS[0], TIMELINE_BOUNDS[1])),
    [],
  );

  return (
    <TooltipProvider>
      <div className="flex h-full flex-col overflow-hidden bg-background text-foreground">
        {/* Unconditional, unlike the header bar below it: the window has no
            decorations, so this strip is the only way to move, resize or close
            it. A start screen or a backend that never answered must not be a
            window the user cannot shut. */}
        <TitleBar
          sections={menuSections}
          onSelectMenuItem={(id) => runMenuAction(id, menuHandlers)}
        />

        {project ? (
          <HeaderBar
            onNewProject={() => void startNewProject()}
            onOpenProject={() => void openProject()}
            onExport={() => setExportOpen(true)}
            onOpenSettings={() => setSettingsOpen(true)}
            onShowShortcuts={() => setShortcutsOpen(true)}
          />
        ) : null}

        {recovered ? (
          <div className="flex shrink-0 items-center gap-2 border-b border-border bg-primary/15 px-3 py-1.5 text-[12px]">
            <LifeBuoyIcon className="size-3.5 shrink-0 text-primary" />
            <span className="min-w-0 flex-1 truncate">
              Restored from the last session
              {recovered.path ? ` — ${recovered.path}` : " — never saved to a file"}. It is not on
              disk yet.
            </span>
            <button
              type="button"
              onClick={() => void saveProject().then((saved) => saved && setRecovered(null))}
              className="shrink-0 rounded-sm px-1.5 py-0.5 font-medium hover:bg-foreground/10"
            >
              Save now
            </button>
            <button
              type="button"
              aria-label="Dismiss"
              onClick={() => setRecovered(null)}
              className="grid size-[18px] shrink-0 place-items-center rounded-sm hover:bg-foreground/10"
            >
              <XIcon className="size-3" />
            </button>
          </div>
        ) : null}

        {error ? (
          <div className="flex shrink-0 items-center gap-2 border-b border-border bg-destructive/20 px-3 py-1.5 text-[12px] text-destructive-foreground">
            <AlertTriangleIcon className="size-3.5 shrink-0" />
            <span className="min-w-0 flex-1 truncate">{error}</span>
            <button
              type="button"
              aria-label="Dismiss"
              onClick={() => setError(null)}
              className="grid size-[18px] shrink-0 place-items-center rounded-sm hover:bg-foreground/10"
            >
              <XIcon className="size-3" />
            </button>
          </div>
        ) : null}

        {status === "error" ? (
          <div className="grid flex-1 place-items-center px-8 text-center">
            <div className="max-w-md">
              <h1 className="text-[15px] font-semibold">The Rust core did not answer</h1>
              <p className="mt-1.5 text-[12px] leading-relaxed text-muted-foreground">
                Every capability crosses the boundary as a registered command. Nothing can be edited
                until the backend is running — start the app with{" "}
                <code className="font-mono">pnpm tauri dev</code>.
              </p>
            </div>
          </div>
        ) : status === "loading" ? (
          <div className="grid flex-1 place-items-center px-8 text-center">
            <p className="flex items-center gap-2 text-[12px] text-muted-foreground">
              <Loader2Icon className="size-3.5 animate-spin" />
              Asking the engine what is open
            </p>
          </div>
        ) : !project ? (
          <StartScreen
            onMoreOptions={() => setNewProjectOpen(true)}
            onOpenSettings={() => setSettingsOpen(true)}
          />
        ) : (
          <main className="flex min-h-0 flex-1 flex-col">
            <div className="flex min-h-0 flex-1">
              <div className="min-w-0 shrink-0" style={{ width: libraryWidth }}>
                <MediaLibrary />
              </div>
              <PanelDivider orientation="vertical" onResize={resizeLibrary} />

              <div className="min-w-0 flex-1">
                <Preview />
              </div>

              <PanelDivider orientation="vertical" onResize={resizeInspector} />
              <div className="min-w-0 shrink-0" style={{ width: inspectorWidth }}>
                <Inspector />
              </div>
            </div>

            <PanelDivider orientation="horizontal" onResize={resizeTimeline} />
            <div className="min-h-0 shrink-0" style={{ height: timelineHeight }}>
              <Timeline />
            </div>
          </main>
        )}

        {fileDragging ? (
          <div className="pointer-events-none fixed inset-x-0 bottom-6 z-50 flex justify-center">
            <p className="flex items-center gap-2 rounded-full border border-primary/60 bg-popover px-3.5 py-1.5 text-[12px] shadow-xl">
              <FileDownIcon className="size-3.5 text-primary" />
              Drop over the timeline to place, anywhere else to import
            </p>
          </div>
        ) : null}

        <NewProjectDialog open={newProjectOpen} onOpenChange={setNewProjectOpen} />
        <ExportDialog open={exportOpen} onOpenChange={setExportOpen} />
        <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
        <ProjectSettingsDialog open={projectSettingsOpen} onOpenChange={setProjectSettingsOpen} />
        {/* The same sections the title bar draws: the reference lists every
            key the menu advertises by reading the bar itself. */}
        <ShortcutsDialog
          open={shortcutsOpen}
          onOpenChange={setShortcutsOpen}
          sections={menuSections}
        />
        {/* One prompt for the whole app: the question outlives the menu item
            that raised it, and a dialog owned by a dropdown dies with it. */}
        <UnsavedChangesDialog />
        {/* Outside the dialog on purpose: an export outlives the dialog that
            started it, and the editor stays usable while it runs. */}
        <ExportProgressDock />

        {/* Last, and above everything: an undecorated window has no frame to
            grab, so these are the only way to resize it with a pointer. */}
        <ResizeEdges />
      </div>
    </TooltipProvider>
  );
}
