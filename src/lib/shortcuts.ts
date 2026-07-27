/**
 * The J/K/L transport keys, bound app-wide.
 *
 * The one-owner-per-key rule (docs/STATUS.md, "A GTK menu accelerator shadows
 * typing"): every key in this app has exactly one `keydown` handler, next to
 * the thing it acts on. The preview panel owns Space, ←/→, Home/End and F;
 * the timeline owns the letters it uses. J, K and L had no owner, and they
 * act on the playback session rather than on either panel, so they live here
 * and are installed once by `App.tsx`.
 *
 * **J is honest, not traditional.** In every NLE J means reverse play, which
 * this engine does not do — the decoder walks forward and the audio clock is
 * the master. Pretending otherwise (say, J seeking backwards on a timer)
 * would stutter and lie about what the engine can do. So J pauses and steps
 * back one frame: the useful part of "go backwards", done exactly. K pauses,
 * L plays; both are exactly the transport buttons.
 *
 * The catalogue in `modules/workspace/lib/shortcuts.ts` lists these three; if
 * either file changes, change both in the same commit.
 */

import { frameDuration } from "@/lib/time";
import { preview } from "@/modules/preview/lib/session";
import { useProjectStore } from "@/modules/project/store";
import { useTimelineStore } from "@/modules/timeline/store";

/** Pause, then land one frame earlier. The engine cannot play in reverse. */
async function pauseAndStepBack(fps: number): Promise<void> {
  await preview.pause();
  const at = useTimelineStore.getState().playhead;
  const target = Math.max(0, at - frameDuration(fps));
  useTimelineStore.getState().setPlayhead(target);
  await preview.seek(target);
}

/**
 * Bind J/K/L. Returns the teardown.
 *
 * Bare letters, so the guard order matters: a focused text field always wins,
 * and any modifier means the key is somebody else's chord (Ctrl+L is the
 * browser's, and must stay so).
 */
export function installTransportKeys(): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;

    const target = event.target as HTMLElement | null;
    if (target && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName))) {
      return;
    }

    // No document, no transport. The keys must not open a session on the
    // start screen.
    const project = useProjectStore.getState().project;
    if (!project) return;

    switch (event.key) {
      case "j":
      case "J":
        event.preventDefault();
        void pauseAndStepBack(project.fps);
        break;
      case "k":
      case "K":
        event.preventDefault();
        void preview.pause();
        break;
      case "l":
      case "L":
        event.preventDefault();
        void preview.play();
        break;
      default:
        break;
    }
  };

  window.addEventListener("keydown", onKeyDown);
  return () => window.removeEventListener("keydown", onKeyDown);
}
