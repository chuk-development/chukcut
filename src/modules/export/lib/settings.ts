/**
 * The dialog's settings arithmetic.
 *
 * Every rule in here mirrors one in `src-tauri/src/modules/export/presets.rs`,
 * and the mirror exists for one reason: Rust validates the combination and
 * answers in prose, but a dialog that lets the user assemble a combination it
 * knows will be rejected is a dialog that fails on the button press. So the form
 * resolves to something encodable *before* the request is built — pick WebM and
 * the codec follows, pick an NVENC encoder and the container follows.
 *
 * All of it is pure, because the interesting part is the coercion and not the
 * React around it.
 */

import type {
  AudioCodec,
  Container,
  ExportPreset,
  ExportRequest,
  Fps,
  HwEncoder,
  Quality,
  VideoCodec,
} from "@/modules/export/lib/api";
import type { Project } from "@/modules/project/types";

/** The escape-hatch preset, which takes its canvas and rate from the project. */
export const CUSTOM_PRESET_ID = "custom";

export function fpsToNumber(fps: Fps): number {
  return fps.den === 0 ? 0 : fps.num / fps.den;
}

/** How a frame rate is written when it is not a whole number. */
export function formatFps(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(3).replace(/0+$/, "");
}

/** The rates Rust snaps decimals onto, in the order the dialog lists them. */
export const FRAME_RATES = [23.976, 24, 25, 29.97, 30, 50, 59.94, 60];

export const CONTAINERS: { value: Container; label: string }[] = [
  { value: "mp4", label: "MP4" },
  { value: "mov", label: "MOV" },
  { value: "mkv", label: "MKV" },
  { value: "webm", label: "WebM" },
];

export const CODEC_LABELS: Record<VideoCodec, string> = {
  h264: "H.264",
  h265: "H.265 / HEVC",
  vp9: "VP9",
  av1: "AV1",
};

export const AUDIO_CODEC_LABELS: Record<AudioCodec, string> = {
  aac: "AAC",
  opus: "Opus",
  none: "none",
};

/**
 * The CRF-equivalent range a codec accepts. From `VideoCodec::crf_range`; the
 * slider is built from it so it cannot produce a value `validate` rejects.
 */
export function crfRange(codec: VideoCodec): [number, number] {
  return codec === "vp9" || codec === "av1" ? [0, 63] : [0, 51];
}

/** Mirrors `Container::accepts`. Muxers are stricter than people expect. */
export function containerAccepts(
  container: Container,
  video: VideoCodec,
  audio: AudioCodec,
): boolean {
  switch (container) {
    case "mp4":
    case "mov":
      return (
        (video === "h264" || video === "h265" || video === "av1") &&
        (audio === "aac" || audio === "none")
      );
    case "webm":
      return (video === "vp9" || video === "av1") && (audio === "opus" || audio === "none");
    // Matroska takes everything, which is why it is the escape hatch.
    case "mkv":
      return true;
  }
}

function videoCodecFor(container: Container, preferred: VideoCodec): VideoCodec {
  if (containerAccepts(container, preferred, "none")) return preferred;
  return container === "webm" ? "vp9" : "h264";
}

function audioCodecFor(container: Container, preferred: AudioCodec): AudioCodec {
  if (containerAccepts(container, "av1", preferred)) return preferred;
  return container === "webm" ? "opus" : "aac";
}

/** The container to fall back to when the chosen one cannot carry `video`. */
function containerFor(wanted: Container, video: VideoCodec): Container {
  if (containerAccepts(wanted, video, "none")) return wanted;
  if (containerAccepts("mp4", video, "none")) return "mp4";
  return "mkv";
}

/** What the user has typed into the dialog. */
export interface ExportForm {
  presetId: string;
  width: number;
  height: number;
  fps: number;
  quality: Quality;
  container: Container;
  /**
   * The preset's codec, kept even while a container that cannot carry it is
   * selected — switching from WebM back to MP4 has to restore the H.265 the
   * "YouTube 4K" preset asked for rather than leaving H.264 behind.
   */
  videoCodec: VideoCodec;
  audioCodec: AudioCodec;
  /** A `HwEncoder.id`, or null for software. */
  hardwareId: string | null;
  includeAudio: boolean;
  /** Null until the user has driven a save dialog. */
  outputPath: string | null;
}

/**
 * The settings that will actually be encoded, after every coercion.
 *
 * The dialog binds its controls to *this* rather than to the raw form, so what
 * is on screen is what Rust will be sent — a container the hardware encoder
 * cannot use never sits in the select looking chosen.
 */
export interface ResolvedSettings {
  width: number;
  height: number;
  fps: number;
  container: Container;
  videoCodec: VideoCodec;
  audioCodec: AudioCodec;
  quality: Quality;
  /** The hardware encoder, if one was picked and it exists. */
  hardware: HwEncoder | null;
}

/** Round down to even: 4:2:0 chroma has no half sample at the edge. */
function even(value: number): number {
  const whole = Math.max(2, Math.floor(value));
  return whole - (whole % 2);
}

/**
 * The starting form for a preset.
 *
 * "Custom" means "what I am looking at": the project's own canvas and rate,
 * not the template's placeholder numbers. An export that silently changes the
 * aspect ratio of the composition the user framed is the worst surprise there
 * is, so `custom_for` on the Rust side does the same thing.
 */
export function formFromPreset(
  preset: ExportPreset,
  project: Project | null,
  previous?: Partial<ExportForm>,
): ExportForm {
  const custom = preset.id === CUSTOM_PRESET_ID && project !== null;
  return {
    presetId: preset.id,
    width: custom && project ? even(project.canvas.width) : preset.width,
    height: custom && project ? even(project.canvas.height) : preset.height,
    fps: custom && project ? project.fps : fpsToNumber(preset.fps),
    quality: preset.quality,
    container: preset.container,
    videoCodec: preset.video_codec,
    audioCodec: preset.audio_codec,
    // Changing preset is a change of destination, not of machine: the encoder
    // and the audio decision the user already made carry over.
    hardwareId: previous?.hardwareId ?? null,
    includeAudio: previous?.includeAudio ?? true,
    outputPath: previous?.outputPath ?? null,
  };
}

/**
 * Fold the hardware choice and the container rules into the form.
 *
 * Order matters. Choosing "H.265 (NVENC)" is choosing a codec as much as an
 * encoder — Rust makes the hardware win over the preset — so the codec is
 * decided first and the container bends around it, not the other way about.
 */
export function resolveSettings(form: ExportForm, hardware: HwEncoder[]): ResolvedSettings {
  const chosen = form.hardwareId
    ? (hardware.find((encoder) => encoder.id === form.hardwareId && encoder.usable) ?? null)
    : null;

  const container = chosen ? containerFor(form.container, chosen.codec) : form.container;
  const videoCodec = chosen ? chosen.codec : videoCodecFor(container, form.videoCodec);
  const audioCodec = audioCodecFor(container, form.audioCodec);

  const [low, high] = crfRange(videoCodec);
  const quality: Quality =
    form.quality.kind === "crf"
      ? { kind: "crf", value: Math.min(high, Math.max(low, Math.round(form.quality.value))) }
      : { kind: "bitrate", value: Math.round(form.quality.value) };

  return {
    width: even(form.width),
    height: even(form.height),
    fps: form.fps,
    container,
    videoCodec,
    audioCodec,
    quality,
    hardware: chosen,
  };
}

/**
 * The request, exactly as Rust deserializes it.
 *
 * Every override the dialog can express is sent explicitly rather than omitted
 * when it happens to match the preset. A request that means the same thing on
 * every path is one that can be read in a log and reproduced.
 */
export function buildRequest(form: ExportForm, resolved: ResolvedSettings): ExportRequest {
  if (!form.outputPath) {
    throw new Error("an export request needs a destination the user chose");
  }
  return {
    output_path: form.outputPath,
    preset_id: form.presetId,
    overrides: {
      width: resolved.width,
      height: resolved.height,
      fps: resolved.fps,
      video_codec: resolved.videoCodec,
      quality: resolved.quality,
      audio_codec: resolved.audioCodec,
      container: resolved.container,
    },
    hardware: resolved.hardware ? resolved.hardware.id : null,
    include_audio: form.includeAudio,
  };
}

/**
 * Force a file extension to match the container. Mirrors `with_extension` in
 * `job.rs`, which does this to the path it is given whatever the dialog sent —
 * so showing the uncorrected name would be showing the user a file that will
 * not exist.
 */
export function withExtension(path: string, extension: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const name = path.slice(slash + 1);
  const dot = name.lastIndexOf(".");
  // A leading dot is a hidden file, not an extension.
  const current = dot > 0 ? name.slice(dot + 1) : null;
  if (current !== null && current.toLowerCase() === extension.toLowerCase()) return path;
  const stem = dot > 0 ? name.slice(0, dot) : name;
  return `${path.slice(0, slash + 1)}${stem}.${extension}`;
}

/** The file name to offer in the save dialog, with the container's extension. */
export function defaultFileName(projectName: string | undefined, container: Container): string {
  const base = (projectName ?? "Untitled").trim() || "Untitled";
  return `${base.replace(/\.[a-z0-9]{2,4}$/i, "")}.${container}`;
}

/**
 * Why the Export button is disabled, or null when it is not.
 *
 * A disabled button with no reason next to it is a dead end; every branch here
 * is something the user can act on.
 */
export function exportBlockedReason(
  project: Project | null,
  durationMicros: number,
  outputPath: string | null,
): string | null {
  if (!project) return "Open a project first.";
  if (durationMicros <= 0) return "The timeline is empty, so there is nothing to export.";
  if (!outputPath) return "Choose where the file should go.";
  return null;
}
