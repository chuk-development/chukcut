/**
 * Everything that is true of the installation rather than of any one project.
 *
 * These types mirror `src-tauri/src/modules/workspace/settings.rs` field for
 * field, and — like the project document — they keep Rust's snake_case rather
 * than being prettified on the way in. A translation layer would be one more
 * place for the two halves to drift apart.
 */

/** Persisted preferences. Mirrors `workspace::Settings`. */
export interface Settings {
  /** Long-edge cap for preview rendering, in pixels. */
  preview_max_edge: number;
  /** Render the preview at the full canvas resolution, whatever it costs. */
  preview_full_quality: boolean;
  /** JPEG quality for preview frames, 1..100. */
  preview_quality: number;
  /** Snap clips to edges, the playhead and second boundaries while dragging. */
  snapping: boolean;
  /** Canvas for new projects, `[width, height]` — a Rust tuple, so an array here. */
  default_canvas: [number, number];
  default_fps: number;
  /** Cap on cache size in bytes. `0` means no cap. */
  cache_limit: number;
}

/**
 * Where this run is writing its log. Mirrors `commands::LogLocation`.
 *
 * `file` is null when the log file could not be opened — a read-only home
 * directory, most likely. The app still runs and still logs to stdout, so the
 * panel says the directory and offers nothing to reveal, rather than a button
 * that opens a folder with nothing in it.
 */
export interface LogLocation {
  directory: string;
  file: string | null;
}

/**
 * What the menu bar is told about the document. Mirrors
 * `workspace::menu::MenuState`.
 *
 * Facts, not decisions: which of these greys out which item is decided in one
 * pure function on the Rust side, so there is a single place to read when an
 * item is unavailable and no chance of the two halves disagreeing about it.
 */
export interface MenuState {
  /** A document is open at all. */
  has_project: boolean;
  /** It has edits that are not on disk. */
  dirty: boolean;
  can_undo: boolean;
  can_redo: boolean;
  /** At least one clip is selected, so there is something to delete or copy. */
  has_selection: boolean;
  /** The playhead is inside a clip, so there is something to cut. */
  can_split: boolean;
  /** There is a timeline with a span worth fitting the viewport to. */
  can_fit: boolean;
  /**
   * Something has been cut or copied in this session.
   *
   * The clipboard itself never crosses the boundary — it is a list of detached
   * segments in the timeline store, see `timeline/lib/clipboard.ts` — and this
   * one bit is all the bar needs to decide whether Paste is worth offering.
   */
  has_clipboard: boolean;
  /** There is at least one clip anywhere, so Select All would select something. */
  has_clips: boolean;
}

/**
 * The menu bar as Rust says to draw it. Mirrors `workspace::menu::SectionView`
 * and friends.
 *
 * The whole bar crosses, not only the enabled flags, and that is the point:
 * `menu.rs` holds one table of every item and its gate, and `MenuBar.tsx`
 * renders it. Adding an item on this side is therefore not possible, which is
 * what stops the bar from growing a row that looks live and does nothing.
 */
export interface MenuItemView {
  id: string;
  label: string;
  /** Written the way it is written on a keyboard: `Ctrl+S`, `Del`, `C`. */
  accelerator: string | null;
  enabled: boolean;
  /**
   * A dimmed second line under the label. Only the recent-projects rows carry
   * one — the path, because two projects called "Untitled" are otherwise the
   * same row.
   */
  detail: string | null;
  /**
   * Why this item can never be enabled, when the feature does not exist at all.
   * `null` for anything that is merely unavailable in the current document.
   */
  unavailable_reason: string | null;
}

/** A nested menu, resolved. One level only — its entries never nest again. */
export interface MenuSubmenuView {
  id: string;
  label: string;
  enabled: boolean;
  entries: MenuEntryView[];
}

export type MenuEntryView =
  | ({ kind: "item" } & MenuItemView)
  | { kind: "separator" }
  | ({ kind: "submenu" } & MenuSubmenuView);

export interface MenuSectionView {
  title: string;
  entries: MenuEntryView[];
}

/**
 * What Rust hands back before the user has ever opened the settings panel.
 *
 * Duplicated from `Settings::default()` on purpose: the panel has to render
 * something the instant it mounts, and a store that starts as `null` makes
 * every control in it nullable for the two frames before the answer lands.
 * If the two ever disagree the round-trip test notices, because the first
 * `workspace_settings_set` would carry a value the user did not choose.
 */
export const DEFAULT_SETTINGS: Settings = {
  preview_max_edge: 0,
  preview_full_quality: false,
  preview_quality: 80,
  snapping: true,
  default_canvas: [1080, 1920],
  default_fps: 30,
  cache_limit: 8 * 1024 * 1024 * 1024,
};

/** One entry of the recent-projects list. Mirrors `workspace::RecentProject`. */
export interface RecentProject {
  path: string;
  name: string;
  /** Unix millis of the last time it was opened. */
  opened_at: number;
}

// ---------------------------------------------------------------------------
// Canvas presets
// ---------------------------------------------------------------------------

export interface CanvasPreset {
  id: string;
  label: string;
  aspect: string;
  width: number;
  height: number;
  /** Where a video of this shape ends up. */
  hint: string;
}

/**
 * The three shapes people actually deliver, at 1080p.
 *
 * Deliberately short. A start screen offering nine resolutions makes the first
 * decision of a session a research task; anything else is one click further in,
 * in the full new-project dialog.
 */
export const CANVAS_PRESETS: CanvasPreset[] = [
  {
    id: "1080x1920",
    label: "Vertical",
    aspect: "9:16",
    width: 1080,
    height: 1920,
    hint: "Reels, Shorts, TikTok",
  },
  {
    id: "1920x1080",
    label: "Landscape",
    aspect: "16:9",
    width: 1920,
    height: 1080,
    hint: "YouTube, everything else",
  },
  {
    id: "1080x1080",
    label: "Square",
    aspect: "1:1",
    width: 1080,
    height: 1080,
    hint: "Feed posts",
  },
];

/** The preset matching a canvas, when there is one. */
export function presetFor(width: number, height: number): CanvasPreset | null {
  return CANVAS_PRESETS.find((p) => p.width === width && p.height === height) ?? null;
}

// ---------------------------------------------------------------------------
// Hardware
//
// The engine establishes all of this by *trying* — encoding a test frame,
// decoding an embedded one — rather than by reading a capability list, because
// an encoder being in the FFmpeg build says nothing about whether the driver
// can drive it. The types keep that distinction visible: `available` and
// `in_build` are claims, `usable` is the only field worth believing.
// ---------------------------------------------------------------------------

export type HwAccelId =
  | "software"
  | "vaapi"
  | "qsv"
  | "nvenc"
  | "video_toolbox"
  | "amf"
  | "media_foundation";

export type VideoCodecId = "h264" | "h265" | "vp9" | "av1";
export type HwDecodeCodecId = "h264" | "hevc" | "vp9" | "av1";

/** Mirrors `export::hwaccel::HwEncoder`. */
export interface HwEncoder {
  id: string;
  accel: HwAccelId;
  codec: VideoCodecId;
  /** What FFmpeg calls it — `h264_vaapi`. */
  encoder_name: string;
  label: string;
  /** In this build *and* backed by a device we can see. Still only a claim. */
  available: boolean;
  /** A test frame went in and a packet came out. */
  usable: boolean;
  /** Why not, in prose, when `usable` is false. */
  note: string | null;
}

/** Mirrors `media::hwdecode::HwDecodeSupport`. */
export interface HwDecoder {
  codec: HwDecodeCodecId;
  /** `h264`, not `h264_vaapi` — VAAPI decode is the ordinary decoder with an accelerator under it. */
  decoder_name: string;
  in_build: boolean;
  /** The decoder *claims* it can produce a VA surface. A claim, from a static table. */
  declares_vaapi: boolean;
  /** A real frame went in and a VA surface came out. */
  usable: boolean;
  note: string | null;
}

/** The adapter the compositor actually opened. */
export interface GpuInfo {
  name: string;
  /** `Vulkan`, `Metal`, `DX12`, `GL`. */
  backend: string;
  driver: string;
  /** `integrated-gpu`, `discrete-gpu`, `cpu`, `other`. */
  device_type: string;
}

/** Mirrors the proposed `workspace_hardware` answer. */
export interface HardwareReport {
  gpu: GpuInfo | null;
  encoders: HwEncoder[];
  decoders: HwDecoder[];
  /** What `media::provider::DEFAULT_ACCELERATION` is set to. */
  decode_default: string | null;
}

/**
 * A report plus how much of it we actually got.
 *
 * `partial` is true when the engine could only tell us about encoders — see
 * `lib/hardware.ts`. The panel says so out loud rather than drawing an empty
 * decoder section that reads as "your GPU cannot decode anything".
 */
export interface HardwareStatus extends HardwareReport {
  partial: boolean;
}

export const EMPTY_HARDWARE: HardwareStatus = {
  gpu: null,
  encoders: [],
  decoders: [],
  decode_default: null,
  partial: false,
};
