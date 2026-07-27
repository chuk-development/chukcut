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

// ---------------------------------------------------------------------------
// Resolution choices
// ---------------------------------------------------------------------------

/** One entry of the resolution select. */
export interface ResolutionOption {
  /** `"source"` for the canvas itself, or the long edge as a string. */
  id: string;
  label: string;
  width: number;
  height: number;
}

/** The long-edge sizes the dialog offers besides the canvas itself. */
export const LONG_EDGES = [2160, 1440, 1080, 720];

/**
 * The canvas plus the standard long-edge sizes, each keeping the canvas
 * aspect. "Long edge" rather than "height" because this editor is mostly
 * vertical video: a 1080×1920 project scaled to "1080" must stay 1080×1920,
 * not become a 608-wide sliver.
 */
export function resolutionOptions(base: { width: number; height: number }): ResolutionOption[] {
  const canvas: ResolutionOption = {
    id: "source",
    label: `Canvas · ${even(base.width)}×${even(base.height)}`,
    width: even(base.width),
    height: even(base.height),
  };
  const long = Math.max(base.width, base.height, 1);
  const scaled = LONG_EDGES.map((edge) => {
    const scale = edge / long;
    return {
      id: String(edge),
      label: `${edge} · ${even(base.width * scale)}×${even(base.height * scale)}`,
      width: even(base.width * scale),
      height: even(base.height * scale),
    };
  }).filter((option) => option.width !== canvas.width || option.height !== canvas.height);
  return [canvas, ...scaled];
}

/** The option matching a width and height, or null when the size is hand-typed. */
export function matchingResolution(
  options: ResolutionOption[],
  width: number,
  height: number,
): ResolutionOption | null {
  return options.find((option) => option.width === width && option.height === height) ?? null;
}

// ---------------------------------------------------------------------------
// Estimates
// ---------------------------------------------------------------------------

/**
 * Bits per pixel of x264 at CRF 20, 1080p30 — which the YouTube preset's own
 * comment calibrates at 8–12 Mbit/s. Every other codec and CRF is derived
 * from this one anchor, which is also why everything below is *labelled* an
 * estimate: the real rate depends on the footage, and grain against a locked
 * shot differ by 5x at the same CRF.
 */
const H264_BPP_AT_CRF20 = 0.16;

/** Roughly what a codec needs relative to H.264 for the same look. */
const CODEC_EFFICIENCY: Record<VideoCodec, number> = {
  h264: 1.0,
  h265: 0.6,
  vp9: 0.65,
  av1: 0.5,
};

/** A codec's CRF on H.264's 0..51 scale, so one set of bands fits all four. */
function crfOnH264Scale(codec: VideoCodec, crf: number): number {
  const [, high] = crfRange(codec);
  return (crf * 51) / Math.max(1, high);
}

/**
 * The bitrate an export will land near, in bits per second.
 *
 * Bitrate mode is exact by definition. CRF mode uses the anchor above and the
 * empirical "half the rate every six CRF points" rule, clamped so nonsense
 * input cannot show a negative or absurd number.
 */
export function estimateVideoBitrate(
  codec: VideoCodec,
  quality: Quality,
  width: number,
  height: number,
  fps: number,
): number {
  if (quality.kind === "bitrate") return Math.max(0, quality.value);
  const equivalent = crfOnH264Scale(codec, quality.value);
  const bitsPerPixel = H264_BPP_AT_CRF20 * CODEC_EFFICIENCY[codec] * 2 ** ((20 - equivalent) / 6);
  const rate = Math.max(2, width) * Math.max(2, height) * Math.max(1, fps) * bitsPerPixel;
  return Math.round(Math.min(400_000_000, Math.max(100_000, rate)));
}

/** Estimated file size in bytes: (video + audio) × duration. */
export function estimateFileSize(
  videoBitsPerSecond: number,
  audioBitsPerSecond: number,
  durationMicros: number,
): number {
  if (durationMicros <= 0) return 0;
  const seconds = durationMicros / 1_000_000;
  return Math.round(((videoBitsPerSecond + audioBitsPerSecond) * seconds) / 8);
}

/**
 * What a CRF value means, in words a person who has never heard of CRF can
 * act on. The bands sit on the H.264-equivalent scale so "High quality" is
 * the same look whichever codec's slider produced it.
 */
export function qualityCaption(codec: VideoCodec, crf: number): string {
  const equivalent = crfOnH264Scale(codec, crf);
  if (equivalent <= 12) return "Near lossless — huge file";
  if (equivalent <= 18) return "Very high quality";
  if (equivalent <= 23) return "High quality";
  if (equivalent <= 28) return "Good quality — smaller file";
  if (equivalent <= 35) return "Compressed — artifacts likely";
  return "Heavily compressed";
}

/** A bit rate at the precision anyone reads it at. Mirrors `bitrate` in job.rs. */
export function formatBitrate(bitsPerSecond: number): string {
  if (bitsPerSecond >= 1_000_000) return `${(bitsPerSecond / 1_000_000).toFixed(1)} Mb/s`;
  return `${Math.round(bitsPerSecond / 1_000)} kb/s`;
}

/** Decimal units, because that is what file managers show next to the file. */
export function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(2)} GB`;
  if (bytes >= 1_000_000) return `${Math.round(bytes / 1_000_000)} MB`;
  return `${Math.max(0, Math.round(bytes / 1_000))} kB`;
}

// ---------------------------------------------------------------------------
// The export range
// ---------------------------------------------------------------------------

/** A clamped, ordered range of the timeline, in microseconds. */
export interface ExportRange {
  start: number;
  end: number;
}

/**
 * Read another store's `exportRange` field without trusting it.
 *
 * The in/out marks live in the timeline store and are being added by other
 * work — this dialog has to behave identically whether the field is absent,
 * null, or carries values an edit has since invalidated. Anything that does
 * not clamp to a non-empty slice of the timeline is simply "no range".
 */
export function normalizeExportRange(raw: unknown, durationMicros: number): ExportRange | null {
  if (durationMicros <= 0 || typeof raw !== "object" || raw === null) return null;
  const { start, end } = raw as { start?: unknown; end?: unknown };
  if (typeof start !== "number" || typeof end !== "number") return null;
  if (!Number.isFinite(start) || !Number.isFinite(end)) return null;
  const low = Math.max(0, Math.min(start, end));
  const high = Math.min(durationMicros, Math.max(start, end));
  if (high <= low) return null;
  return { start: Math.round(low), end: Math.round(high) };
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/** The directory part of a path, without the trailing separator. */
export function dirOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return slash > 0 ? path.slice(0, slash) : "";
}

/** The file name without its directory or extension. */
export function stemOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const name = path.slice(slash + 1);
  const dot = name.lastIndexOf(".");
  // A leading dot is a hidden file, not an extension.
  return dot > 0 ? name.slice(0, dot) : name;
}

/** Join with whichever separator the directory already uses. */
export function joinPath(dir: string, name: string): string {
  const separator = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  const trimmed = dir.endsWith("/") || dir.endsWith("\\") ? dir.slice(0, -1) : dir;
  return `${trimmed}${separator}${name}`;
}

/**
 * A file name a file system will take: no separators, no leading dot that
 * would hide the file. Empty comes back as "Untitled" rather than producing
 * a path that ends in a bare extension.
 */
export function sanitizeFileName(name: string): string {
  const cleaned = name.replace(/[/\\]/g, " ").replace(/^\.+/, "").trim();
  return cleaned || "Untitled";
}

// ---------------------------------------------------------------------------
// Remembered settings
// ---------------------------------------------------------------------------

/**
 * What "remember these settings" writes into the workspace settings file.
 *
 * Choices, not measurements: the resolution is remembered as the long-edge
 * *choice* rather than as absolute pixels, because 3840×2160 remembered from
 * a landscape project would distort the next vertical one. Paths are not
 * remembered at all — a destination belongs to a project, not to the app.
 */
export interface RememberedExportSettings {
  preset_id: string;
  fps: number;
  quality: Quality;
  container: Container;
  video_codec: VideoCodec;
  audio_codec: AudioCodec;
  hardware_id: string | null;
  include_audio: boolean;
  /** A `LONG_EDGES` entry, or null for "the project canvas". */
  long_edge: number | null;
}

/** The remembered shape of a form. `base` is the canvas the sizes derive from. */
export function rememberForm(
  form: ExportForm,
  base: { width: number; height: number },
): RememberedExportSettings {
  const match = matchingResolution(resolutionOptions(base), form.width, form.height);
  return {
    preset_id: form.presetId,
    fps: form.fps,
    quality: form.quality,
    container: form.container,
    video_codec: form.videoCodec,
    audio_codec: form.audioCodec,
    hardware_id: form.hardwareId,
    include_audio: form.includeAudio,
    long_edge: match && match.id !== "source" ? Number(match.id) : null,
  };
}

/** The preset a remembered blob names, when it names one at all. */
export function rememberedPresetId(raw: unknown): string | null {
  if (typeof raw !== "object" || raw === null) return null;
  const id = (raw as { preset_id?: unknown }).preset_id;
  return typeof id === "string" && id.length > 0 ? id : null;
}

const VIDEO_CODECS: VideoCodec[] = ["h264", "h265", "vp9", "av1"];
const AUDIO_CODECS: AudioCodec[] = ["aac", "opus", "none"];

/**
 * Lay a remembered blob over a freshly seeded form, field by defended field.
 *
 * The blob comes from a settings file any historical build may have written,
 * so every field is type-checked and anything unrecognisable keeps the seed's
 * value — a corrupt memory degrades to "the dialog forgot", never to a form
 * that assembles an invalid request.
 */
export function applyRemembered(
  form: ExportForm,
  raw: unknown,
  base: { width: number; height: number },
): ExportForm {
  if (typeof raw !== "object" || raw === null) return form;
  const r = raw as Partial<Record<keyof RememberedExportSettings, unknown>>;
  const next = { ...form };

  if (typeof r.fps === "number" && Number.isFinite(r.fps) && r.fps > 0) next.fps = r.fps;
  if (
    typeof r.quality === "object" &&
    r.quality !== null &&
    ((r.quality as Quality).kind === "crf" || (r.quality as Quality).kind === "bitrate") &&
    Number.isFinite((r.quality as Quality).value)
  ) {
    next.quality = r.quality as Quality;
  }
  if (CONTAINERS.some((c) => c.value === r.container)) next.container = r.container as Container;
  if (VIDEO_CODECS.includes(r.video_codec as VideoCodec))
    next.videoCodec = r.video_codec as VideoCodec;
  if (AUDIO_CODECS.includes(r.audio_codec as AudioCodec))
    next.audioCodec = r.audio_codec as AudioCodec;
  if (typeof r.hardware_id === "string" || r.hardware_id === null)
    next.hardwareId = (r.hardware_id as string | null) ?? null;
  if (typeof r.include_audio === "boolean") next.includeAudio = r.include_audio;
  if (typeof r.long_edge === "number" && LONG_EDGES.includes(r.long_edge)) {
    const option = resolutionOptions(base).find((o) => o.id === String(r.long_edge));
    if (option) {
      next.width = option.width;
      next.height = option.height;
    }
  }
  return next;
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
export function buildRequest(
  form: ExportForm,
  resolved: ResolvedSettings,
  range: ExportRange | null = null,
): ExportRequest {
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
    range: range ? [range.start, range.end] : null,
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

/**
 * The file name to offer in the save dialog.
 *
 * Only `.chukcut` is stripped, not any trailing dotted word: a project called
 * "Holiday.v2" is named that on purpose, and turning it into "Holiday.mp4"
 * would quietly drop the part the user was using to tell two cuts apart.
 */
export function defaultFileName(projectName: string | undefined, container: Container): string {
  const base = (projectName ?? "Untitled").trim() || "Untitled";
  return `${base.replace(/\.chukcut$/i, "")}.${container}`;
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
