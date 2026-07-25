import type { Micros } from "@/modules/project/types";

interface PlayheadProps {
  time: Micros;
  zoom: number;
  /** Pointer-down on the head starts a scrub; the line itself stays inert. */
  onGrab: (event: React.PointerEvent) => void;
}

/**
 * The playhead spans the ruler and every lane, so it lives in the scrolling
 * content rather than in either. The head is sticky against the top of the
 * viewport so it stays grabbable when the lane list is scrolled down.
 */
export function Playhead({ time, zoom, onGrab }: PlayheadProps) {
  return (
    <div
      data-slot="playhead"
      className="pointer-events-none absolute inset-y-0 z-30 w-px bg-timeline-playhead"
      style={{ left: time * zoom }}
    >
      <button
        type="button"
        aria-label="Playhead"
        onPointerDown={onGrab}
        className="pointer-events-auto sticky top-0 -ml-[6px] block h-[13px] w-[13px] cursor-ew-resize border-none bg-transparent p-0 outline-none"
      >
        <span className="block size-full rounded-[2px] rounded-b-[6px] bg-timeline-playhead" />
      </button>
    </div>
  );
}
