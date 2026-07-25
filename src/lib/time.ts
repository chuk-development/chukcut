/**
 * Time helpers.
 *
 * The document stores microseconds and nothing else (see
 * `docs/architecture/project-format.md`). Frames exist only at the edges — the
 * ruler, the transport readout, the step buttons — so every conversion to a
 * frame number happens here rather than being scattered through components.
 */

export const MICROS_PER_SECOND = 1_000_000;

/** Length of one frame at `fps`, rounded to whole microseconds. */
export function frameDuration(fps: number): number {
  return Math.max(1, Math.round(MICROS_PER_SECOND / fps));
}

export function microsToFrame(micros: number, fps: number): number {
  return Math.round((micros * fps) / MICROS_PER_SECOND);
}

export function frameToMicros(frame: number, fps: number): number {
  return Math.round((frame * MICROS_PER_SECOND) / fps);
}

function pad(value: number, width = 2): string {
  return String(Math.floor(Math.abs(value))).padStart(width, "0");
}

/**
 * `HH:MM:SS:FF` — the transport readout. Hours are dropped below one hour
 * because a vertical-video editor spends its life under three minutes and the
 * leading `00:` is noise.
 */
export function formatTimecode(micros: number, fps: number): string {
  const clamped = Math.max(0, micros);
  const totalSeconds = Math.floor(clamped / MICROS_PER_SECOND);
  // Floor, not round: rounding lets the last fraction of a second display as
  // frame `fps`, which is a frame that does not exist.
  const frames = Math.floor(((clamped % MICROS_PER_SECOND) * fps) / MICROS_PER_SECOND);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const head = hours > 0 ? `${pad(hours)}:${pad(minutes)}` : pad(minutes);
  return `${head}:${pad(seconds)}:${pad(frames)}`;
}

/** `M:SS` / `H:MM:SS` — for media tiles, where frame accuracy is not the point. */
export function formatDuration(micros: number): string {
  const totalSeconds = Math.round(Math.max(0, micros) / MICROS_PER_SECOND);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${minutes}:${pad(seconds)}`;
}

/** Ruler label: seconds and up, never frames. */
export function formatRulerLabel(micros: number): string {
  const totalSeconds = Math.floor(Math.max(0, micros) / MICROS_PER_SECOND);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${pad(minutes)}:${pad(seconds)}`;
}

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
