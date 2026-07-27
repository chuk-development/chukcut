import { useTimelineStore } from "@/modules/timeline/store";

interface PlayheadProps {
  zoom: number;
  /** Pointer-down on the head starts a scrub; the line itself stays inert. */
  onGrab: (event: React.PointerEvent) => void;
}

/**
 * The playhead spans the ruler and every lane, so it lives in the scrolling
 * content rather than in either. The head is sticky against the top of the
 * viewport so it stays grabbable when the lane list is scrolled down.
 *
 * It reads its position from the store itself instead of taking it as a prop,
 * and that is a performance decision, not a style one: the playhead is the
 * fastest-changing value in the app — every position event during playback,
 * every pointer move during a scrub — and a prop puts that change rate on
 * whichever component passes it. It used to be a prop, and the component
 * passing it was the entire timeline: one thin line moved 24 times a second by
 * re-rendering fifteen hundred lines of JSX. `bodyPaintCount` in `Timeline.tsx`
 * is the proof this stays fixed.
 */
export function Playhead({ zoom, onGrab }: PlayheadProps) {
  const time = useTimelineStore((s) => s.playhead);
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
