import {
  ChevronDownIcon,
  FilePlus2Icon,
  FolderOpenIcon,
  Loader2Icon,
  Redo2Icon,
  SaveIcon,
  ScissorsLineDashedIcon,
  Undo2Icon,
  UploadIcon,
} from "lucide-react";
import { useCallback, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Separator } from "@/components/ui/separator";
import { IconTooltip } from "@/components/ui/tooltip";
import { openFileDialog, PROJECT_FILTERS, saveFileDialog } from "@/lib/dialog";
import { runningJobs, useExportStore } from "@/modules/export/store";
import { projectOpen, projectSave } from "@/modules/project/lib/api";
import { describeError, useProjectStore } from "@/modules/project/store";
import { redo, undo } from "@/modules/timeline/lib/edits";

interface HeaderBarProps {
  onNewProject: () => void;
  onExport: () => void;
}

export function HeaderBar({ onNewProject, onExport }: HeaderBarProps) {
  const project = useProjectStore((s) => s.project);
  const path = useProjectStore((s) => s.path);
  const dirty = useProjectStore((s) => s.dirty);
  const canUndo = useProjectStore((s) => s.canUndo);
  const canRedo = useProjectStore((s) => s.canRedo);
  const undoLabel = useProjectStore((s) => s.undoLabel);
  const redoLabel = useProjectStore((s) => s.redoLabel);
  const loadProject = useProjectStore((s) => s.loadProject);
  const markSaved = useProjectStore((s) => s.markSaved);
  const setError = useProjectStore((s) => s.setError);
  const exportJobs = useExportStore((s) => s.jobs);
  const [busy, setBusy] = useState(false);

  const exporting = runningJobs(exportJobs).length;

  const open = useCallback(async () => {
    try {
      const [chosen] = await openFileDialog({ title: "Open project", filters: PROJECT_FILTERS });
      if (!chosen) return;
      loadProject(await projectOpen(chosen), chosen);
    } catch (error) {
      setError(describeError(error));
    }
  }, [loadProject, setError]);

  const save = useCallback(
    async (forcePrompt: boolean) => {
      setBusy(true);
      try {
        let target = path ?? undefined;
        if (forcePrompt || !target) {
          const chosen = await saveFileDialog({
            title: "Save project",
            filters: PROJECT_FILTERS,
            defaultPath: `${project?.name ?? "Untitled"}.chukcut`,
          });
          if (!chosen) return;
          target = chosen;
        }
        markSaved(await projectSave(target));
      } catch (error) {
        setError(describeError(error));
      } finally {
        setBusy(false);
      }
    },
    [markSaved, path, project?.name, setError],
  );

  return (
    <header
      data-slot="header-bar"
      className="flex h-9 shrink-0 items-center gap-1 border-b border-border bg-panel px-2"
    >
      <div className="flex items-center gap-1.5 pr-1">
        <ScissorsLineDashedIcon className="size-4 text-primary" />
        <span className="text-[12px] font-semibold tracking-tight">chukcut</span>
      </div>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button size="sm">
            Project
            <ChevronDownIcon className="size-3 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuItem onSelect={onNewProject}>
            <FilePlus2Icon />
            New project…
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void open()}>
            <FolderOpenIcon />
            Open…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => void save(false)}>
            <SaveIcon />
            Save
            <DropdownMenuShortcut>Ctrl+S</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void save(true)}>
            <SaveIcon />
            Save as…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={onExport}>
            <UploadIcon />
            Export…
            <DropdownMenuShortcut>Ctrl+E</DropdownMenuShortcut>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <Separator orientation="vertical" className="mx-1 h-4" />

      <IconTooltip label={undoLabel ? `Undo ${undoLabel.toLowerCase()}` : "Undo"} hint="Ctrl+Z">
        <Button size="icon" onClick={() => void undo()} disabled={!canUndo}>
          <Undo2Icon />
        </Button>
      </IconTooltip>
      <IconTooltip
        label={redoLabel ? `Redo ${redoLabel.toLowerCase()}` : "Redo"}
        hint="Ctrl+Shift+Z"
      >
        <Button size="icon" onClick={() => void redo()} disabled={!canRedo}>
          <Redo2Icon />
        </Button>
      </IconTooltip>

      <div className="mx-auto flex min-w-0 items-center gap-1.5 px-4">
        <span className="truncate text-[12px] font-medium text-panel-foreground">
          {project?.name ?? "No project"}
        </span>
        {dirty ? (
          <span className="size-1.5 shrink-0 rounded-full bg-primary" title="Unsaved changes" />
        ) : null}
        {path ? (
          <span className="hidden truncate text-[11px] text-muted-foreground xl:inline">
            {path}
          </span>
        ) : null}
      </div>

      <IconTooltip label={dirty ? "Save changes" : "Saved"} hint="Ctrl+S">
        <Button size="sm" onClick={() => void save(false)} disabled={busy || !project}>
          {busy ? <Loader2Icon className="animate-spin" /> : <SaveIcon />}
          {busy ? "Saving…" : "Save"}
        </Button>
      </IconTooltip>
      <IconTooltip
        label={exporting > 0 ? `Export — ${exporting} running` : "Export the timeline to a file"}
        hint="Ctrl+E"
      >
        <Button variant="default" size="sm" onClick={onExport} disabled={!project}>
          {exporting > 0 ? <Loader2Icon className="animate-spin" /> : <UploadIcon />}
          Export
        </Button>
      </IconTooltip>
    </header>
  );
}
