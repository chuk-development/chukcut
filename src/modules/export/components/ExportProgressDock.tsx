import {
  AlertTriangleIcon,
  CheckCircle2Icon,
  FolderOpenIcon,
  Loader2Icon,
  SlashIcon,
  XIcon,
} from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { IconTooltip } from "@/components/ui/tooltip";
import { revealInFileManager } from "@/lib/opener";
import { cn } from "@/lib/utils";
import {
  type ExportJob,
  fileNameOf,
  formatPercent,
  formatRemaining,
  isTerminalStage,
  stageLabel,
} from "@/modules/export/lib/progress";
import { useExportStore } from "@/modules/export/store";

/**
 * Where a running export lives.
 *
 * Deliberately not a dialog. Rust takes a snapshot of the document when the job
 * starts and renders from that, so the user can keep cutting while it encodes —
 * a modal over the top of that would be a lie about what the app is doing. The
 * dock sits over the bottom-right corner, takes pointer events only on the
 * cards themselves, and disappears when there is nothing to say.
 */
export function ExportProgressDock() {
  const jobs = useExportStore((s) => s.jobs);
  if (jobs.length === 0) return null;

  return (
    <aside
      data-slot="export-dock"
      aria-label="Exports"
      className="pointer-events-none fixed bottom-3 right-3 z-40 flex w-[320px] flex-col gap-2"
    >
      {jobs.map((job) => (
        <ExportCard key={job.id} job={job} />
      ))}
    </aside>
  );
}

function ExportCard({ job }: { job: ExportJob }) {
  const cancel = useExportStore((s) => s.cancel);
  const dismiss = useExportStore((s) => s.dismiss);
  const [revealError, setRevealError] = useState<string | null>(null);

  const terminal = isTerminalStage(job.stage);
  const failed = job.stage === "failed";
  const done = job.stage === "done";
  const remaining = formatRemaining(job.remainingSeconds);

  const reveal = async () => {
    setRevealError(null);
    try {
      await revealInFileManager(job.outputPath);
    } catch {
      // The file is written either way; a file manager that will not open is
      // worth a line, not an error banner across the editor.
      setRevealError(`The file is at ${job.outputPath}`);
    }
  };

  return (
    <section
      // Live but not assertive: an export finishing should be announced, not
      // interrupt what the user is typing.
      aria-live="polite"
      className={cn(
        "pointer-events-auto rounded-md border bg-popover px-3 py-2.5 shadow-2xl",
        failed ? "border-destructive/50" : "border-border",
      )}
    >
      <header className="flex items-center gap-2">
        <StageIcon job={job} />
        <span className="min-w-0 flex-1 truncate text-[12px] font-medium">
          {stageLabel(job.stage)}
        </span>
        {terminal ? (
          <IconTooltip label="Dismiss" side="left">
            <Button size="icon-sm" aria-label="Dismiss" onClick={() => dismiss(job.id)}>
              <XIcon />
            </Button>
          </IconTooltip>
        ) : (
          <Button
            size="sm"
            variant="outline"
            onClick={() => void cancel(job.id)}
            disabled={job.cancelling}
          >
            {job.cancelling ? "Stopping…" : "Cancel"}
          </Button>
        )}
      </header>

      <p className="mt-0.5 truncate text-[11px] text-muted-foreground" title={job.outputPath}>
        {job.outputPath ? fileNameOf(job.outputPath) : "…"}
      </p>

      {!terminal ? (
        <>
          {/* A determinate bar the moment there are frames to count, and an
              indeterminate stripe before that — mixing audio has no frame
              number, and a bar frozen at zero reads as a hang. */}
          <div className="mt-2 h-1 overflow-hidden rounded-full bg-input">
            {job.totalFrames > 0 ? (
              <div
                className="h-full rounded-full bg-primary transition-[width] duration-200"
                style={{ width: formatPercent(job.fraction) }}
              />
            ) : (
              <div className="h-full w-1/3 animate-pulse rounded-full bg-primary/70" />
            )}
          </div>

          <div className="mt-1.5 flex items-baseline gap-2 font-mono text-[10px] tabular-nums text-muted-foreground">
            <span className="text-foreground">{formatPercent(job.fraction)}</span>
            {job.totalFrames > 0 ? (
              <span>
                {job.frame.toLocaleString()} / {job.totalFrames.toLocaleString()} frames
              </span>
            ) : null}
            {job.fps > 0 ? <span className="ml-auto">{job.fps.toFixed(1)} fps</span> : null}
          </div>
          {remaining ? (
            <p className="mt-0.5 text-[10px] text-muted-foreground">{remaining}</p>
          ) : null}
        </>
      ) : null}

      {done ? (
        <div className="mt-2 flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={() => void reveal()}>
            <FolderOpenIcon />
            Show in folder
          </Button>
          <span className="truncate font-mono text-[10px] text-muted-foreground">
            {job.frame.toLocaleString()} frames in {job.elapsedSeconds.toFixed(1)}s
          </span>
        </div>
      ) : null}

      {/* Rust writes its failures as a sentence for the user, so it is shown
          exactly as it arrived rather than being wrapped in prose of our own. */}
      {job.message && job.stage !== "done" ? (
        <p className="mt-1.5 text-[11px] leading-snug text-foreground/90">{job.message}</p>
      ) : null}

      {revealError ? <p className="mt-1 text-[10px] text-muted-foreground">{revealError}</p> : null}
    </section>
  );
}

function StageIcon({ job }: { job: ExportJob }) {
  if (job.stage === "done") {
    return <CheckCircle2Icon className="size-3.5 shrink-0 text-track-audio" />;
  }
  if (job.stage === "failed") {
    return <AlertTriangleIcon className="size-3.5 shrink-0 text-destructive" />;
  }
  if (job.stage === "cancelled") {
    return <SlashIcon className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  return <Loader2Icon className="size-3.5 shrink-0 animate-spin text-primary" />;
}
