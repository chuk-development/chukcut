/**
 * The fold from `ExportProgress` to a job row.
 *
 * Every case here is one where showing the message literally would be wrong:
 * the terminal `cancelled` snapshot rewinding the counters, a stray message
 * after the end reopening a finished job, a zero total dividing the bar. The
 * contract on the Rust side is "exactly one terminal message"; this is what
 * makes a violation of it look like a bug in Rust rather than a broken card.
 */

import { describe, expect, it } from "vitest";

import {
  applyProgress,
  type ExportJob,
  fileNameOf,
  formatPercent,
  formatRemaining,
  isTerminalStage,
  newJob,
  stageLabel,
} from "@/modules/export/lib/progress";
import { makeProgress } from "@/test/fixtures";

/** The state a job is in halfway through a 900 frame encode. */
function halfway(): ExportJob {
  return applyProgress(
    newJob("job-1", "/home/me/cut.mp4"),
    makeProgress({
      stage: "encoding",
      frame: 450,
      total_frames: 900,
      fraction: 0.5,
      fps: 24.5,
      elapsed_seconds: 18.4,
      remaining_seconds: 18.4,
    }),
  );
}

describe("folding a message in", () => {
  it("carries the counters, the rate and the estimate through", () => {
    const job = halfway();

    expect(job.stage).toBe("encoding");
    expect(job.frame).toBe(450);
    expect(job.totalFrames).toBe(900);
    expect(job.fraction).toBe(0.5);
    expect(job.fps).toBe(24.5);
    expect(job.remainingSeconds).toBe(18.4);
  });

  it("walks the stages in the order Rust sends them", () => {
    let job = newJob("job-1", "/home/me/cut.mp4");
    for (const stage of ["preparing", "mixing_audio", "encoding", "finalizing"] as const) {
      job = applyProgress(job, makeProgress({ stage }));
      expect(job.stage).toBe(stage);
    }
    expect(isTerminalStage(job.stage)).toBe(false);
  });

  it("takes the corrected output path off the terminal message", () => {
    // The dialog asked for `.webm`; the container was MP4, so Rust wrote
    // `.mp4` and says so. Showing the requested name would name a file that
    // does not exist.
    const job = applyProgress(
      newJob("job-1", "/home/me/cut.webm"),
      makeProgress({
        stage: "done",
        frame: 900,
        fraction: 1,
        output_path: "/home/me/cut.mp4",
      }),
    );

    expect(job.outputPath).toBe("/home/me/cut.mp4");
  });

  it("keeps the destination when a message carries none", () => {
    const job = applyProgress(newJob("job-1", "/home/me/cut.mp4"), makeProgress());

    expect(job.outputPath).toBe("/home/me/cut.mp4");
  });

  it("finishes at a full bar", () => {
    const job = applyProgress(
      halfway(),
      makeProgress({ stage: "done", frame: 900, fraction: 1, remaining_seconds: 0 }),
    );

    expect(job.stage).toBe("done");
    expect(job.frame).toBe(900);
    expect(formatPercent(job.fraction)).toBe("100%");
  });
});

describe("the terminal snapshot of a run that did not finish", () => {
  it("does not rewind the counters when the export is cancelled", () => {
    // `run_export` takes the cancelled/failed snapshot at frame 0, because the
    // tracker is read fresh. Shown literally, a cancel at 50% flashes back to
    // "0 of 900 frames" on its way out.
    const job = applyProgress(
      halfway(),
      makeProgress({
        stage: "cancelled",
        frame: 0,
        fraction: 0,
        message: "the export was stopped",
      }),
    );

    expect(job.stage).toBe("cancelled");
    expect(job.frame).toBe(450);
    expect(job.fraction).toBe(0.5);
    expect(job.message).toBe("the export was stopped");
  });

  it("does the same for a failure, and keeps Rust's sentence verbatim", () => {
    const job = applyProgress(
      halfway(),
      makeProgress({
        stage: "failed",
        frame: 0,
        fraction: 0,
        message: "cannot read /media/b.mp4: no such file",
      }),
    );

    expect(job.stage).toBe("failed");
    expect(job.frame).toBe(450);
    expect(job.message).toBe("cannot read /media/b.mp4: no such file");
  });

  it("drops the estimate, because nothing is coming", () => {
    const job = applyProgress(halfway(), makeProgress({ stage: "cancelled", frame: 0 }));

    expect(job.remainingSeconds).toBeNull();
  });
});

describe("a job that has already ended", () => {
  it("ignores anything that arrives after the terminal message", () => {
    const finished = applyProgress(
      halfway(),
      makeProgress({ stage: "done", frame: 900, fraction: 1 }),
    );

    const after = applyProgress(finished, makeProgress({ stage: "encoding", frame: 12 }));

    expect(after).toBe(finished);
  });

  it("cannot be turned from cancelled into failed by a second terminal message", () => {
    const cancelled = applyProgress(halfway(), makeProgress({ stage: "cancelled" }));

    expect(applyProgress(cancelled, makeProgress({ stage: "failed", message: "x" })).stage).toBe(
      "cancelled",
    );
  });
});

describe("numbers that would break the bar", () => {
  it("clamps a fraction outside 0..1", () => {
    const job = newJob("job-1", "/tmp/a.mp4");

    expect(applyProgress(job, makeProgress({ fraction: 1.4 })).fraction).toBe(1);
    expect(applyProgress(job, makeProgress({ fraction: -0.2 })).fraction).toBe(0);
    expect(applyProgress(job, makeProgress({ fraction: Number.NaN })).fraction).toBe(0);
  });

  it("keeps the last known total rather than adopting a zero", () => {
    // `preparing` and `mixing_audio` are sent before the count is meaningful.
    const job = applyProgress(halfway(), makeProgress({ stage: "finalizing", total_frames: 0 }));

    expect(job.totalFrames).toBe(900);
  });
});

describe("what the card reads", () => {
  it("floors the percentage so nothing claims 100% before the last frame", () => {
    expect(formatPercent(0)).toBe("0%");
    expect(formatPercent(0.999)).toBe("99%");
    expect(formatPercent(1)).toBe("100%");
  });

  it("says nothing at all when there is no estimate yet", () => {
    // Rust sends null until the first frame is done; an ETA from no samples is
    // an invented number, and one that opens by claiming four hours is worse
    // than one that waits a second.
    expect(formatRemaining(null)).toBeNull();
    expect(formatRemaining(-1)).toBeNull();
  });

  it("writes the estimate at the scale it is on", () => {
    expect(formatRemaining(0.2)).toBe("almost done");
    expect(formatRemaining(42)).toBe("42s left");
    expect(formatRemaining(125)).toBe("2:05 left");
    expect(formatRemaining(3 * 3600 + 20 * 60)).toBe("3h 20m left");
  });

  it("names every stage", () => {
    expect(stageLabel("mixing_audio")).toBe("Mixing audio");
    expect(stageLabel("done")).toBe("Export finished");
    expect(stageLabel("failed")).toBe("Export failed");
  });

  it("shortens a path to its file name, on either separator", () => {
    expect(fileNameOf("/home/me/videos/cut.mp4")).toBe("cut.mp4");
    expect(fileNameOf("C:\\videos\\cut.mp4")).toBe("cut.mp4");
    expect(fileNameOf("cut.mp4")).toBe("cut.mp4");
  });
});
