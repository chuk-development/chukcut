/**
 * The razor's cut line: exactly where a click would land, snapped the same way
 * a drag is.
 *
 * Its own component, subscribed to the store itself, so that the pointer moves
 * that drive it re-render one line per lane instead of the whole timeline. The
 * lane it belongs to is a prop; the position is not.
 */

import { memo } from "react";

import type { Id } from "@/modules/project/types";
import { useTimelineStore } from "@/modules/timeline/store";

interface RazorGuideProps {
  trackId: Id;
  /** Pixels per microsecond. */
  zoom: number;
}

function Guide({ trackId, zoom }: RazorGuideProps) {
  const target = useTimelineStore((state) => state.razorTarget);
  if (!target || target.trackId !== trackId) return null;

  return (
    <span
      aria-hidden
      data-slot="razor-guide"
      className="pointer-events-none absolute inset-y-0 z-20 w-px bg-timeline-razor"
      style={{ left: target.at * zoom }}
    />
  );
}

export const RazorGuide = memo(Guide);
