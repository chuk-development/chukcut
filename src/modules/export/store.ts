/**
 * Running exports.
 *
 * A store rather than dialog state because an export outlives the dialog. The
 * user presses Export, the dialog closes, and they keep cutting while the
 * encode runs — so the job rows, the channel subscription and the cancel
 * button all have to live above any component that happens to be mounted.
 *
 * ## Why a message can create a job
 *
 * `export_start` returns the job id, and the encode thread starts sending
 * before that reply has crossed back. For a short export the terminal message
 * can genuinely arrive first. So the registry is keyed by `job_id` and a
 * message for an id nobody has seen opens the row; the reply then fills in the
 * destination. The other order — waiting for the id and dropping what arrives
 * first — leaves a finished export stuck on "Preparing" forever.
 */

import { create } from "zustand";

import {
  type ExportOptions,
  type ExportProgress,
  type ExportRequest,
  exportCancel,
  exportPresets,
  exportStart,
  newProgressChannel,
} from "@/modules/export/lib/api";
import { applyProgress, type ExportJob, isTerminalStage, newJob } from "@/modules/export/lib/progress";
import { describeError } from "@/modules/project/store";

interface ExportState {
  /** Presets and hardware, fetched once. Null until the first dialog opens. */
  options: ExportOptions | null;
  loadingOptions: boolean;
  optionsError: string | null;

  jobs: ExportJob[];
  /** `export_start` is in flight. */
  starting: boolean;
  /** Why the last start was refused, in Rust's own words. */
  startError: string | null;

  loadOptions: (force?: boolean) => Promise<void>;
  /** Returns the job id, or null when Rust refused the settings. */
  start: (request: ExportRequest) => Promise<string | null>;
  cancel: (jobId: string) => Promise<void>;
  dismiss: (jobId: string) => void;
  clearStartError: () => void;
  /** Fold one progress message in. Exported for the channel callback and for tests. */
  receive: (progress: ExportProgress) => void;
}

export const useExportStore = create<ExportState>((set, get) => ({
  options: null,
  loadingOptions: false,
  optionsError: null,
  jobs: [],
  starting: false,
  startError: null,

  loadOptions: async (force = false) => {
    if (!force && (get().options || get().loadingOptions)) return;
    set({ loadingOptions: true, optionsError: null });
    try {
      set({ options: await exportPresets(), loadingOptions: false });
    } catch (error) {
      // Hardware detection is the part that can go wrong on a strange machine,
      // and it is the part the user cannot fix. Say so rather than showing an
      // empty preset list that looks like a bug.
      set({ loadingOptions: false, optionsError: describeError(error) });
    }
  },

  start: async (request) => {
    set({ starting: true, startError: null });

    const channel = newProgressChannel();
    channel.onmessage = (progress) => get().receive(progress);

    try {
      const jobId = await exportStart(request, channel);
      set((state) => ({
        starting: false,
        jobs: state.jobs.some((job) => job.id === jobId)
          ? // Progress beat the reply. Only the destination is missing.
            state.jobs.map((job) =>
              job.id === jobId
                ? { ...job, outputPath: job.outputPath || request.output_path }
                : job,
            )
          : [...state.jobs, newJob(jobId, request.output_path)],
      }));
      return jobId;
    } catch (error) {
      // A rejected export never started, so there is no job and no terminal
      // message coming — the dialog has to show this itself.
      set({ starting: false, startError: describeError(error) });
      return null;
    }
  },

  cancel: async (jobId) => {
    set((state) => ({
      jobs: state.jobs.map((job) => (job.id === jobId ? { ...job, cancelling: true } : job)),
    }));
    try {
      // `false` means it had already finished. Not an error: the terminal
      // message is the one that closes the card, and it is already on its way.
      await exportCancel(jobId);
    } catch (error) {
      set((state) => ({
        jobs: state.jobs.map((job) =>
          job.id === jobId
            ? { ...job, cancelling: false, message: describeError(error) }
            : job,
        ),
      }));
    }
  },

  dismiss: (jobId) =>
    set((state) => ({ jobs: state.jobs.filter((job) => job.id !== jobId) })),

  clearStartError: () => set({ startError: null }),

  receive: (progress) =>
    set((state) => {
      const index = state.jobs.findIndex((job) => job.id === progress.job_id);
      if (index === -1) {
        return {
          jobs: [...state.jobs, applyProgress(newJob(progress.job_id, ""), progress)],
        };
      }
      const jobs = state.jobs.slice();
      jobs[index] = applyProgress(jobs[index], progress);
      return { jobs };
    }),
}));

/** Jobs still encoding. The header uses the count to show that work is running. */
export function runningJobs(jobs: ExportJob[]): ExportJob[] {
  return jobs.filter((job) => !isTerminalStage(job.stage));
}
