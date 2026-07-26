import {
  ChevronDownIcon,
  FilePlus2Icon,
  FolderOpenIcon,
  KeyboardIcon,
  Loader2Icon,
  Redo2Icon,
  SaveIcon,
  ScissorsLineDashedIcon,
  SettingsIcon,
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
import { runningJobs, useExportStore } from "@/modules/export/store";
import { useProjectStore } from "@/modules/project/store";
import { redo, undo } from "@/modules/timeline/lib/edits";
import { saveProject } from "@/modules/workspace/lib/lifecycle";
import { MODIFIER } from "@/modules/workspace/lib/shortcuts";

interface HeaderBarProps {
  /** Guarded upstream: both of these can discard unsaved work. */
  onNewProject: () => void;
  onOpenProject: () => void;
  onExport: () => void;
  onOpenSettings: () => void;
  onShowShortcuts: () => void;
}

export function HeaderBar({
  onNewProject,
  onOpenProject,
  onExport,
  onOpenSettings,
  onShowShortcuts,
}: HeaderBarProps) {
  const project = useProjectStore((s) => s.project);
  const path = useProjectStore((s) => s.path);
  const dirty = useProjectStore((s) => s.dirty);
  const canUndo = useProjectStore((s) => s.canUndo);
  const canRedo = useProjectStore((s) => s.canRedo);
  const undoLabel = useProjectStore((s) => s.undoLabel);
  const redoLabel = useProjectStore((s) => s.redoLabel);
  const exportJobs = useExportStore((s) => s.jobs);
  const [busy, setBusy] = useState(false);

  const exporting = runningJobs(exportJobs).length;

  // Saving itself — the file dialog, the recent list, the error — belongs to
  // `workspace/lib/lifecycle`, because Ctrl+S and the start screen need exactly
  // the same behaviour and neither of them is this component.
  const save = useCallback(async (promptForPath: boolean) => {
    setBusy(true);
    try {
      await saveProject({ promptForPath });
    } finally {
      setBusy(false);
    }
  }, []);

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
            <DropdownMenuShortcut>{MODIFIER}+N</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onOpenProject}>
            <FolderOpenIcon />
            Open…
            <DropdownMenuShortcut>{MODIFIER}+O</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => void save(false)}>
            <SaveIcon />
            Save
            <DropdownMenuShortcut>{MODIFIER}+S</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void save(true)}>
            <SaveIcon />
            Save as…
            <DropdownMenuShortcut>{MODIFIER}+Shift+S</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={onExport}>
            <UploadIcon />
            Export…
            <DropdownMenuShortcut>{MODIFIER}+E</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={onOpenSettings}>
            <SettingsIcon />
            Settings…
            <DropdownMenuShortcut>{MODIFIER}+,</DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onShowShortcuts}>
            <KeyboardIcon />
            Keyboard shortcuts
            <DropdownMenuShortcut>?</DropdownMenuShortcut>
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
          <>
            <span
              aria-hidden
              className="size-1.5 shrink-0 rounded-full bg-primary"
              title="Unsaved changes"
            />
            <span className="sr-only">Unsaved changes</span>
          </>
        ) : null}
        {path ? (
          <span className="hidden truncate text-[11px] text-muted-foreground xl:inline">
            {path}
          </span>
        ) : project ? (
          // Worth saying out loud rather than leaving blank: a project with no
          // path is one crash away from being gone.
          <span className="hidden text-[11px] text-muted-foreground xl:inline">Never saved</span>
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
