/**
 * Progress, as the dock reads it.
 *
 * A job row is not a copy of the last message. Two of Rust's messages would
 * read as a regression if they were shown literally:
 *
 * - the terminal `cancelled` / `failed` snapshot reports `frame: 0` and
 *   `fraction: 0`, because it is taken from a fresh tracker read and not from
 *   where the encode stopped. Shown as-is, a cancel at 80% would flash back to
 *   "0 of 900 frames" on its way out;
 * - anything arriving after a terminal message would reopen a finished job.
 *
 * So the fold is a function, and it is the piece worth testing.
 */

import type { ExportProgress, ExportStage } from "@/modules/export/lib/api";

/** One export the user can see. Wire fields folded into UI state. */
export interface ExportJob {
  id: string;
  stage: ExportStage;
  /** Frames finished. Frozen at the last encoded frame once the job settles. */
  frame: number;
  totalFrames: number;
  /** 0..1. */
  fraction: number;
  /** Encoding rate achieved, not the output frame rate. */
  fps: number;
  elapsedSeconds: number;
  remainingSeconds: number | null;
  /** Where the file is going. Rust corrects the extension, so its word wins. */
  outputPath: string;
  /** Prose from Rust, shown verbatim. */
  message: string | null;
  /** Cancel was asked for and the terminal message has not landed yet. */
  cancelling: boolean;
}

export function isTerminalStage(stage: ExportStage): boolean {
  return stage === "done" || stage === "cancelled" || stage === "failed";
}

export function newJob(id: string, outputPath: string): ExportJob {
  return {
    id,
    stage: "preparing",
    frame: 0,
    totalFrames: 0,
    fraction: 0,
    fps: 0,
    elapsedSeconds: 0,
    remainingSeconds: null,
    outputPath,
    message: null,
    cancelling: false,
  };
}

/**
 * Fold one message into a job.
 *
 * Pure and total: an out-of-contract message — a second terminal, a fraction
 * outside 0..1, a zero total — produces a sane row rather than a broken bar.
 */
export function applyProgress(job: ExportJob, progress: ExportProgress): ExportJob {
  // Rust promises exactly one terminal message. The guard is for the case where
  // it does not: a late `encoding` after `done` must not reopen the row.
  if (isTerminalStage(job.stage)) return job;

  const settled = progress.stage === "cancelled" || progress.stage === "failed";
  return {
    ...job,
    stage: progress.stage,
    frame: settled ? job.frame : progress.frame,
    totalFrames: progress.total_frames > 0 ? progress.total_frames : job.totalFrames,
    fraction: settled ? job.fraction : clamp01(progress.fraction),
    fps: settled ? job.fps : progress.fps,
    elapsedSeconds: progress.elapsed_seconds,
    remainingSeconds: settled ? null : progress.remaining_seconds,
    // Only the terminal message carries a path, and it is the corrected one.
    outputPath: progress.output_path ?? job.outputPath,
    message: progress.message ?? job.message,
  };
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value));
}

const STAGE_LABELS: Record<ExportStage, string> = {
  preparing: "Preparing",
  mixing_audio: "Mixing audio",
  encoding: "Encoding",
  finalizing: "Finishing",
  done: "Export finished",
  cancelled: "Export cancelled",
  failed: "Export failed",
};

export function stageLabel(stage: ExportStage): string {
  return STAGE_LABELS[stage];
}

/** `43%`. Rounded down so nothing claims 100% before the last frame. */
export function formatPercent(fraction: number): string {
  return `${Math.floor(clamp01(fraction) * 100)}%`;
}

/**
 * `about 2:05 left`, or nothing at all.
 *
 * Null before the first frame is deliberate on the Rust side — an ETA built
 * from no samples is a made-up number, and a bar that opens by claiming four
 * hours is worse than one that says nothing yet.
 */
export function formatRemaining(seconds: number | null): string | null {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return null;
  const whole = Math.round(seconds);
  if (whole < 1) return "almost done";
  if (whole < 60) return `${whole}s left`;
  const minutes = Math.floor(whole / 60);
  const rest = whole % 60;
  if (minutes < 60) return `${minutes}:${String(rest).padStart(2, "0")} left`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m left`;
}

/** The file name out of a path, for a card that has no room for the directory. */
export function fileNameOf(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}
