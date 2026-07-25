import { AlertTriangleIcon, FileDownIcon, XIcon } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { HeaderBar } from "@/app/components/HeaderBar";
import { NewProjectDialog } from "@/app/components/NewProjectDialog";
import { PanelDivider } from "@/app/components/PanelDivider";
import { TooltipProvider } from "@/components/ui/tooltip";
import { type FileDropEvent, listenForFileDrop } from "@/lib/fileDrop";
import { clamp } from "@/lib/time";
import { ExportDialog } from "@/modules/export/components/ExportDialog";
import { ExportProgressDock } from "@/modules/export/components/ExportProgressDock";
import { Inspector } from "@/modules/inspector/components/Inspector";
import { MediaLibrary } from "@/modules/media/components/MediaLibrary";
import { useMediaStore } from "@/modules/media/store";
import { Preview } from "@/modules/preview/components/Preview";
import { projectGet, projectNew, projectPath } from "@/modules/project/lib/api";
import { describeError, useProjectStore } from "@/modules/project/store";
import { Timeline } from "@/modules/timeline/components/Timeline";
import { resolveTimelineDrop } from "@/modules/timeline/lib/dropTarget";
import { insertMaterial, STILL_DURATION } from "@/modules/timeline/lib/edits";

const LIBRARY_BOUNDS = [220, 560] as const;
const INSPECTOR_BOUNDS = [200, 520] as const;
const TIMELINE_BOUNDS = [160, 720] as const;

export function App() {
  const status = useProjectStore((s) => s.status);
  const error = useProjectStore((s) => s.error);
  const setError = useProjectStore((s) => s.setError);
  const setStatus = useProjectStore((s) => s.setStatus);
  const loadProject = useProjectStore((s) => s.loadProject);

  const [libraryWidth, setLibraryWidth] = useState(320);
  const [inspectorWidth, setInspectorWidth] = useState(272);
  const [timelineHeight, setTimelineHeight] = useState(300);
  const [newProjectOpen, setNewProjectOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [fileDragging, setFileDragging] = useState(false);

  // Adopt whatever Rust already has open; start something usable if it has
  // nothing, because an editor that opens onto an empty shell is hostile.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const existing = await projectGet();
        if (cancelled) return;
        if (existing) {
          loadProject(existing, await projectPath());
          return;
        }
        const created = await projectNew({
          name: "Untitled",
          width: 1080,
          height: 1920,
          fps: 30,
        });
        if (!cancelled) loadProject(created, null);
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
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return;
      const key = event.key.toLowerCase();
      if (key === "s") {
        // Let the header own saving; here we only stop the webview from
        // offering to save the page.
        event.preventDefault();
      }
      if (key === "e") {
        event.preventDefault();
        setExportOpen(true);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

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
        <HeaderBar
          onNewProject={() => setNewProjectOpen(true)}
          onExport={() => setExportOpen(true)}
        />

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
        {/* Outside the dialog on purpose: an export outlives the dialog that
            started it, and the editor stays usable while it runs. */}
        <ExportProgressDock />
      </div>
    </TooltipProvider>
  );
}
