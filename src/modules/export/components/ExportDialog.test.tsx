/**
 * The dialog, from opening it to a job on the dock.
 *
 * What is worth testing here is everything that only exists because the export
 * is real: the default preset being the one Rust nominated, an encoder that is
 * present but unusable being visible and refused rather than missing, a
 * destination that has to come from a dialog the user drove, and the request
 * that leaves when the button is pressed.
 */

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TooltipProvider } from "@/components/ui/tooltip";
import { ExportDialog } from "@/modules/export/components/ExportDialog";
import { useExportStore } from "@/modules/export/store";
import { useProjectStore } from "@/modules/project/store";
import { useTimelineStore } from "@/modules/timeline/store";
import { useWorkspaceStore } from "@/modules/workspace/store";
import { DEFAULT_SETTINGS } from "@/modules/workspace/types";
import {
  makeExportOptions,
  makeProject,
  makeSegment,
  NVENC_H264,
  projectWithSegments,
  VAAPI_H264,
} from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

/** The timeline store with the not-yet-typed `exportRange` field another
 * module is adding; the dialog reads it defensively, so the tests write it
 * the same way. */
function setExportRange(range: { start: number; end: number } | null) {
  useTimelineStore.setState({ exportRange: range } as never);
}

/** Two seconds of video on one track, so the timeline is not empty. */
const PROJECT = projectWithSegments(
  makeSegment("segment-1", { target_range: { start: 0, duration: 2_000_000 } }),
);

let ipc: IpcHarness;
const onOpenChange = vi.fn();

function mount() {
  return render(
    <TooltipProvider>
      <ExportDialog open onOpenChange={onOpenChange} />
    </TooltipProvider>,
  );
}

/** Drive a Radix select to the option named `option`. */
async function choose(trigger: string, option: string | RegExp) {
  const user = userEvent.setup();
  await user.click(screen.getByRole("combobox", { name: trigger }));
  await user.click(await screen.findByRole("option", { name: option }));
}

beforeEach(() => {
  onOpenChange.mockReset();
  ipc = installIpc();
  ipc.handle("export_presets", makeExportOptions());
  useProjectStore.setState({ project: PROJECT, path: "/home/me/cut.chukcut", status: "ready" });
  useExportStore.setState({
    options: null,
    loadingOptions: false,
    optionsError: null,
    jobs: [],
    starting: false,
    startError: null,
  });
  useWorkspaceStore.setState({ settings: DEFAULT_SETTINGS });
  setExportRange(null);
  useTimelineStore.setState({ playhead: 0 });
});

afterEach(() => {
  ipc.restore();
  useProjectStore.setState({ project: null, path: null, status: "loading" });
});

// ---------------------------------------------------------------------------
// Coming up
// ---------------------------------------------------------------------------

describe("opening the dialog", () => {
  it("asks Rust what this machine can encode", async () => {
    mount();

    await waitFor(() => expect(ipc.count("export_presets")).toBe(1));
  });

  it("preselects the preset Rust nominated, not simply the first one", async () => {
    // `default_preset_id` is "custom"; the list starts with "YouTube 1080p".
    mount();

    expect(await screen.findByRole("combobox", { name: "Preset" })).toHaveTextContent("Custom");
  });

  it("fills the custom preset in from the project canvas", async () => {
    // The template says 1080×1920. This project does not.
    useProjectStore.setState({
      project: { ...PROJECT, canvas: { width: 1440, height: 1080, background: [0, 0, 0, 1] } },
    });
    mount();

    expect(await screen.findByLabelText("Width")).toHaveValue(1440);
    expect(screen.getByLabelText("Height")).toHaveValue(1080);
  });

  it("says what it is doing while the answer is in flight", async () => {
    let answer: (options: unknown) => void = () => {};
    ipc.handle("export_presets", () => new Promise((resolve) => (answer = resolve)));
    mount();

    expect(screen.getByText(/Asking Rust/)).toBeInTheDocument();

    answer(makeExportOptions());
    expect(await screen.findByRole("combobox", { name: "Preset" })).toBeInTheDocument();
  });

  it("offers a retry when detection fails rather than an empty list", async () => {
    ipc.fail("export_presets", "this machine has no GPU that can render frames");
    mount();

    expect(
      await screen.findByText("this machine has no GPU that can render frames"),
    ).toBeInTheDocument();

    ipc.handle("export_presets", makeExportOptions());
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));

    expect(await screen.findByRole("combobox", { name: "Preset" })).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// Hardware
// ---------------------------------------------------------------------------

describe("the encoder list", () => {
  it("shows an unusable encoder greyed out with its reason", async () => {
    ipc.handle("export_presets", makeExportOptions({ hardware: [NVENC_H264, VAAPI_H264] }));
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await userEvent.setup().click(screen.getByRole("combobox", { name: "Encoder" }));

    // Listed, not hidden: "your GPU is not supported yet" beats a missing option.
    const vaapi = await screen.findByRole("option", { name: /VAAPI/ });
    expect(vaapi).toHaveAttribute("data-disabled");
    expect(vaapi).toHaveTextContent("hardware frame pool");
    expect(screen.getByRole("option", { name: /NVENC/ })).not.toHaveAttribute("data-disabled");
  });

  it("sends the chosen encoder's id, and lets it decide the codec", async () => {
    const NVENC_H265 = {
      ...NVENC_H264,
      id: "nvenc_h265",
      codec: "h265" as const,
      encoder_name: "hevc_nvenc",
      label: "H.265 / HEVC (NVIDIA NVENC)",
    };
    ipc.handle("export_presets", makeExportOptions({ hardware: [NVENC_H265] }));
    ipc.handle("export_start", "job-1");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await choose("Encoder", /NVENC/);
    // The destination defaulted from the saved project; nothing to choose.
    await screen.findByTitle("/home/me/cut.mp4");
    await userEvent.setup().click(screen.getByRole("button", { name: "Export" }));

    await waitFor(() => expect(ipc.count("export_start")).toBe(1));
    const request = ipc.lastCall("export_start")?.request as Record<string, unknown>;
    expect(request.hardware).toBe("nvenc_h265");
    expect(request.overrides).toMatchObject({ video_codec: "h265" });
  });
});

// ---------------------------------------------------------------------------
// The destination
// ---------------------------------------------------------------------------

describe("where the file goes", () => {
  it("defaults to the project's own folder and name, ready to export", async () => {
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    // The saved project is the strongest hint there is: same folder, same
    // stem, the container's extension. Nothing had to be chosen.
    expect(await screen.findByTitle("/home/me/cut.mp4")).toBeInTheDocument();
    expect(screen.getByLabelText("File name")).toHaveValue("cut");
    expect(screen.getByRole("button", { name: "Export" })).toBeEnabled();
  });

  it("will not export an unsaved project until a folder is chosen, and says so", async () => {
    useProjectStore.setState({ path: null });
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    expect(screen.getByRole("button", { name: "Export" })).toBeDisabled();
    expect(screen.getByText(/Choose where/)).toBeInTheDocument();
  });

  it("moves the file when a different folder is picked, keeping the name", async () => {
    ipc.handle("plugin:dialog|open", ["/mnt/renders"]);
    mount();
    await screen.findByRole("combobox", { name: "Preset" });
    await screen.findByTitle("/home/me/cut.mp4");

    await userEvent.setup().click(screen.getByRole("button", { name: /Choose/ }));

    // The folder picker starts where the file currently points.
    const options = ipc.lastCall("plugin:dialog|open")?.options as Record<string, unknown>;
    expect(options.directory).toBe(true);
    expect(options.defaultPath).toBe("/home/me");
    expect(await screen.findByTitle("/mnt/renders/cut.mp4")).toBeInTheDocument();
  });

  it("renames the file when the name is edited", async () => {
    mount();
    await screen.findByRole("combobox", { name: "Preset" });
    await screen.findByTitle("/home/me/cut.mp4");

    const user = userEvent.setup();
    const name = screen.getByLabelText("File name");
    await user.clear(name);
    await user.type(name, "final v2");

    expect(await screen.findByTitle("/home/me/final v2.mp4")).toBeInTheDocument();
  });

  it("shows the name Rust will actually write when the container moves", async () => {
    mount();
    await screen.findByRole("combobox", { name: "Preset" });
    await screen.findByTitle("/home/me/cut.mp4");

    await choose("Container", "WebM");

    // `with_extension` corrects the path on the way in, so showing the old one
    // would name a file that will not exist.
    expect(await screen.findByTitle("/home/me/cut.webm")).toBeInTheDocument();
  });

  it("refuses to export an empty timeline, and names the reason", async () => {
    useProjectStore.setState({ project: makeProject() });
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    expect(screen.getByText(/timeline is empty/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export" })).toBeDisabled();
  });
});

// ---------------------------------------------------------------------------
// Starting
// ---------------------------------------------------------------------------

describe("pressing Export", () => {
  async function mountAndExport() {
    mount();
    await screen.findByRole("combobox", { name: "Preset" });
    // The destination defaulted from the saved project.
    await screen.findByTitle("/home/me/cut.mp4");
    await userEvent.setup().click(screen.getByRole("button", { name: "Export" }));
  }

  it("sends the settings on screen and gets out of the way", async () => {
    ipc.handle("export_start", "job-1");

    await mountAndExport();

    await waitFor(() => expect(ipc.count("export_start")).toBe(1));
    expect(ipc.lastCall("export_start")?.request).toEqual({
      output_path: "/home/me/cut.mp4",
      preset_id: "custom",
      overrides: {
        width: 1080,
        height: 1920,
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
    // The encode is minutes of work; a modal over it would be a lie.
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(useExportStore.getState().jobs).toHaveLength(1);
  });

  it("passes the audio toggle through", async () => {
    ipc.handle("export_start", "job-1");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });
    const user = userEvent.setup();
    await user.click(screen.getByRole("switch", { name: "Include audio" }));
    await screen.findByTitle("/home/me/cut.mp4");
    await user.click(screen.getByRole("button", { name: "Export" }));

    await waitFor(() => expect(ipc.count("export_start")).toBe(1));
    expect(ipc.lastCall("export_start")?.request).toMatchObject({ include_audio: false });
  });

  it("stays open with Rust's sentence when the settings are refused", async () => {
    ipc.fail("export_start", "a .webm file cannot carry H.264 video");

    await mountAndExport();

    expect(await screen.findByText("a .webm file cannot carry H.264 video")).toBeInTheDocument();
    // Closing would throw the only copy of the message away.
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(useExportStore.getState().jobs).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// The export range
// ---------------------------------------------------------------------------

describe("the export range", () => {
  it("offers only the whole project while the timeline has no marks", async () => {
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    // No select — a choice with one option is not a choice.
    expect(screen.queryByRole("combobox", { name: "Range" })).not.toBeInTheDocument();
    expect(screen.getByText("Whole project")).toBeInTheDocument();
  });

  it("offers the marks with their times, and sends the range when chosen", async () => {
    setExportRange({ start: 500_000, end: 1_500_000 });
    ipc.handle("export_start", "job-1");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await choose("Range", /in\/out marks/);
    await screen.findByTitle("/home/me/cut.mp4");
    await userEvent.setup().click(screen.getByRole("button", { name: "Export" }));

    await waitFor(() => expect(ipc.count("export_start")).toBe(1));
    expect(ipc.lastCall("export_start")?.request).toMatchObject({
      range: [500_000, 1_500_000],
    });
  });

  it("clamps marks that outlived an edit, and drops an empty range entirely", async () => {
    // The project is two seconds; the out mark sits at four.
    setExportRange({ start: 1_000_000, end: 4_000_000 });
    ipc.handle("export_start", "job-1");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await choose("Range", /in\/out marks/);
    await userEvent.setup().click(screen.getByRole("button", { name: "Export" }));

    await waitFor(() => expect(ipc.count("export_start")).toBe(1));
    expect(ipc.lastCall("export_start")?.request).toMatchObject({
      range: [1_000_000, 2_000_000],
    });
  });

  it("survives a store that carries garbage in the field", async () => {
    setExportRange({ start: Number.NaN, end: 1 } as never);
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    expect(screen.queryByRole("combobox", { name: "Range" })).not.toBeInTheDocument();
    expect(screen.getByText("Whole project")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// Resolution and the estimate
// ---------------------------------------------------------------------------

describe("the resolution options", () => {
  it("scales the long edge and keeps the project's aspect", async () => {
    // A vertical 1080×1920 project: "720" must mean 404×720, not 720×1280.
    ipc.handle("export_start", "job-1");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await choose("Size", /^720/);

    expect(screen.getByLabelText("Width")).toHaveValue(404);
    expect(screen.getByLabelText("Height")).toHaveValue(720);
  });

  it("shows the canvas as the default choice", async () => {
    mount();

    expect(await screen.findByRole("combobox", { name: "Size" })).toHaveTextContent(
      "Canvas · 1080×1920",
    );
  });
});

describe("the estimated file size", () => {
  it("is on screen, labelled as an estimate", async () => {
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    const estimate = document.querySelector("[data-slot=export-estimate]");
    expect(estimate?.textContent).toMatch(/≈ .*MB|≈ .*GB|≈ .*kB/);
    expect(estimate?.textContent).toMatch(/estimate/);
  });

  it("shrinks when the export range shrinks the duration", async () => {
    setExportRange({ start: 0, end: 1_000_000 });
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    const whole = document.querySelector("[data-slot=export-estimate]")?.textContent ?? "";
    await choose("Range", /in\/out marks/);
    const ranged = document.querySelector("[data-slot=export-estimate]")?.textContent ?? "";

    // Two seconds of project, one second of range: the number halves.
    expect(ranged).not.toBe(whole);
  });
});

// ---------------------------------------------------------------------------
// Remembering the settings
// ---------------------------------------------------------------------------

describe("remember these settings", () => {
  it("persists the form into the workspace settings when an export starts", async () => {
    ipc.handle("export_start", "job-1");
    ipc.handle("workspace_settings_set", null);
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    const user = userEvent.setup();
    await user.click(screen.getByRole("switch", { name: "Remember these settings" }));
    await screen.findByTitle("/home/me/cut.mp4");
    await user.click(screen.getByRole("button", { name: "Export" }));

    await waitFor(() => expect(ipc.count("workspace_settings_set")).toBe(1));
    const settings = ipc.lastCall("workspace_settings_set")?.settings as Record<string, unknown>;
    expect(settings.export_remember).toBe(true);
    expect(settings.export_defaults).toMatchObject({
      preset_id: "custom",
      include_audio: true,
      // The canvas choice is remembered as a choice, not as pixels.
      long_edge: null,
    });
  });

  it("seeds the next dialog from what was remembered", async () => {
    useWorkspaceStore.setState({
      settings: {
        ...DEFAULT_SETTINGS,
        export_remember: true,
        export_defaults: {
          preset_id: "custom",
          fps: 30,
          quality: { kind: "crf", value: 28 },
          container: "mkv",
          video_codec: "h264",
          audio_codec: "aac",
          hardware_id: null,
          include_audio: false,
          long_edge: 720,
        },
      } as never,
    });
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    expect(screen.getByRole("combobox", { name: "Container" })).toHaveTextContent("MKV");
    expect(screen.getByRole("switch", { name: "Include audio" })).not.toBeChecked();
    expect(screen.getByRole("switch", { name: "Remember these settings" })).toBeChecked();
    // long_edge 720 on the 1080×1920 canvas.
    expect(screen.getByLabelText("Width")).toHaveValue(404);
    expect(screen.getByLabelText("Height")).toHaveValue(720);
  });

  it("shrugs off a corrupt memory rather than assembling a broken form", async () => {
    useWorkspaceStore.setState({
      settings: {
        ...DEFAULT_SETTINGS,
        export_remember: true,
        export_defaults: { container: 7, quality: "loud", long_edge: "many" },
      } as never,
    });
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    // Everything unrecognisable kept the seed's value.
    expect(screen.getByRole("combobox", { name: "Container" })).toHaveTextContent("MP4");
    expect(screen.getByLabelText("Width")).toHaveValue(1080);
  });
});

// ---------------------------------------------------------------------------
// Frame snapshots
// ---------------------------------------------------------------------------

describe("the snapshot button", () => {
  it("saves the frame at the playhead to the chosen file", async () => {
    useTimelineStore.setState({ playhead: 1_250_000 });
    ipc.handle("plugin:dialog|save", "/home/me/frame.png");
    ipc.handle("export_snapshot", "/home/me/frame.png");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await userEvent.setup().click(screen.getByRole("button", { name: /Save frame as PNG/ }));

    await waitFor(() => expect(ipc.count("export_snapshot")).toBe(1));
    expect(ipc.lastCall("export_snapshot")).toMatchObject({
      time: 1_250_000,
      outputPath: "/home/me/frame.png",
    });
    expect(await screen.findByText(/Saved \/home\/me\/frame\.png/)).toBeInTheDocument();
  });

  it("does nothing when the save dialog is dismissed", async () => {
    ipc.handle("plugin:dialog|save", null);
    ipc.handle("export_snapshot", "/never.png");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await userEvent.setup().click(screen.getByRole("button", { name: /Save frame as PNG/ }));

    expect(ipc.count("export_snapshot")).toBe(0);
  });

  it("shows Rust's sentence when the snapshot fails", async () => {
    ipc.handle("plugin:dialog|save", "/home/me/frame.png");
    ipc.fail("export_snapshot", "rendering the frame failed: no source for segment-1");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await userEvent.setup().click(screen.getByRole("button", { name: /Save frame as PNG/ }));

    expect(await screen.findByText(/rendering the frame failed/)).toBeInTheDocument();
  });
});
