/**
 * Small formatters for the chrome.
 *
 * Kept here rather than in `src/lib/` because none of them is about time on the
 * timeline, which is what `lib/time.ts` owns, and mixing "when was this project
 * last opened" into the module that converts microseconds to frames is how that
 * file stops being readable.
 */

const KIB = 1024;
const UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/**
 * Bytes, at the precision a person can act on.
 *
 * One decimal below 10 and none above, so a cache reads as "1.4 GB" and
 * "612 MB" rather than "1.42 GB" and "612.4 MB" — the extra digit is noise on a
 * number whose only use is deciding whether to press Clear.
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  let value = bytes;
  let unit = 0;
  while (value >= KIB && unit < UNITS.length - 1) {
    value /= KIB;
    unit += 1;
  }
  const decimals = unit === 0 || value >= 10 ? 0 : 1;
  return `${value.toFixed(decimals)} ${UNITS[unit]}`;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * When something happened, relative to now.
 *
 * Relative up to a week, absolute after that: "6 days ago" is a useful answer
 * and "83 days ago" is not — past a certain distance people want the date.
 */
export function formatWhen(timestamp: number, now: number = Date.now()): string {
  const elapsed = now - timestamp;
  if (!Number.isFinite(timestamp) || timestamp <= 0) return "never";
  if (elapsed < 0) return "just now";
  if (elapsed < MINUTE) return "just now";
  if (elapsed < HOUR) {
    const minutes = Math.floor(elapsed / MINUTE);
    return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  }
  if (elapsed < DAY) {
    const hours = Math.floor(elapsed / HOUR);
    return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  }
  if (elapsed < 7 * DAY) {
    const days = Math.floor(elapsed / DAY);
    return days === 1 ? "yesterday" : `${days} days ago`;
  }
  return new Date(timestamp).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/** The directory a file sits in, for the dimmed second line of a recent entry. */
export function parentDirectory(path: string): string {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut > 0 ? path.slice(0, cut) : path;
}

/** A canvas, as it is written everywhere in this app. */
export function formatCanvas(width: number, height: number): string {
  return `${width} × ${height}`;
}
