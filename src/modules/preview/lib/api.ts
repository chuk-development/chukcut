/**
 * The preview frame server.
 *
 * Frames do not cross IPC. Rust renders at proxy resolution, encodes JPEG and
 * serves the bytes over a registered URI scheme; these commands only ever move
 * the playhead and report where it is. See
 * `docs/architecture/preview-pipeline.md`.
 *
 * ## Casing
 *
 * This module does not follow the document convention of leaving Rust's
 * snake_case alone, because the preview structs do not: `PreviewInfo`,
 * `PreviewStatus`, `PreviewOptions` and `PreviewEvent` all carry
 * `#[serde(rename_all = "camelCase")]`. `EditCommand` and the project document
 * remain snake_case. Mirroring each type as it is actually serialized is the
 * only version of this that stays true.
 */

import { Channel, invoke } from "@tauri-apps/api/core";

import type { Micros } from "@/modules/project/types";

/** What every preview command hands back. */
export interface PreviewInfo {
  session: number;
  width: number;
  height: number;
  quality: number;
  fps: number;
  duration: Micros;
  position: Micros;
  frame: number;
  playing: boolean;
  /** Base URL for this session's frames; append `/<frame>`. */
  frameUrl: string;
}

/** `preview_state`, for a frontend that mounted after playback started. */
export interface PreviewStatus {
  /** Null when no session is open. */
  session: number | null;
  width: number;
  height: number;
  quality: number;
  fps: number;
  duration: Micros;
  position: Micros;
  frame: number;
  playing: boolean;
  /** Frames in the ring, and how many it holds. */
  cached: number;
  capacity: number;
  frameUrl: string | null;
}

export interface PreviewOptions {
  /** Raise or lower the proxy cap on the long edge. */
  longEdge?: number;
  quality?: number;
}

/**
 * Pushed from the render/pacer threads.
 *
 * Internally tagged: the variant lands in a `type` field, camelCased, with the
 * payload alongside it. Not `{ Ready: { … } }`.
 */
export type PreviewEvent =
  | {
      type: "ready";
      session: number;
      width: number;
      height: number;
      fps: number;
      duration: Micros;
    }
  | { type: "position"; session: number; frame: number; time: Micros; playing: boolean }
  | { type: "ended"; session: number }
  | { type: "error"; message: string };

export function previewStart(
  onEvent: Channel<PreviewEvent>,
  time?: Micros,
  options?: PreviewOptions,
): Promise<PreviewInfo> {
  return invoke<PreviewInfo>("preview_start", {
    time: time ?? null,
    options: options ?? null,
    onEvent,
  });
}

export function previewSeek(time: Micros): Promise<PreviewInfo> {
  return invoke<PreviewInfo>("preview_seek", { time });
}

export function previewPlay(): Promise<PreviewInfo> {
  return invoke<PreviewInfo>("preview_play");
}

export function previewPause(): Promise<PreviewInfo> {
  return invoke<PreviewInfo>("preview_pause");
}

export function previewStop(): Promise<void> {
  return invoke<void>("preview_stop");
}

export function previewState(): Promise<PreviewStatus> {
  return invoke<PreviewStatus>("preview_state");
}

export function newPreviewChannel(): Channel<PreviewEvent> {
  return new Channel<PreviewEvent>();
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/**
 * The three answers the frame protocol gives, kept apart because they mean
 * different things to the viewer.
 *
 * `gone` is a *superseded session* — a request from before a seek. It is not an
 * error and must never reach the user: a newer frame is already on its way.
 * `pending` is the right session but the frame is not encoded yet.
 */
export type FrameResult =
  | { status: "ok"; bitmap: ImageBitmap }
  | { status: "gone" }
  | { status: "pending" }
  | { status: "error"; message: string };

/**
 * Fetch one frame.
 *
 * `fetch` rather than an `<img>` src because an image element reports only
 * load-or-fail, and 410-vs-404 is exactly the distinction that decides whether
 * to drop the frame or try again.
 */
export async function fetchFrame(base: string, frame: number): Promise<FrameResult> {
  try {
    const response = await fetch(`${base}/${frame}`);
    if (response.status === 410) return { status: "gone" };
    if (response.status === 404) return { status: "pending" };
    if (!response.ok) {
      return { status: "error", message: `frame ${frame}: ${response.status}` };
    }
    return { status: "ok", bitmap: await createImageBitmap(await response.blob()) };
  } catch (error) {
    return { status: "error", message: error instanceof Error ? error.message : String(error) };
  }
}

/**
 * Proxy resolution for a canvas. Mirrors `preview::session::proxy_size`.
 *
 * Only used to size the placeholder before a session exists — once one does,
 * `PreviewInfo.width/height` is authoritative, because Rust also clamps to what
 * the device can allocate. It still has to agree with Rust: a placeholder at
 * one resolution followed by a first frame at another is a visible jump, and
 * the number in the player's header would be a lie until the session opened.
 *
 * Everything up to 1920 on the *long* edge previews natively, which is every
 * 1080p project in either orientation. The trap this band exists to avoid is
 * that a 1080x1920 vertical project measures 1920 here even though everyone
 * calls it "1080p"; an earlier table capped it at 720 and the preview looked
 * soft next to the source file.
 */
export function proxyResolution(canvas: { width: number; height: number }): {
  width: number;
  height: number;
} {
  const long = Math.max(canvas.width, canvas.height);
  if (long <= 1920) {
    return { width: Math.max(2, canvas.width), height: Math.max(2, canvas.height) };
  }
  const scale = 1920 / long;
  return {
    width: Math.max(2, Math.round(canvas.width * scale)),
    height: Math.max(2, Math.round(canvas.height * scale)),
  };
}
