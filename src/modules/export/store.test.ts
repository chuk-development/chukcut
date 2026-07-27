/**
 * The export flow, driven through the real IPC plumbing.
 *
 * The interesting parts are the ones that only exist because Rust is on the
 * other end: the payload as it crosses (snake_case in the request, camelCase
 * around it), progress arriving on the caller's own channel, a terminal message
 * that can beat the reply carrying the job id, and a cancel whose `false` means
 * "already finished" rather than "failed".
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { ExportRequest } from "@/modules/export/lib/api";
import { useExportStore } from "@/modules/export/store";
import { makeExportOptions, makeProgress, NVENC_H264, VAAPI_H264 } from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

const REQUEST: ExportRequest = {
  output_path: "/home/me/cut.mp4",
  preset_id: "youtube_1080p",
  overrides: {
    width: 1920,
    height: 1080,
    fps: 30,
    video_codec: "h264",
    quality: { kind: "crf", value: 20 },
    audio_codec: "aac",
    container: "mp4",
  },
  range: null,
  hardware: null,
  include_audio: true,
};

let ipc: IpcHarness;

function reset() {
  useExportStore.setState({
    options: null,
    loadingOptions: false,
    optionsError: null,
    jobs: [],
    starting: false,
    startError: null,
  });
}

beforeEach(() => {
  ipc = installIpc();
  reset();
});

afterEach(() => {
  ipc.restore();
  reset();
});

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

describe("export_presets", () => {
  it("is asked for once and kept", async () => {
    ipc.handle("export_presets", makeExportOptions({ hardware: [NVENC_H264, VAAPI_H264] }));

    await useExportStore.getState().loadOptions();
    await useExportStore.getState().loadOptions();

    expect(ipc.count("export_presets")).toBe(1);
    expect(useExportStore.getState().options?.default_preset_id).toBe("custom");
    expect(useExportStore.getState().options?.hardware).toHaveLength(2);
  });

  it("is asked again when the dialog retries", async () => {
    ipc.handle("export_presets", makeExportOptions());
    await useExportStore.getState().loadOptions();

    await useExportStore.getState().loadOptions(true);

    expect(ipc.count("export_presets")).toBe(2);
  });

  it("keeps Rust's sentence when detection fails instead of showing an empty list", async () => {
    ipc.fail("export_presets", "this machine has no GPU that can render frames");

    await useExportStore.getState().loadOptions();

    expect(useExportStore.getState().options).toBeNull();
    expect(useExportStore.getState().optionsError).toBe(
      "this machine has no GPU that can render frames",
    );
  });
});

// ---------------------------------------------------------------------------
// Starting
// ---------------------------------------------------------------------------

describe("export_start", () => {
  it("sends the request and a channel, and nothing else", async () => {
    ipc.handle("export_start", "job-1");

    await useExportStore.getState().start(REQUEST);

    const payload = ipc.lastCall("export_start");
    expect(Object.keys(payload ?? {}).sort()).toEqual(["onProgress", "request"]);
    // The argument names are camelCase because Tauri maps them onto the
    // command's snake_case parameters; the struct inside is not.
    expect(payload?.onProgress).toMatch(/^__CHANNEL__:\d+$/);
    expect(payload?.request).toEqual({
      output_path: "/home/me/cut.mp4",
      preset_id: "youtube_1080p",
      overrides: {
        width: 1920,
        height: 1080,
        fps: 30,
        video_codec: "h264",
        quality: { kind: "crf", value: 20 },
        audio_codec: "aac",
        container: "mp4",
      },
      hardware: null,
      include_audio: true,
      range: null,
    });
  });

  it("never camelCases a field of the request on the way out", async () => {
    ipc.handle("export_start", "job-1");

    await useExportStore.getState().start({ ...REQUEST, hardware: "nvenc_h264" });

    const request = ipc.lastCall("export_start")?.request as Record<string, unknown>;
    const overrides = request.overrides as Record<string, unknown>;
    for (const key of [...Object.keys(request), ...Object.keys(overrides)]) {
      expect(key, `"${key}" must not be camelCase`).toBe(key.toLowerCase());
    }
  });

  it("opens a job the moment the id comes back", async () => {
    ipc.handle("export_start", "job-1");

    const id = await useExportStore.getState().start(REQUEST);

    expect(id).toBe("job-1");
    expect(useExportStore.getState().jobs).toHaveLength(1);
    expect(useExportStore.getState().jobs[0]).toMatchObject({
      id: "job-1",
      stage: "preparing",
      outputPath: "/home/me/cut.mp4",
    });
    expect(useExportStore.getState().starting).toBe(false);
  });

  it("opens no job when Rust refuses the settings, and keeps the reason", async () => {
    ipc.fail("export_start", "the timeline is empty, so there is nothing to export");

    const id = await useExportStore.getState().start(REQUEST);

    expect(id).toBeNull();
    // Nothing started, so no terminal message is coming — the dialog is the
    // only place this can be seen.
    expect(useExportStore.getState().jobs).toEqual([]);
    expect(useExportStore.getState().startError).toBe(
      "the timeline is empty, so there is nothing to export",
    );
    expect(useExportStore.getState().starting).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

describe("progress on the channel", () => {
  it("reaches the job it belongs to, in order", async () => {
    ipc.handle("export_start", "job-1");
    await useExportStore.getState().start(REQUEST);

    const wire = ipc.channel("export_start");
    wire.emit(makeProgress({ stage: "preparing" }));
    wire.emit(makeProgress({ stage: "mixing_audio" }));
    wire.emit(
      makeProgress({
        stage: "encoding",
        frame: 300,
        fraction: 1 / 3,
        fps: 20,
        remaining_seconds: 30,
      }),
    );

    const job = useExportStore.getState().jobs[0];
    expect(job.stage).toBe("encoding");
    expect(job.frame).toBe(300);
    expect(job.totalFrames).toBe(900);
    expect(job.fps).toBe(20);
    expect(job.remainingSeconds).toBe(30);
  });

  it("does not leak into another export's card", async () => {
    ipc.handle("export_start", (_payload, index) => `job-${index + 1}`);
    await useExportStore.getState().start(REQUEST);
    await useExportStore.getState().start({ ...REQUEST, output_path: "/home/me/second.mp4" });

    ipc.channel("export_start", 0).emit(makeProgress({ job_id: "job-1", frame: 100 }));

    const [first, second] = useExportStore.getState().jobs;
    expect(first.frame).toBe(100);
    expect(second.frame).toBe(0);
  });

  it("opens the job when a message beats the reply carrying the id", async () => {
    // The encode thread sends `preparing` before `export_start` has returned,
    // and for a short export the terminal message can genuinely arrive first.
    // Dropping what arrives early leaves a finished export stuck on "Preparing".
    let wire: ReturnType<IpcHarness["channel"]> | null = null;
    ipc.handle("export_start", () => {
      wire = ipc.channel("export_start");
      wire.emit(makeProgress({ job_id: "job-1", stage: "done", frame: 900, fraction: 1 }));
      return "job-1";
    });

    await useExportStore.getState().start(REQUEST);

    const jobs = useExportStore.getState().jobs;
    expect(jobs).toHaveLength(1);
    expect(jobs[0].stage).toBe("done");
    // The reply is what knows where the file was asked to go.
    expect(jobs[0].outputPath).toBe("/home/me/cut.mp4");
  });

  it("lets the terminal message correct the extension the dialog asked for", async () => {
    ipc.handle("export_start", "job-1");
    await useExportStore.getState().start({ ...REQUEST, output_path: "/home/me/cut.webm" });

    ipc
      .channel("export_start")
      .emit(
        makeProgress({ stage: "done", frame: 900, fraction: 1, output_path: "/home/me/cut.mp4" }),
      );

    expect(useExportStore.getState().jobs[0].outputPath).toBe("/home/me/cut.mp4");
  });

  it("shows a failure in Rust's own words", async () => {
    ipc.handle("export_start", "job-1");
    await useExportStore.getState().start(REQUEST);

    const wire = ipc.channel("export_start");
    wire.emit(makeProgress({ stage: "encoding", frame: 12, fraction: 0.01 }));
    wire.emit(
      makeProgress({
        stage: "failed",
        frame: 0,
        fraction: 0,
        message: "this build of FFmpeg has no libsvtav1 encoder",
      }),
    );

    const job = useExportStore.getState().jobs[0];
    expect(job.stage).toBe("failed");
    expect(job.message).toBe("this build of FFmpeg has no libsvtav1 encoder");
    // The failure snapshot reports frame 0; the card still says where it got to.
    expect(job.frame).toBe(12);
  });
});

// ---------------------------------------------------------------------------
// Cancelling
// ---------------------------------------------------------------------------

describe("export_cancel", () => {
  it("names the job with a camelCase argument", async () => {
    ipc.handle("export_start", "job-1");
    ipc.handle("export_cancel", true);
    await useExportStore.getState().start(REQUEST);

    await useExportStore.getState().cancel("job-1");

    expect(ipc.lastCall("export_cancel")).toEqual({ jobId: "job-1" });
  });

  it("waits for the terminal message rather than closing the card itself", async () => {
    ipc.handle("export_start", "job-1");
    ipc.handle("export_cancel", true);
    await useExportStore.getState().start(REQUEST);
    ipc
      .channel("export_start")
      .emit(makeProgress({ stage: "encoding", frame: 450, fraction: 0.5 }));

    await useExportStore.getState().cancel("job-1");

    // Still running as far as the UI is concerned — Rust unwinds within a
    // frame and says so, and only then does the card settle.
    expect(useExportStore.getState().jobs[0]).toMatchObject({
      stage: "encoding",
      cancelling: true,
    });

    ipc
      .channel("export_start")
      .emit(makeProgress({ stage: "cancelled", frame: 0, fraction: 0, message: "cancelled" }));

    expect(useExportStore.getState().jobs[0].stage).toBe("cancelled");
    expect(useExportStore.getState().jobs[0].frame).toBe(450);
  });

  it("treats `false` as 'it already finished', not as an error", async () => {
    ipc.handle("export_start", "job-1");
    // The dialog closing on the last frame is a race nobody can win.
    ipc.handle("export_cancel", false);
    await useExportStore.getState().start(REQUEST);
    ipc.channel("export_start").emit(makeProgress({ stage: "done", frame: 900, fraction: 1 }));

    await useExportStore.getState().cancel("job-1");

    const job = useExportStore.getState().jobs[0];
    expect(job.stage).toBe("done");
    expect(job.message).toBeNull();
  });

  it("surfaces a cancel that could not be delivered", async () => {
    ipc.handle("export_start", "job-1");
    ipc.fail("export_cancel", "the backend went away");
    await useExportStore.getState().start(REQUEST);

    await useExportStore.getState().cancel("job-1");

    expect(useExportStore.getState().jobs[0]).toMatchObject({
      cancelling: false,
      message: "the backend went away",
    });
  });

  it("cancels only the job it was given", async () => {
    ipc.handle("export_start", (_payload, index) => `job-${index + 1}`);
    ipc.handle("export_cancel", true);
    await useExportStore.getState().start(REQUEST);
    await useExportStore.getState().start(REQUEST);

    await useExportStore.getState().cancel("job-2");

    const [first, second] = useExportStore.getState().jobs;
    expect(first.cancelling).toBe(false);
    expect(second.cancelling).toBe(true);
  });
});

describe("dismissing a card", () => {
  it("takes that job off the dock and leaves the others", async () => {
    ipc.handle("export_start", (_payload, index) => `job-${index + 1}`);
    await useExportStore.getState().start(REQUEST);
    await useExportStore.getState().start(REQUEST);

    useExportStore.getState().dismiss("job-1");

    expect(useExportStore.getState().jobs.map((job) => job.id)).toEqual(["job-2"]);
  });
});
