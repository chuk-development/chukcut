/**
 * The preview session's lifecycle.
 *
 * Module-level rather than a hook: there is exactly one frame server, the event
 * channel has to outlive any component that happens to be mounted, and a
 * restart triggered by an edit must not be cancelled because the player panel
 * re-rendered.
 *
 * ## Who moves the playhead
 *
 * Strictly one direction each way, or the two ends fight:
 *
 * - Rust → frontend: `position` events write the timeline playhead. Nothing
 *   here calls `seek` in response.
 * - Frontend → Rust: user gestures (scrubbing, stepping, jumping) call
 *   `seek()` explicitly. The store is never watched for that, because a
 *   playhead move caused by playback is indistinguishable from one caused by a
 *   drag once it has landed in the store.
 */

import {
  newPreviewChannel,
  type PreviewEvent,
  previewPause,
  previewPlay,
  previewSeek,
  previewStart,
} from "@/modules/preview/lib/api";
import { usePreviewStore } from "@/modules/preview/store";
import { describeError, useProjectStore } from "@/modules/project/store";
import type { Micros } from "@/modules/project/types";
import { useTimelineStore } from "@/modules/timeline/store";

/**
 * How long to wait after an edit before re-snapshotting.
 *
 * A drag that ends in a trim plus a reindex, or a paste of several clips,
 * arrives as a burst of documents. Opening a session per document would spend
 * the GPU on frames nobody sees.
 */
const RESTART_DEBOUNCE_MS = 160;

let starting: Promise<void> | null = null;
let restartTimer: ReturnType<typeof setTimeout> | null = null;

function handleEvent(event: PreviewEvent): void {
  const store = usePreviewStore.getState();

  switch (event.type) {
    case "ready":
      store.setError(null);
      break;

    case "position": {
      // Ignore anything from a session we have already replaced; a superseded
      // frame number means nothing against the current ring.
      if (store.session !== null && event.session !== store.session) return;
      store.setFrame(event.frame, event.playing);
      // Rust's clock is the authority during playback, so the ruler follows it.
      useTimelineStore.getState().setPlayhead(event.time);
      break;
    }

    case "ended":
      store.setPlaying(false);
      break;

    case "error":
      store.setError(event.message);
      break;

    default:
      break;
  }
}

/** Open a session at `time`. Safe to call repeatedly; overlapping calls are folded. */
async function open(time: Micros): Promise<void> {
  if (!useProjectStore.getState().project) return;
  try {
    const info = await previewStart(
      (() => {
        const channel = newPreviewChannel();
        channel.onmessage = handleEvent;
        return channel;
      })(),
      time,
    );
    usePreviewStore.getState().applyInfo(info);
  } catch (error) {
    usePreviewStore.getState().setError(describeError(error));
  }
}

export const preview = {
  /** Open the first session, or do nothing if one is already open. */
  async ensureStarted(): Promise<void> {
    if (usePreviewStore.getState().session !== null || starting) return;
    starting = open(useTimelineStore.getState().playhead);
    try {
      await starting;
    } finally {
      starting = null;
    }
  },

  /**
   * Re-snapshot the document after an edit.
   *
   * `preview_start` is how a change is picked up at all — the render thread
   * works from a snapshot taken when the session opened, so an edited document
   * is invisible until a new session exists. Cheap: the threads and the GPU
   * device survive, and it costs one frame.
   */
  restart(): void {
    if (restartTimer) clearTimeout(restartTimer);
    restartTimer = setTimeout(() => {
      restartTimer = null;
      // `preview_start` always adopts paused; resume if the user was watching.
      const wasPlaying = usePreviewStore.getState().playing;
      void open(useTimelineStore.getState().playhead).then(() => {
        if (wasPlaying) void preview.play();
      });
    }, RESTART_DEBOUNCE_MS);
  },

  /**
   * Move the playhead.
   *
   * Deliberately not queued: each seek supersedes the last, so a scrub that
   * produces forty positions should produce forty seeks and one visible frame,
   * not forty frames rendered in order.
   */
  async seek(time: Micros): Promise<void> {
    if (usePreviewStore.getState().session === null) {
      await preview.ensureStarted();
      return;
    }
    try {
      usePreviewStore.getState().applyInfo(await previewSeek(Math.max(0, Math.round(time))));
    } catch {
      // A seek against a closed session is not worth a message; the next edit
      // or mount opens a new one.
    }
  },

  async play(): Promise<void> {
    try {
      usePreviewStore.getState().applyInfo(await previewPlay());
    } catch (error) {
      usePreviewStore.getState().setError(describeError(error));
    }
  },

  async pause(): Promise<void> {
    try {
      usePreviewStore.getState().applyInfo(await previewPause());
    } catch {
      // Pausing something that is not playing is not a failure.
    }
  },

  async toggle(): Promise<void> {
    if (usePreviewStore.getState().playing) await preview.pause();
    else await preview.play();
  },
};

/**
 * Restart the preview whenever the document changes.
 *
 * The document is replaced wholesale by every edit, so identity is the signal —
 * no diffing, and no chance of missing a change that a shallow compare would
 * have skipped.
 */
export function watchDocumentForPreview(): () => void {
  return useProjectStore.subscribe((state, previous) => {
    if (state.project === previous.project) return;
    if (!state.project) return;
    if (usePreviewStore.getState().session === null) void preview.ensureStarted();
    else preview.restart();
  });
}
