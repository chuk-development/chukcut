/**
 * What the app opens onto when no project is open.
 *
 * It replaces an editor that silently created "Untitled" at launch. That was
 * hostile in a way that took a while to name: the user never chose anything, so
 * the first thing they did in a session was undo a decision the app had made
 * for them — and their previous work was not reachable from anywhere.
 *
 * Two columns, because there are exactly two things anyone wants here: start
 * something, or go back to something. Everything else is one click further in.
 */

import {
  AlertTriangleIcon,
  ClockIcon,
  FolderOpenIcon,
  LifeBuoyIcon,
  Loader2Icon,
  RotateCcwIcon,
  ScissorsLineDashedIcon,
  SettingsIcon,
  Trash2Icon,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { formatWhen, parentDirectory } from "@/modules/workspace/lib/format";
import {
  chooseAndOpenProject,
  createProject,
  openProjectAt,
} from "@/modules/workspace/lib/lifecycle";
import {
  discardRecovery,
  pendingRecovery,
  type RecoveredProject,
  restoreRecovery,
} from "@/modules/workspace/lib/recovery";
import { useWorkspaceStore } from "@/modules/workspace/store";
import { CANVAS_PRESETS, type CanvasPreset } from "@/modules/workspace/types";

interface StartScreenProps {
  /** The full new-project dialog, for a canvas that is not one of the three. */
  onMoreOptions: () => void;
  onOpenSettings: () => void;
}

export function StartScreen({ onMoreOptions, onOpenSettings }: StartScreenProps) {
  const recent = useWorkspaceStore((s) => s.recent);
  const recentStatus = useWorkspaceStore((s) => s.recentStatus);
  const recentError = useWorkspaceStore((s) => s.recentError);
  const refreshRecent = useWorkspaceStore((s) => s.refreshRecent);
  const settings = useWorkspaceStore((s) => s.settings);

  const [name, setName] = useState("Untitled");
  const [busy, setBusy] = useState<string | null>(null);
  const [recovered, setRecovered] = useState<RecoveredProject | null>(null);

  useEffect(() => {
    void refreshRecent();
  }, [refreshRecent]);

  // The recovery seam. `pendingRecovery` answers null both when there is
  // nothing to recover and when the engine cannot yet be asked, so this is
  // inert until the Rust side lands rather than being a broken banner.
  useEffect(() => {
    let cancelled = false;
    void pendingRecovery().then((found) => {
      if (!cancelled) setRecovered(found);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const start = useCallback(
    async (preset: CanvasPreset) => {
      setBusy(preset.id);
      try {
        await createProject({
          name: name.trim() || "Untitled",
          width: preset.width,
          height: preset.height,
          fps: settings.default_fps,
        });
      } finally {
        setBusy(null);
      }
    },
    [name, settings.default_fps],
  );

  const open = useCallback(async () => {
    setBusy("open");
    try {
      await chooseAndOpenProject();
    } finally {
      setBusy(null);
    }
  }, []);

  const openRecent = useCallback(async (path: string) => {
    setBusy(path);
    try {
      await openProjectAt(path);
    } finally {
      setBusy(null);
    }
  }, []);

  return (
    <main
      className="flex min-h-0 flex-1 flex-col overflow-y-auto bg-background"
      aria-label="Start"
      data-slot="start-screen"
    >
      <div className="mx-auto flex w-full max-w-[860px] flex-col gap-6 px-8 py-10">
        <header className="flex items-start justify-between gap-4">
          <div>
            <div className="flex items-center gap-2">
              <ScissorsLineDashedIcon className="size-5 text-primary" />
              <h1 className="text-[17px] font-semibold tracking-tight">chukcut</h1>
            </div>
            <p className="mt-1 text-[12px] text-muted-foreground">
              Pick a shape to start in, or reopen something you were working on.
            </p>
          </div>
          <Button size="sm" onClick={onOpenSettings}>
            <SettingsIcon />
            Settings
          </Button>
        </header>

        {recovered ? (
          <RecoveryBanner recovered={recovered} onDone={() => setRecovered(null)} />
        ) : null}

        <div className="grid grid-cols-1 gap-6 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
          {/* ---------------------------------------------------------- New */}
          <section className="flex flex-col gap-3" aria-label="New project">
            <h2 className="text-[12px] font-semibold text-panel-foreground">New project</h2>

            <div className="grid grid-cols-[64px_1fr] items-center gap-3 text-[12px]">
              <span className="text-muted-foreground">Name</span>
              <Input
                aria-label="Project name"
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </div>

            <div className="flex flex-col gap-1.5">
              {CANVAS_PRESETS.map((preset) => (
                <button
                  key={preset.id}
                  type="button"
                  onClick={() => void start(preset)}
                  disabled={busy !== null}
                  className="group flex items-center gap-3 rounded-md border border-border bg-surface px-3 py-2.5 text-left outline-none transition-colors hover:border-primary/60 hover:bg-accent focus-visible:ring-[2px] focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-50"
                >
                  <AspectGlyph preset={preset} />
                  <span className="min-w-0 flex-1">
                    <span className="flex items-baseline gap-1.5">
                      <span className="text-[12px] font-medium">{preset.label}</span>
                      <span className="text-[11px] text-muted-foreground">{preset.aspect}</span>
                    </span>
                    <span className="block truncate text-[11px] text-muted-foreground">
                      {preset.width} × {preset.height} · {preset.hint}
                    </span>
                  </span>
                  {busy === preset.id ? (
                    <Loader2Icon className="size-3.5 animate-spin text-muted-foreground" />
                  ) : null}
                </button>
              ))}
            </div>

            <p className="text-[11px] text-muted-foreground">
              All at {formatFps(settings.default_fps)} fps, from your defaults.{" "}
              <button
                type="button"
                onClick={onMoreOptions}
                className="text-foreground underline underline-offset-2 outline-none hover:text-primary focus-visible:ring-[2px] focus-visible:ring-ring/60"
              >
                Another canvas or frame rate…
              </button>
            </p>

            <Button
              variant="outline"
              size="md"
              className="w-full justify-center"
              onClick={() => void open()}
              disabled={busy !== null}
            >
              {busy === "open" ? <Loader2Icon className="animate-spin" /> : <FolderOpenIcon />}
              Open a project…
            </Button>
          </section>

          {/* ------------------------------------------------------- Recent */}
          <section className="flex min-h-0 flex-col gap-3" aria-label="Recent projects">
            <h2 className="text-[12px] font-semibold text-panel-foreground">Recent</h2>
            <RecentList
              status={recentStatus}
              error={recentError}
              entries={recent}
              busyPath={busy}
              onOpen={(path) => void openRecent(path)}
              onRetry={() => void refreshRecent()}
            />
          </section>
        </div>
      </div>
    </main>
  );
}

/** A filled rectangle in the preset's own proportions. Faster to read than "9:16". */
function AspectGlyph({ preset }: { preset: CanvasPreset }) {
  const tall = preset.height >= preset.width;
  const long = 26;
  const short = Math.round(
    (long * Math.min(preset.width, preset.height)) / Math.max(preset.width, preset.height),
  );
  return (
    <span
      aria-hidden
      className="shrink-0 rounded-[3px] border border-primary/50 bg-primary/20 transition-colors group-hover:border-primary group-hover:bg-primary/30"
      style={{ width: tall ? short : long, height: tall ? long : short }}
    />
  );
}

interface RecentListProps {
  status: "idle" | "loading" | "ready" | "error";
  error: string | null;
  entries: { path: string; name: string; opened_at: number }[];
  busyPath: string | null;
  onOpen: (path: string) => void;
  onRetry: () => void;
}

function RecentList({ status, error, entries, busyPath, onOpen, onRetry }: RecentListProps) {
  if (status === "error") {
    return (
      <div className="rounded-md border border-border bg-destructive/10 px-3 py-3">
        <p className="flex items-start gap-1.5 text-[11px] leading-snug text-destructive-foreground">
          <AlertTriangleIcon className="mt-px size-3 shrink-0" />
          <span className="min-w-0 flex-1">{error ?? "The recent list could not be read."}</span>
        </p>
        <Button size="sm" variant="outline" className="mt-2.5" onClick={onRetry}>
          <RotateCcwIcon />
          Try again
        </Button>
      </div>
    );
  }

  if (status === "idle" || status === "loading") {
    return (
      <div role="status" className="flex flex-col gap-1.5" aria-label="Loading recent projects">
        {[0, 1, 2].map((row) => (
          <div
            key={row}
            className="h-[46px] animate-pulse rounded-md border border-border bg-surface/60"
            style={{ animationDelay: `${row * 90}ms` }}
          />
        ))}
      </div>
    );
  }

  if (entries.length === 0) {
    return (
      <div className="rounded-md border border-dashed border-border px-4 py-8 text-center">
        <ClockIcon className="mx-auto size-5 text-muted-foreground" />
        <p className="mt-2 text-[12px] font-medium">No recent projects</p>
        <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground">
          Anything you save shows up here. Projects that have been moved or deleted drop off the
          list on their own.
        </p>
      </div>
    );
  }

  return (
    <ScrollArea className="max-h-[320px] min-h-0">
      <ul className="flex flex-col gap-1 pr-2">
        {entries.map((entry) => (
          <li key={entry.path}>
            <button
              type="button"
              onClick={() => onOpen(entry.path)}
              disabled={busyPath !== null}
              title={entry.path}
              className="flex w-full items-center gap-2.5 rounded-md border border-transparent bg-surface/60 px-3 py-2 text-left outline-none transition-colors hover:border-border hover:bg-accent focus-visible:ring-[2px] focus-visible:ring-ring/60 disabled:pointer-events-none disabled:opacity-50"
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[12px] font-medium">{entry.name}</span>
                <span className="block truncate text-[11px] text-muted-foreground">
                  {parentDirectory(entry.path)}
                </span>
              </span>
              <span className="shrink-0 text-[11px] text-muted-foreground">
                {busyPath === entry.path ? (
                  <Loader2Icon className="size-3.5 animate-spin" />
                ) : (
                  formatWhen(entry.opened_at)
                )}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </ScrollArea>
  );
}

function RecoveryBanner({
  recovered,
  onDone,
}: {
  recovered: RecoveredProject;
  onDone: () => void;
}) {
  const [busy, setBusy] = useState(false);

  return (
    <section
      className="flex items-start gap-3 rounded-md border border-primary/50 bg-primary/10 px-3.5 py-3"
      aria-label="Recovered work"
    >
      <LifeBuoyIcon className="mt-0.5 size-4 shrink-0 text-primary" />
      <div className="min-w-0 flex-1">
        <p className="text-[12px] font-medium">chukcut closed without saving “{recovered.name}”</p>
        <p className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
          A copy from {formatWhen(recovered.saved_at)} is still here
          {recovered.path ? `, for ${recovered.path}` : ""}. Restoring opens it as unsaved work;
          discarding deletes the copy.
        </p>
      </div>
      <div className="flex shrink-0 gap-1.5">
        <Button
          size="sm"
          onClick={async () => {
            setBusy(true);
            await discardRecovery();
            setBusy(false);
            onDone();
          }}
          disabled={busy}
        >
          <Trash2Icon />
          Discard
        </Button>
        <Button
          variant="default"
          size="sm"
          onClick={async () => {
            setBusy(true);
            const ok = await restoreRecovery();
            setBusy(false);
            if (ok) onDone();
          }}
          disabled={busy}
        >
          {busy ? <Loader2Icon className="animate-spin" /> : <RotateCcwIcon />}
          Restore
        </Button>
      </div>
    </section>
  );
}

/** 29.97 stays 29.97; 30 does not become 30.0. */
function formatFps(fps: number): string {
  return Number.isInteger(fps) ? String(fps) : fps.toFixed(2);
}
