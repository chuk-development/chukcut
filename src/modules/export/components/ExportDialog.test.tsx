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
import {
  makeExportOptions,
  makeProject,
  makeSegment,
  NVENC_H264,
  projectWithSegments,
  VAAPI_H264,
} from "@/test/fixtures";
import { type IpcHarness, installIpc } from "@/test/ipc";

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
    ipc.handle("plugin:dialog|save", "/home/me/cut.mp4");
    ipc.handle("export_start", "job-1");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await choose("Encoder", /NVENC/);
    await userEvent.setup().click(screen.getByRole("button", { name: /Choose/ }));
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
  it("will not export until the user has chosen, and says so", async () => {
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    expect(screen.getByRole("button", { name: "Export" })).toBeDisabled();
    expect(screen.getByText(/Choose where/)).toBeInTheDocument();
  });

  it("opens a save dialog next to the project, with the container's extension", async () => {
    ipc.handle("plugin:dialog|save", "/home/me/cut.mp4");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });

    await userEvent.setup().click(screen.getByRole("button", { name: /Choose/ }));

    // The webview never invents a path: this is the saved project's own, with
    // the container's extension on it.
    const options = ipc.lastCall("plugin:dialog|save")?.options as Record<string, unknown>;
    expect(options.defaultPath).toBe("/home/me/cut.mp4");
    expect(options.filters).toEqual([{ name: "MP4", extensions: ["mp4"] }]);
    expect(await screen.findByTitle("/home/me/cut.mp4")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export" })).toBeEnabled();
  });

  it("shows the name Rust will actually write when the container moves", async () => {
    ipc.handle("plugin:dialog|save", "/home/me/cut.mp4");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });
    await userEvent.setup().click(screen.getByRole("button", { name: /Choose/ }));
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
  async function chooseAndExport() {
    ipc.handle("plugin:dialog|save", "/home/me/cut.mp4");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /Choose/ }));
    await screen.findByTitle("/home/me/cut.mp4");
    await user.click(screen.getByRole("button", { name: "Export" }));
  }

  it("sends the settings on screen and gets out of the way", async () => {
    ipc.handle("export_start", "job-1");

    await chooseAndExport();

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
    });
    // The encode is minutes of work; a modal over it would be a lie.
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(useExportStore.getState().jobs).toHaveLength(1);
  });

  it("passes the audio toggle through", async () => {
    ipc.handle("export_start", "job-1");
    ipc.handle("plugin:dialog|save", "/home/me/cut.mp4");
    mount();
    await screen.findByRole("combobox", { name: "Preset" });
    const user = userEvent.setup();
    await user.click(screen.getByRole("switch", { name: "Include audio" }));
    await user.click(screen.getByRole("button", { name: /Choose/ }));
    await screen.findByTitle("/home/me/cut.mp4");
    await user.click(screen.getByRole("button", { name: "Export" }));

    await waitFor(() => expect(ipc.count("export_start")).toBe(1));
    expect(ipc.lastCall("export_start")?.request).toMatchObject({ include_audio: false });
  });

  it("stays open with Rust's sentence when the settings are refused", async () => {
    ipc.fail("export_start", "a .webm file cannot carry H.264 video");

    await chooseAndExport();

    expect(await screen.findByText("a .webm file cannot carry H.264 video")).toBeInTheDocument();
    // Closing would throw the only copy of the message away.
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(useExportStore.getState().jobs).toEqual([]);
  });
});
