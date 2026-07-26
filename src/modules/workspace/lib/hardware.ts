/**
 * What this machine can actually do, as far as the engine will say.
 *
 * ## Why there are two sources
 *
 * The engine establishes hardware support by *trying* it — `export/hwaccel.rs`
 * encodes a 320×240 frame with each encoder, `media/hwdecode.rs` decodes an
 * embedded bitstream with each decoder — because an encoder being present in
 * the FFmpeg build says nothing about whether the driver can drive it. Both
 * results exist in Rust and are cached for the process.
 *
 * Only one of them is reachable over IPC today: `export_presets` carries the
 * encoder list because the export dialog needs it. The decoder probe and the
 * identity of the GPU the compositor opened are not exposed by any command.
 *
 * So this asks for `workspace_hardware` first — the command that would answer
 * the whole question in one call — and falls back to the encoder half when it
 * is not registered. The fallback is marked `partial`, and the panel says so,
 * because an empty decoder list that means "nobody asked" looks exactly like an
 * empty decoder list that means "your GPU decodes nothing".
 *
 * The Rust side of this is one command over `hwaccel::detect()`,
 * `hwdecode::capabilities()` and the render context's adapter info; the shape
 * it must answer with is `HardwareReport` in `../types.ts`.
 */

import { invoke } from "@tauri-apps/api/core";

import { workspaceHardware } from "@/modules/workspace/lib/api";
import type { HardwareStatus, HwEncoder } from "@/modules/workspace/types";

/** Just the slice of `export_presets` this needs. Mirrors `export::job::ExportOptions`. */
interface ExportOptionsShape {
  hardware: HwEncoder[];
}

export async function hardwareStatus(): Promise<HardwareStatus> {
  try {
    const report = await workspaceHardware();
    return { ...report, partial: false };
  } catch {
    // Not registered, or it failed. Either way the encoder list is still
    // reachable through the command the export dialog uses, and half an answer
    // that is labelled as half is worth more than an error page.
    const options = await invoke<ExportOptionsShape>("export_presets");
    return {
      gpu: null,
      encoders: options.hardware ?? [],
      decoders: [],
      decode_default: null,
      partial: true,
    };
  }
}

/** Encoders this driver actually drove, newest API first is not meaningful — keep Rust's order. */
export function usableEncoders(status: HardwareStatus): HwEncoder[] {
  return status.encoders.filter((encoder) => encoder.usable);
}

/** Present, and refused. These are the interesting ones: each carries a reason. */
export function refusedEncoders(status: HardwareStatus): HwEncoder[] {
  return status.encoders.filter((encoder) => !encoder.usable);
}

/**
 * The one-line answer to "is it actually using my graphics card".
 *
 * Deliberately conservative: nothing that only *claims* support counts, because
 * the whole point of the probe is that claims are wrong on this class of
 * hardware.
 */
export function summarise(status: HardwareStatus): string {
  const encoders = usableEncoders(status).length;
  const decoders = status.decoders.filter((decoder) => decoder.usable).length;

  if (encoders === 0 && decoders === 0) {
    return status.partial
      ? "No hardware encoder on this machine could encode a test frame"
      : "Nothing on this machine could be driven — every stage runs on the CPU";
  }
  const parts: string[] = [];
  if (encoders > 0) parts.push(`${encoders} encoder${encoders === 1 ? "" : "s"}`);
  if (decoders > 0) parts.push(`${decoders} decoder${decoders === 1 ? "" : "s"}`);
  return `${parts.join(" and ")} verified by running them`;
}
