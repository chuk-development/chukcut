/**
 * The panel that answers "is it actually using my graphics card".
 *
 * The case worth testing is the disappointing one: a machine where nothing is
 * usable. That has three distinguishable shapes and conflating any two of them
 * is a lie — nothing present at all, present and refused by the driver, and
 * "nobody asked" because the command that would answer does not exist. Each has
 * to read differently on screen.
 */

import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { HardwarePanel } from "@/modules/workspace/components/HardwarePanel";
import { useWorkspaceStore } from "@/modules/workspace/store";
import { EMPTY_HARDWARE, type HardwareReport, type HwEncoder } from "@/modules/workspace/types";
import { type IpcHarness, installIpc } from "@/test/ipc";

/** Present in the build, backed by a device, and refused when actually run. */
const REFUSED_AV1: HwEncoder = {
  id: "vaapi_av1",
  accel: "vaapi",
  codec: "av1",
  encoder_name: "av1_vaapi",
  label: "AV1 (VAAPI)",
  available: true,
  usable: false,
  note: "VAAPI is present but this driver could not encode a test frame with it (Invalid argument). Software encoding still works.",
};

const NOTHING_USABLE: HardwareReport = {
  gpu: {
    name: "Intel Raptor Lake-P",
    backend: "Vulkan",
    driver: "Mesa 24.0",
    device_type: "integrated-gpu",
  },
  encoders: [REFUSED_AV1],
  decoders: [
    {
      codec: "av1",
      decoder_name: "libdav1d",
      in_build: true,
      declares_vaapi: false,
      usable: false,
      note: null,
    },
  ],
  decode_default: "software",
};

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  useWorkspaceStore.setState({
    hardware: EMPTY_HARDWARE,
    hardwareStatus: "idle",
    hardwareError: null,
  });
});

afterEach(() => {
  ipc.restore();
});

describe("a machine where nothing is usable", () => {
  beforeEach(() => {
    ipc.handle("workspace_hardware", NOTHING_USABLE);
  });

  it("still names the GPU, because that is the question being asked", async () => {
    render(<HardwarePanel />);

    expect(await screen.findByText("Intel Raptor Lake-P")).toBeInTheDocument();
    expect(screen.getByText(/Vulkan/)).toBeInTheDocument();
  });

  it("says nothing could be driven, rather than leaving the sections blank", async () => {
    render(<HardwarePanel />);

    expect(await screen.findByText(/Nothing on this machine could be driven/)).toBeInTheDocument();
  });

  it("shows the refused encoder with the driver's reason, not as a missing row", async () => {
    render(<HardwarePanel />);

    // A silently absent option is how "your GPU is not supported" becomes a bug
    // report. The reason is the whole value of the probe.
    expect(await screen.findByText("AV1 (VAAPI)")).toBeInTheDocument();
    expect(screen.getByText(/could not encode a test frame/)).toBeInTheDocument();
    expect(
      screen.getByText(/Every encoder in this build was tried and refused/),
    ).toBeInTheDocument();
  });

  it("explains a decoder that has no hardware path rather than printing nothing", async () => {
    render(<HardwarePanel />);

    expect(await screen.findByText("libdav1d")).toBeInTheDocument();
    expect(screen.getByText(/no hardware path in this FFmpeg build/)).toBeInTheDocument();
  });
});

describe("a machine with no hardware video path at all", () => {
  it("says that is fine and what happens instead", async () => {
    ipc.handle("workspace_hardware", {
      gpu: null,
      encoders: [],
      decoders: [],
      decode_default: null,
    } satisfies HardwareReport);
    render(<HardwarePanel />);

    expect(await screen.findByText("No hardware video path on this machine")).toBeInTheDocument();
    expect(screen.getByText(/Export and preview run on the CPU/)).toBeInTheDocument();
  });
});

describe("when the engine can only answer half of it", () => {
  it("falls back to the encoder list the export dialog already uses", async () => {
    // `workspace_hardware` is not registered in Rust yet; `export_presets` is.
    ipc.fail("workspace_hardware", "Command workspace_hardware not found");
    ipc.handle("export_presets", {
      presets: [],
      hardware: [REFUSED_AV1],
      default_preset_id: "custom",
    });
    render(<HardwarePanel />);

    expect(await screen.findByText("AV1 (VAAPI)")).toBeInTheDocument();
    // And says the empty decoder section means "not asked", not "not supported".
    expect(screen.getByText(/only reports the encoder probe/)).toBeInTheDocument();
    expect(screen.getByText("Adapter not reported")).toBeInTheDocument();
  });

  it("offers a retry when neither source answers", async () => {
    ipc.fail("workspace_hardware", "Command workspace_hardware not found");
    ipc.fail("export_presets", "this machine has no GPU that can render frames");
    render(<HardwarePanel />);

    expect(
      await screen.findByText("this machine has no GPU that can render frames"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Try again/ })).toBeInTheDocument();
  });
});
