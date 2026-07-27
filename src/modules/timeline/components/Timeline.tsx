import { FoldHorizontalIcon, PlusIcon } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { MEDIA_DRAG_MIME } from "@/lib/dnd";
import { clamp, MICROS_PER_SECOND } from "@/lib/time";
import { useMediaStore } from "@/modules/media/store";
import { preview } from "@/modules/preview/lib/session";
import { usePreviewStore } from "@/modules/preview/store";
import { useProjectStore } from "@/modules/project/store";
import type {
  Id,
  Micros,
  Project,
  // The component below is also called `Segment`; the document's one is the
  // model.
  Segment as SegmentModel,
  TimeRange,
  Track,
  TrackKind,
} from "@/modules/project/types";
import {
  findSegment,
  linkedPartners,
  linkGroupOf,
  materialDuration,
  projectDuration,
  rangeEnd,
  segmentLabel,
  soundIsOnALinkedLane,
} from "@/modules/project/types";
import { Playhead } from "@/modules/timeline/components/Playhead";
import { RazorGuide } from "@/modules/timeline/components/RazorGuide";
import { Segment, type SegmentGesture } from "@/modules/timeline/components/Segment";
import { TimelineToolbar } from "@/modules/timeline/components/TimelineToolbar";
import { TimeRuler } from "@/modules/timeline/components/TimeRuler";
import { TrackHeader, TrackLane } from "@/modules/timeline/components/Track";
import { registerTimelineDropTarget } from "@/modules/timeline/lib/dropTarget";
import {
  copySegments,
  deleteSegments,
  duplicateSegments,
  insertMaterial,
  linkSegments,
  minClipDuration,
  moveSegments,
  pasteEntries,
  redo,
  type SegmentMove,
  type SegmentTrim,
  segmentUnderPlayhead,
  splitAt,
  toggleTrackFlag,
  trimSegments,
  undo,
  unlinkSegment,
} from "@/modules/timeline/lib/edits";
import { applyFades, type Fades } from "@/modules/timeline/lib/fades";
import { buildMaterialIndex } from "@/modules/timeline/lib/materials";
import { freeSpan, nearestFreeStart } from "@/modules/timeline/lib/placement";
import { closeGap, gapAt, rippleDeleteSegment } from "@/modules/timeline/lib/ripple";
import {
  allSelectableIds,
  liveSelection,
  runBetween,
  type SelectionBand,
  segmentsInBand,
} from "@/modules/timeline/lib/selection";
import {
  buildSnapContext,
  type SnapContext,
  snapInstant,
  snapRadius,
  snapRange,
} from "@/modules/timeline/lib/snapping";
import {
  ADDABLE_KINDS,
  addTrack,
  deleteTrack,
  moveTrack,
  reorderTarget,
} from "@/modules/timeline/lib/tracks";
import {
  MAX_ZOOM,
  MIN_ZOOM,
  type RazorTarget,
  RULER_HEIGHT,
  soleSelection,
  TRACK_HEADER_WIDTH,
  trackHeight,
  useTimelineStore,
} from "@/modules/timeline/store";
import { TransitionLane } from "@/modules/transitions/components/TransitionLane";
import { applyOverlap, overlapGesture } from "@/modules/transitions/lib/edits";

/** Empty timeline still shows this much time, so the ruler is never a stub. */
const MIN_VISIBLE_SPAN = 20 * MICROS_PER_SECOND;
/** Room past the last clip to drag into. */
const TRAILING_PX = 480;
/**
 * How far past the edges of the screen clips keep drawing, and how coarsely the
 * window moves.
 *
 * A scroll publishes a new pixel offset on every wheel tick, and the visible
 * window is a prop on every clip — so an un-quantised window means fifty clips
 * recompute their tiles and their waveform columns for a two-pixel scroll. With
 * the window snapped to a grid it changes once per grid step, and the overscan
 * is what stops the edges being empty in between.
 */
const VIEWPORT_QUANTUM = 256;
const VIEWPORT_OVERSCAN = 512;
/**
 * How far the pointer has to travel on an empty lane before the press stops
 * being a click and becomes a rubber band. Below this, a click that wobbles by
 * a pixel would flash a selection box.
 */
const BAND_THRESHOLD_PX = 4;

/**
 * Clips that will move with the one being dragged, and where they are now.
 *
 * Two kinds of them, and the difference is who builds the edit:
 *
 * - **Link partners.** The edit is Rust's — `History::apply` expands a move of
 *   a linked clip into a move of the whole group — so the gesture sends nothing
 *   for these. `selected` is false.
 * - **The rest of the selection.** Four clips dragged together are four
 *   commands in one batch, and the gesture has to send all four.
 *
 * Either way the *drag* has to show them moving, or the timeline reads as one
 * clip moving until the mouse comes up and the others jump. Captured when the
 * gesture starts, because the document does not change during one.
 */
interface Partner {
  id: Id;
  trackId: Id;
  target: TimeRange;
  source: TimeRange;
  speed: number;
  /** For the source limit a trim may not pull past. */
  materialId: Id;
  /** In the selection, so this gesture owns its command. */
  selected: boolean;
}

/**
 * Everyone who moves when `grabbedId` moves, except `grabbedId` itself.
 *
 * The selection first, in document order, then the link partners of everything
 * in it — and a clip is only listed once, because a selection that holds both
 * halves of a linked pair must not move the sound twice. (Rust guards the same
 * thing on the edit itself, in `compose_edits`; this is the preview's copy of
 * the rule, and the two have to agree or the drag lies about where things will
 * land.)
 */
function travellers(project: Project, movingIds: readonly Id[], grabbedId: Id): Partner[] {
  const moving = new Set(movingIds);
  const claimed = new Set(movingIds);
  const partners: Partner[] = [];

  const describe = (track: Track, segment: SegmentModel, isSelected: boolean): Partner => ({
    id: segment.id,
    trackId: track.id,
    target: segment.target_range,
    source: segment.source_range,
    speed: segment.speed > 0 ? segment.speed : 1,
    materialId: segment.material_id,
    selected: isSelected,
  });

  for (const track of project.tracks) {
    for (const segment of track.segments) {
      if (segment.id !== grabbedId && moving.has(segment.id)) {
        partners.push(describe(track, segment, true));
      }
    }
  }

  for (const id of movingIds) {
    for (const partner of linkedPartners(project, id)) {
      if (claimed.has(partner.id)) continue;
      claimed.add(partner.id);
      const track = project.tracks.find((lane) => lane.segments.some((s) => s.id === partner.id));
      if (track) partners.push(describe(track, partner, false));
    }
  }

  return partners;
}

type DragState =
  | {
      kind: "move";
      segmentId: Id;
      fromTrackId: Id;
      toTrackId: Id;
      originStart: Micros;
      start: Micros;
      /**
       * Where the pointer asked the clip to go, before the clamp that keeps it
       * from overlapping its neighbours.
       *
       * Kept because the difference between this and `start` is the whole
       * transition gesture: dragging a clip back over the neighbour it already
       * touches leaves `start` unchanged — the clamp refuses the overlap — and
       * the overlap the user drew survives only here.
       */
      proposedStart: Micros;
      duration: Micros;
      partners: Partner[];
      /** Position of the live snap, for the guide line. Null when nothing is docked. */
      snapAt: Micros | null;
    }
  | {
      kind: "trim";
      segmentId: Id;
      edge: "start" | "end";
      beforeTarget: TimeRange;
      beforeSource: TimeRange;
      afterTarget: TimeRange;
      afterSource: TimeRange;
      partners: Partner[];
      snapAt: Micros | null;
    }
  | null;

/**
 * A rubber band, while it is being dragged.
 *
 * In pixels relative to the scrolling content rather than in microseconds,
 * because it is a rectangle on screen: the vertical extent has no time in it at
 * all, and converting back and forth twice per pointer move to draw a box would
 * be arithmetic for its own sake. It becomes a time range and a set of lanes
 * once, on release, in `segmentsInBand`.
 */
interface BandState {
  fromX: number;
  fromY: number;
  toX: number;
  toY: number;
  /** Client coordinates, kept for the lane hit test, which works in those. */
  fromClientY: number;
  toClientY: number;
}

/** Where a partner sits while its group is being dragged by the same deltas. */
function partnerPreview(
  drag: NonNullable<DragState>,
  segmentId: Id,
): { start: Micros; duration: Micros } | null {
  const partner = drag.partners.find((candidate) => candidate.id === segmentId);
  if (!partner) return null;
  if (drag.kind === "move") {
    const delta = drag.start - drag.originStart;
    return { start: partner.target.start + delta, duration: partner.target.duration };
  }
  const head = drag.afterTarget.start - drag.beforeTarget.start;
  const tail = rangeEnd(drag.afterTarget) - rangeEnd(drag.beforeTarget);
  return {
    start: partner.target.start + head,
    duration: partner.target.duration + tail - head,
  };
}

/**
 * How many times the timeline body has rendered.
 *
 * Same instrument as `clipPaintCount`, same reason: the claim it guards can
 * only be observed from inside the component. The claim is that **the playhead
 * is not a subscription of this component**. It was — the fastest-changing
 * value in the app, written on every position event during playback and every
 * pointer move during a scrub, re-rendering these fifteen hundred lines each
 * time to move a one-pixel line and a timecode that read it themselves ever
 * since. `Timeline.paint.test.tsx` pins it.
 */
export const bodyPaintCount = { renders: 0 };

/**
 * The empty stretch of a lane: click seeks, drag draws a rubber band, and a
 * right-click offers to close the gap under the pointer.
 *
 * A gap is not a component, so the menu has to work out at open time what it
 * is pointing at — the instant under the cursor and whether a bounded gap sits
 * there. That answer is snapshotted into state when the menu opens rather than
 * derived in the item's handler, because by the time "Close gap" is clicked
 * the pointer is on the menu, not on the gap.
 */
function LaneSurface({
  track,
  onPointerDown,
  timeAtClientX,
}: {
  track: Track;
  onPointerDown: (event: React.PointerEvent) => void;
  timeAtClientX: (clientX: number) => Micros;
}) {
  const [gapMenu, setGapMenu] = useState<{ at: Micros; open: boolean }>({ at: 0, open: false });
  const gap = gapMenu.open ? gapAt(track, gapMenu.at) : null;

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        {/* Empty lane space: clicking it deselects and moves the playhead,
            dragging draws a rubber band — except under the razor, where a
            click on a gap is a cut that has nothing to cut. */}
        <button
          type="button"
          tabIndex={-1}
          aria-label={`${track.name} lane`}
          className="absolute inset-0 cursor-default border-none bg-transparent p-0 outline-none"
          onPointerDown={onPointerDown}
          onContextMenu={(event) => setGapMenu({ at: timeAtClientX(event.clientX), open: true })}
        />
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem
          disabled={gap === null || track.locked}
          onSelect={() => {
            const current = useProjectStore.getState().project;
            if (current) void closeGap(current, track.id, gapMenu.at);
          }}
        >
          <FoldHorizontalIcon />
          Close gap
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

export function Timeline() {
  bodyPaintCount.renders += 1;

  const project = useProjectStore((s) => s.project);
  const canUndo = useProjectStore((s) => s.canUndo);
  const canRedo = useProjectStore((s) => s.canRedo);
  const setError = useProjectStore((s) => s.setError);

  const zoom = useTimelineStore((s) => s.zoom);
  const scrollX = useTimelineStore((s) => s.scrollX);
  const selection = useTimelineStore((s) => s.selection);
  const tool = useTimelineStore((s) => s.tool);
  const snapping = useTimelineStore((s) => s.snapping);
  const setZoom = useTimelineStore((s) => s.setZoom);
  const setScrollX = useTimelineStore((s) => s.setScrollX);
  const setPlayhead = useTimelineStore((s) => s.setPlayhead);
  const select = useTimelineStore((s) => s.select);
  const selectMany = useTimelineStore((s) => s.selectMany);
  const setTool = useTimelineStore((s) => s.setTool);
  const toggleSnapping = useTimelineStore((s) => s.toggleSnapping);
  // Deliberately not subscribed to `razorTarget`: it moves with the pointer,
  // and reading it here would re-render every lane to move a one-pixel line.
  // `RazorGuide` subscribes to it instead.
  const setRazorTarget = useTimelineStore((s) => s.setRazorTarget);

  const scrollRef = useRef<HTMLDivElement>(null);
  const headerScrollRef = useRef<HTMLDivElement>(null);
  const lanesRef = useRef(new Map<Id, HTMLDivElement>());
  const pendingScrollRef = useRef<number | null>(null);
  const [viewportWidth, setViewportWidth] = useState(0);
  const [drag, setDrag] = useState<DragState>(null);
  const [band, setBand] = useState<BandState | null>(null);

  /**
   * Membership, as a set, rebuilt only when the selection is.
   *
   * Every clip on the timeline asks whether it is in there on every render, and
   * the answer has to be a stable boolean or `React.memo` stops working — see
   * `Timeline.paint.test.tsx`.
   */
  const selected = useMemo(() => new Set(selection), [selection]);

  const fps = project?.fps ?? 30;
  const duration = project ? projectDuration(project) : 0;
  const contentWidth = Math.max(
    viewportWidth,
    Math.max(duration, MIN_VISIBLE_SPAN) * zoom + TRAILING_PX,
  );

  // ---------------------------------------------------------------------
  // Viewport plumbing
  // ---------------------------------------------------------------------

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setViewportWidth(element.clientWidth));
    observer.observe(element);
    setViewportWidth(element.clientWidth);
    return () => observer.disconnect();
  }, []);

  const handleScroll = useCallback(() => {
    const element = scrollRef.current;
    if (!element) return;
    setScrollX(element.scrollLeft);
    if (headerScrollRef.current) headerScrollRef.current.scrollTop = element.scrollTop;
  }, [setScrollX]);

  // Wheel has to be a native non-passive listener: React routes it through a
  // passive root listener, where preventDefault is ignored.
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;

    const onWheel = (event: WheelEvent) => {
      if (event.ctrlKey || event.metaKey) {
        event.preventDefault();
        const state = useTimelineStore.getState();
        const rect = element.getBoundingClientRect();
        const cursorPx = event.clientX - rect.left + element.scrollLeft;
        const cursorTime = cursorPx / state.zoom;
        const next = clamp(state.zoom * Math.exp(-event.deltaY * 0.0015), MIN_ZOOM, MAX_ZOOM);
        // Keep the instant under the cursor pinned while the scale changes.
        pendingScrollRef.current = Math.max(0, cursorTime * next - (event.clientX - rect.left));
        state.setZoom(next);
        return;
      }
      if (event.shiftKey) return;
      event.preventDefault();
      element.scrollLeft += event.deltaY + event.deltaX;
    };

    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, []);

  // `zoom` is the trigger, not a read: the anchored scroll position has to land
  // in the same frame the new scale does, or the timeline visibly jumps.
  // biome-ignore lint/correctness/useExhaustiveDependencies: zoom is the trigger for this effect
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element || pendingScrollRef.current === null) return;
    element.scrollLeft = pendingScrollRef.current;
    pendingScrollRef.current = null;
    setScrollX(element.scrollLeft);
  }, [zoom, setScrollX]);

  const registerLane = useCallback((trackId: Id, element: HTMLDivElement | null) => {
    if (element) lanesRef.current.set(trackId, element);
    else lanesRef.current.delete(trackId);
  }, []);

  const timeAtClientX = useCallback((clientX: number): Micros => {
    const element = scrollRef.current;
    if (!element) return 0;
    const rect = element.getBoundingClientRect();
    return Math.max(
      0,
      (clientX - rect.left + element.scrollLeft) / useTimelineStore.getState().zoom,
    );
  }, []);

  const laneAtClientY = useCallback((clientY: number): Id | null => {
    for (const [trackId, element] of lanesRef.current) {
      const rect = element.getBoundingClientRect();
      if (clientY >= rect.top && clientY <= rect.bottom) return trackId;
    }
    return null;
  }, []);

  /** Every lane a vertical drag crossed, ends included. */
  const lanesBetween = useCallback((fromClientY: number, toClientY: number): Id[] => {
    const top = Math.min(fromClientY, toClientY);
    const bottom = Math.max(fromClientY, toClientY);
    const hit: Id[] = [];
    for (const [trackId, element] of lanesRef.current) {
      const rect = element.getBoundingClientRect();
      if (rect.top <= bottom && top <= rect.bottom) hit.push(trackId);
    }
    return hit;
  }, []);

  // ---------------------------------------------------------------------
  // Gestures
  //
  // Intermediate positions are local state and never reach Rust. One command
  // is emitted on release; see docs/architecture/timeline-editing.md.
  // ---------------------------------------------------------------------

  /**
   * Drag the playhead.
   *
   * Every position seeks. They are not queued or throttled: each `preview_seek`
   * supersedes the last, so a forty-position drag costs forty cheap calls and
   * renders the one frame the user stopped on.
   */
  const scrub = useCallback(
    (event: React.PointerEvent) => {
      const state = useTimelineStore.getState();
      // Null playhead: it is the thing moving, so it cannot be a candidate.
      const context = buildSnapContext(useProjectStore.getState().project, null, null);
      const radius = snapRadius(state.zoom);
      const apply = (clientX: number, altKey: boolean) => {
        const raw = timeAtClientX(clientX);
        const hit = state.snapping && !altKey ? snapInstant(raw, context, radius) : null;
        const time = hit?.value ?? raw;
        setPlayhead(time);
        void preview.seek(time);
      };
      apply(event.clientX, event.altKey);

      const onMove = (moveEvent: PointerEvent) => apply(moveEvent.clientX, moveEvent.altKey);
      const onUp = () => window.removeEventListener("pointermove", onMove);
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp, { once: true });
    },
    [setPlayhead, timeAtClientX],
  );

  /**
   * Where a razor click at this point would cut.
   *
   * The cut is where the mouse is, not where the playhead is — that is the
   * whole difference between the tool and the `C` shortcut. It has to land
   * strictly inside a clip: Rust rejects a split on an edge, and a cut at a
   * boundary would produce a zero-length clip anyway. So a snap that lands on
   * this clip's own boundary is dropped rather than clamped, and the razor
   * simply stays under the pointer. A gap or a locked lane is not a cut at all.
   */
  const razorTargetAt = useCallback(
    (clientX: number, clientY: number, altKey: boolean): RazorTarget | null => {
      const current = useProjectStore.getState().project;
      if (!current) return null;
      const laneId = laneAtClientY(clientY);
      const track = current.tracks.find((candidate) => candidate.id === laneId);
      if (!track || track.locked) return null;

      const time = Math.round(timeAtClientX(clientX));
      const segment = track.segments.find(
        (s) => time > s.target_range.start && time < rangeEnd(s.target_range),
      );
      if (!segment) return null;

      const state = useTimelineStore.getState();
      let at = time;
      if (state.snapping && !altKey) {
        const context = buildSnapContext(current, state.playhead, segment.id);
        const hit = snapInstant(time, context, snapRadius(state.zoom));
        if (
          hit &&
          hit.value > segment.target_range.start &&
          hit.value < rangeEnd(segment.target_range)
        ) {
          at = hit.value;
        }
      }
      return { trackId: track.id, segmentId: segment.id, at };
    },
    [laneAtClientY, timeAtClientX],
  );

  const trackRazor = useCallback(
    (event: React.PointerEvent) => {
      if (useTimelineStore.getState().tool !== "razor") return;
      setRazorTarget(razorTargetAt(event.clientX, event.clientY, event.altKey));
    },
    [razorTargetAt, setRazorTarget],
  );

  const clearRazor = useCallback(() => setRazorTarget(null), [setRazorTarget]);

  // The cut line is a property of the razor, so it goes away with the tool.
  useEffect(() => {
    if (tool !== "razor") setRazorTarget(null);
  }, [tool, setRazorTarget]);

  const beginGesture = useCallback(
    (event: React.PointerEvent, gesture: SegmentGesture, segmentId: Id) => {
      const current = useProjectStore.getState().project;
      if (!current) return;
      const found = findSegment(current, segmentId);
      if (!found || found.track.locked) return;

      // The razor replaces dragging entirely: with it selected a click cuts,
      // at the pointer rather than at the playhead.
      if (useTimelineStore.getState().tool === "razor") {
        if (gesture !== "move") return;
        const target = razorTargetAt(event.clientX, event.clientY, event.altKey);
        if (target) void splitAt(target.segmentId, target.at);
        return;
      }

      event.preventDefault();
      const { segment, track } = found;

      // The clips that will travel with this one, and the material already on
      // their lanes. Both matter: the first so the drag *shows* the group
      // moving, the second so it never comes to rest somewhere the group does
      // not fit. Rust refuses a move whose mirror lands on an occupied range,
      // and refuses the whole thing — so a drop the audio lane cannot take
      // leaves the picture where it was, which reads as the drag having done
      // nothing at all.
      //
      // Grabbing a clip that is part of a multi-selection drags the whole
      // selection; grabbing one that is not drags it alone, whatever else is
      // selected. That is the rule everywhere else a list can be dragged.
      const chosen = liveSelection(current, useTimelineStore.getState().selection);
      const dragsTheSelection = chosen.length > 1 && chosen.includes(segmentId);
      const partners = travellers(current, dragsTheSelection ? chosen : [segmentId], segmentId);
      const travellingIds = new Set([segmentId, ...partners.map((partner) => partner.id)]);
      const partnerOccupants = partners.length
        ? current.tracks
            .filter((lane) => lane.segments.some((s) => travellingIds.has(s.id)))
            .flatMap((lane) => lane.segments)
            .filter((s) => !travellingIds.has(s.id))
        : [];

      /** What is on a lane that is not going anywhere. */
      const stationaryOn = (trackId: Id) =>
        (current.tracks.find((lane) => lane.id === trackId)?.segments ?? []).filter(
          (s) => !travellingIds.has(s.id),
        );

      /** Everything the gesture moves, the grabbed clip included. */
      const movers: Partner[] = [
        {
          id: segmentId,
          trackId: track.id,
          target: segment.target_range,
          source: segment.source_range,
          speed: segment.speed > 0 ? segment.speed : 1,
          materialId: segment.material_id,
          selected: true,
        },
        ...partners,
      ];

      const startClientX = event.clientX;
      const { zoom: zoom0, snapping: snapping0 } = useTimelineStore.getState();
      const radius = snapRadius(zoom0);
      const context: SnapContext = buildSnapContext(
        current,
        useTimelineStore.getState().playhead,
        segmentId,
      );
      const minDuration = minClipDuration(current.fps);
      const speed = segment.speed > 0 ? segment.speed : 1;
      const sourceLimit = materialDuration(current, segment.material_id);
      // How far the trim handles can travel before they overlap a neighbour —
      // on this lane or on a linked clip's.
      const room = freeSpan(
        [...track.segments, ...partnerOccupants],
        segmentId,
        segment.target_range,
      );

      /**
       * How far the whole convoy may travel, at each edge.
       *
       * A group gesture applies *one* delta to every clip in it — that is what
       * makes it one gesture — so the delta it may use is the intersection of
       * what each clip can take. Anything looser and the batch is refused as a
       * whole, which reads as the drag having done nothing.
       */
      const convoy = movers.map((mover) => {
        const span = freeSpan(stationaryOn(mover.trackId), null, mover.target);
        const limit = materialDuration(current, mover.materialId);
        return {
          moveMin: span.min - mover.target.start,
          moveMax: span.max - rangeEnd(mover.target),
          headMin: Math.max(
            -Math.floor(mover.source.start / mover.speed),
            span.min - mover.target.start,
          ),
          headMax: mover.target.duration - minDuration,
          tailMin: minDuration - mover.target.duration,
          tailMax: Math.min(
            span.max - rangeEnd(mover.target),
            limit === null
              ? Number.POSITIVE_INFINITY
              : Math.floor((limit - rangeEnd(mover.source)) / mover.speed),
          ),
        };
      });
      const together = (
        pick: (limits: (typeof convoy)[number]) => number,
        how: "min" | "max",
      ): number =>
        convoy.reduce(
          (best, limits) =>
            how === "min" ? Math.max(best, pick(limits)) : Math.min(best, pick(limits)),
          how === "min" ? Number.NEGATIVE_INFINITY : Number.POSITIVE_INFINITY,
        );

      let live: DragState = null;

      const onMove = (moveEvent: PointerEvent) => {
        const deltaMicros = (moveEvent.clientX - startClientX) / zoom0;
        // Alt suspends magnetism for as long as it is held, for the times when
        // the clip belongs a few microseconds off a neighbour on purpose.
        const magnetic = snapping0 && !moveEvent.altKey;

        if (gesture === "move") {
          const duration = segment.target_range.duration;
          const wanted = Math.max(0, segment.target_range.start + deltaMicros);
          const hit = magnetic ? snapRange(wanted, duration, context, radius) : null;
          const start = Math.max(0, Math.round(hit?.value ?? wanted));

          // A selection travels in time only, and every clip in it stays on its
          // own lane. Dropping four clips from four lanes onto the one under
          // the pointer is not a gesture anyone means to make.
          if (dragsTheSelection) {
            const wantedDelta = start - segment.target_range.start;
            const delta = clamp(
              wantedDelta,
              together((limits) => limits.moveMin, "min"),
              together((limits) => limits.moveMax, "max"),
            );
            const landedTogether = segment.target_range.start + delta;
            live = {
              kind: "move",
              segmentId,
              fromTrackId: track.id,
              toTrackId: track.id,
              originStart: segment.target_range.start,
              start: landedTogether,
              proposedStart: start,
              duration,
              partners,
              // Only claim the snap the clamp did not take back.
              snapAt: landedTogether === start ? (hit?.at ?? null) : null,
            };
            setDrag(live);
            return;
          }

          const laneId = laneAtClientY(moveEvent.clientY);
          const lane = current.tracks.find((t) => t.id === laneId && !t.locked) ?? track;
          // A drag never ends in "target range is occupied": if the drop lands
          // on material, it butts up against the side it was dropped on.
          const landed = nearestFreeStart(
            [...lane.segments, ...partnerOccupants],
            start,
            duration,
            segmentId,
          );

          live = {
            kind: "move",
            segmentId,
            fromTrackId: track.id,
            toTrackId: lane.id,
            originStart: segment.target_range.start,
            start: landed,
            proposedStart: start,
            duration,
            partners,
            snapAt:
              landed === start
                ? (hit?.at ?? null)
                : // Pushed off the snap by a neighbour: the guide belongs on the
                  // edge it is now resting against.
                  landed > start
                  ? landed
                  : landed + duration,
          };
        } else if (gesture === "trim-start") {
          const originalStart = segment.target_range.start;
          const wanted = Math.max(0, originalStart + deltaMicros);
          const hit = magnetic ? snapInstant(wanted, context, radius) : null;
          let delta = Math.round((hit?.value ?? wanted) - originalStart);
          if (dragsTheSelection) {
            // Every clip in the selection gives up the same amount of head, so
            // the limit is whichever of them runs out of material or of room
            // first.
            delta = clamp(
              delta,
              together((limits) => limits.headMin, "min"),
              together((limits) => limits.headMax, "max"),
            );
          } else {
            // Cannot swallow the whole clip, and cannot run off the head of the source.
            delta = Math.min(delta, segment.target_range.duration - minDuration);
            delta = Math.max(delta, -Math.floor(segment.source_range.start / speed));
            delta = Math.max(delta, room.min - originalStart);
          }
          const start = originalStart + delta;
          live = {
            kind: "trim",
            segmentId,
            edge: "start",
            beforeTarget: segment.target_range,
            beforeSource: segment.source_range,
            afterTarget: {
              start,
              duration: segment.target_range.duration - delta,
            },
            afterSource: {
              start: Math.round(segment.source_range.start + delta * speed),
              duration: Math.round(segment.source_range.duration - delta * speed),
            },
            partners,
            // Only claim a snap the clamps did not take back.
            snapAt: hit && hit.at === start ? hit.at : null,
          };
        } else {
          const originalEnd = rangeEnd(segment.target_range);
          const wanted = originalEnd + deltaMicros;
          const hit = magnetic ? snapInstant(wanted, context, radius) : null;
          let delta = Math.round((hit?.value ?? wanted) - originalEnd);
          if (dragsTheSelection) {
            delta = clamp(
              delta,
              together((limits) => limits.tailMin, "min"),
              together((limits) => limits.tailMax, "max"),
            );
          } else {
            delta = Math.max(delta, minDuration - segment.target_range.duration);
            if (sourceLimit !== null) {
              const spare = sourceLimit - rangeEnd(segment.source_range);
              delta = Math.min(delta, Math.floor(spare / speed));
            }
            delta = Math.min(delta, room.max - originalEnd);
          }
          const end = originalEnd + delta;
          live = {
            kind: "trim",
            segmentId,
            edge: "end",
            beforeTarget: segment.target_range,
            beforeSource: segment.source_range,
            afterTarget: {
              start: segment.target_range.start,
              duration: segment.target_range.duration + delta,
            },
            afterSource: {
              start: segment.source_range.start,
              duration: Math.round(segment.source_range.duration + delta * speed),
            },
            partners,
            snapAt: hit && hit.at === end ? hit.at : null,
          };
        }

        setDrag(live);
      };

      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        setDrag(null);
        const final = live;
        if (!final) return;

        // Only the selected travellers get a command. A link partner is Rust's
        // to move — `compose_edits` mirrors it — and sending one as well would
        // move it twice.
        const alsoMoving = final.partners.filter((partner) => partner.selected);

        if (final.kind === "move") {
          // Dragging a clip back over the neighbour it already touches is not a
          // move at all: the document forbids the overlap, so the clamp holds
          // the clip still and `moved` below is false. What the user drew is
          // the length of a transition, and this is the only place that reading
          // is still available.
          //
          // Single clip, same lane, dragged leftwards — a batch drag has no one
          // unambiguous join to attach to, so it stays an ordinary move.
          if (
            alsoMoving.length === 0 &&
            final.toTrackId === final.fromTrackId &&
            final.proposedStart < final.originStart
          ) {
            const document = useProjectStore.getState().project;
            if (document && overlapGesture(document, final.segmentId, final.proposedStart)) {
              void applyOverlap(document, final.segmentId, final.proposedStart);
              return;
            }
          }

          const moved = final.start !== final.originStart || final.toTrackId !== final.fromTrackId;
          if (!moved) return;
          const delta = final.start - final.originStart;
          const moves: SegmentMove[] = [
            {
              segmentId: final.segmentId,
              fromTrackId: final.fromTrackId,
              toTrackId: final.toTrackId,
              fromStart: final.originStart,
              toStart: final.start,
            },
            ...alsoMoving.map((partner) => ({
              segmentId: partner.id,
              fromTrackId: partner.trackId,
              toTrackId: partner.trackId,
              fromStart: partner.target.start,
              toStart: partner.target.start + delta,
            })),
          ];
          void moveSegments(moves);
          return;
        }

        const unchanged =
          final.afterTarget.start === final.beforeTarget.start &&
          final.afterTarget.duration === final.beforeTarget.duration;
        if (unchanged) return;

        // The same two deltas every clip in the selection takes, which is what
        // the drag has been drawing all along.
        const head = final.afterTarget.start - final.beforeTarget.start;
        const tail = rangeEnd(final.afterTarget) - rangeEnd(final.beforeTarget);
        const trims: SegmentTrim[] = [
          {
            segmentId: final.segmentId,
            beforeTarget: final.beforeTarget,
            beforeSource: final.beforeSource,
            afterTarget: final.afterTarget,
            afterSource: final.afterSource,
          },
          ...alsoMoving.map((partner) => {
            const target = {
              start: partner.target.start + head,
              duration: partner.target.duration + tail - head,
            };
            return {
              segmentId: partner.id,
              beforeTarget: partner.target,
              beforeSource: partner.source,
              afterTarget: target,
              // Each clip's source follows at its own speed: a head trim of
              // 100 ms on a 2x clip consumes 200 ms of file. The same
              // arithmetic as `mirror_trim` in Rust, which has to agree with
              // this or the edit is refused for breaking the speed invariant.
              afterSource: {
                start: Math.round(partner.source.start + head * partner.speed),
                duration: Math.round(target.duration * partner.speed),
              },
            };
          }),
        ];
        void trimSegments(trims);
      };

      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp, { once: true });
    },
    [laneAtClientY, razorTargetAt],
  );

  // ---------------------------------------------------------------------
  // Commands
  // ---------------------------------------------------------------------

  const handleSplit = useCallback(() => {
    const current = useProjectStore.getState().project;
    if (!current) return;
    const { playhead: at, selection: chosen } = useTimelineStore.getState();
    // With several clips selected there is no "the" selected clip to prefer, so
    // the razor falls back to what is under the playhead — which is the same
    // answer it gives with nothing selected at all.
    const hit = segmentUnderPlayhead(current, at, soleSelection(chosen));
    if (!hit) {
      setError("Nothing to split: put the playhead over a clip first.");
      return;
    }
    void splitAt(hit.segmentId, at);
  }, [setError]);

  const handleDelete = useCallback(() => {
    const current = useProjectStore.getState().project;
    const ids = liveSelection(current, useTimelineStore.getState().selection);
    if (!current || ids.length === 0) return;
    select(null);
    void deleteSegments(current, ids);
  }, [select]);

  const handleDuplicate = useCallback(() => {
    const current = useProjectStore.getState().project;
    const ids = liveSelection(current, useTimelineStore.getState().selection);
    if (!current || ids.length === 0) return;
    void duplicateSegments(current, ids);
  }, []);

  // ---------------------------------------------------------------------
  // The clipboard
  //
  // Copying is not an edit: it takes nothing from the document and puts
  // nothing in the history. It only fills the store. Pasting is one edit, and
  // one undo step, however many clips it puts down.
  // ---------------------------------------------------------------------

  const handleCopy = useCallback(() => {
    const current = useProjectStore.getState().project;
    const ids = liveSelection(current, useTimelineStore.getState().selection);
    if (!current || ids.length === 0) return;
    useTimelineStore.getState().setClipboard(copySegments(current, ids));
  }, []);

  const handleCut = useCallback(() => {
    const current = useProjectStore.getState().project;
    const ids = liveSelection(current, useTimelineStore.getState().selection);
    if (!current || ids.length === 0) return;
    useTimelineStore.getState().setClipboard(copySegments(current, ids));
    select(null);
    void deleteSegments(current, ids);
  }, [select]);

  const handlePaste = useCallback(() => {
    const current = useProjectStore.getState().project;
    const { clipboard, playhead: at } = useTimelineStore.getState();
    if (!current || clipboard.length === 0) return;
    void pasteEntries(current, clipboard, at).then((ids) => {
      // Selecting what landed is half the feature: a paste onto a lane that was
      // not on screen is otherwise indistinguishable from nothing happening.
      if (ids.length > 0) selectMany(ids);
    });
  }, [selectMany]);

  const handleSelectAll = useCallback(() => {
    selectMany(allSelectableIds(useProjectStore.getState().project));
  }, [selectMany]);

  const handleLink = useCallback(() => {
    const current = useProjectStore.getState().project;
    const ids = liveSelection(current, useTimelineStore.getState().selection);
    if (ids.length < 2) return;
    void linkSegments(ids);
  }, []);

  const handleToggleFlag = useCallback((track: Track, flag: "muted" | "locked" | "hidden") => {
    void toggleTrackFlag(track, flag);
  }, []);

  // ---------------------------------------------------------------------
  // Track management
  // ---------------------------------------------------------------------

  const handleAddTrack = useCallback((kind: TrackKind) => {
    const current = useProjectStore.getState().project;
    if (current) void addTrack(current, kind);
  }, []);

  const handleDeleteTrack = useCallback(
    (track: Track) => {
      const current = useProjectStore.getState().project;
      if (!current) return;
      // Whatever was selected on the lane is about to go with it.
      select(null);
      void deleteTrack(current, track.id);
    },
    [select],
  );

  /**
   * Where a header drag would drop, while one is running. Drawn as a line in
   * the header gutter; null the rest of the time.
   */
  const [trackDrop, setTrackDrop] = useState<{ from: number; to: number } | null>(null);

  /**
   * Drag a lane to a new place in the order.
   *
   * The document snapshot from the press is used throughout: the tracks cannot
   * change mid-drag (edits land on release everywhere in this file), and one
   * snapshot means the geometry and the final command agree about indices.
   */
  const beginTrackReorder = useCallback((event: React.PointerEvent, track: Track) => {
    const current = useProjectStore.getState().project;
    if (!current) return;
    const from = current.tracks.findIndex((candidate) => candidate.id === track.id);
    if (from === -1) return;
    event.preventDefault();
    const startY = event.clientY;
    let live = from;

    const onMove = (moveEvent: PointerEvent) => {
      live = reorderTarget(current.tracks, from, moveEvent.clientY - startY);
      setTrackDrop({ from, to: live });
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      setTrackDrop(null);
      if (live !== from) void moveTrack(current, track.id, live);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp, { once: true });
  }, []);

  const zoomToFit = useCallback(() => {
    const width = scrollRef.current?.clientWidth ?? 0;
    const span = Math.max(duration, MIN_VISIBLE_SPAN);
    if (width <= 0 || span <= 0) return;
    setZoom((width - 48) / span);
    if (scrollRef.current) scrollRef.current.scrollLeft = 0;
  }, [duration, setZoom]);

  // ---------------------------------------------------------------------
  // Keyboard
  // ---------------------------------------------------------------------

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName))
      ) {
        return;
      }

      // Nothing here is on screen in fullscreen, and two of these keys are
      // spoken for there: Escape leaves, and a razor toggle the user cannot see
      // is a surprise waiting on the way back.
      if (usePreviewStore.getState().fullscreen) return;

      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        void (event.shiftKey ? redo() : undo());
        return;
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "y") {
        event.preventDefault();
        void redo();
        return;
      }

      // The clipboard four are bound here rather than by the menu bar, and that
      // is not an oversight. It began as a GTK constraint — an accelerator was
      // consulted before the key reached the focused widget, so an enabled
      // Edit → Copy carrying Ctrl+C took the key away from every text field in
      // the app — and it outlived the native menu because it is the right shape
      // anyway: the timeline is what Ctrl+C acts on, the handler above has
      // already declined when the focus is in a field, and the bar only
      // advertises the key. One handler per key; see `workspace/menu.rs`.
      if (event.ctrlKey || event.metaKey) {
        switch (event.key.toLowerCase()) {
          case "a":
            event.preventDefault();
            handleSelectAll();
            return;
          case "c":
            event.preventDefault();
            handleCopy();
            return;
          case "x":
            event.preventDefault();
            handleCut();
            return;
          case "v":
            event.preventDefault();
            handlePaste();
            return;
          case "d":
            event.preventDefault();
            handleDuplicate();
            return;
          default:
            return;
        }
      }
      if (event.altKey) return;

      switch (event.key.toLowerCase()) {
        case "delete":
        case "backspace":
          event.preventDefault();
          handleDelete();
          break;
        case "c":
          event.preventDefault();
          handleSplit();
          break;
        case "v":
          setTool("select");
          break;
        case "b":
          setTool("razor");
          break;
        case "s":
          toggleSnapping();
          break;
        case "escape":
          select(null);
          break;
        default:
          break;
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    handleCopy,
    handleCut,
    handleDelete,
    handleDuplicate,
    handlePaste,
    handleSelectAll,
    handleSplit,
    select,
    setTool,
    toggleSnapping,
  ]);

  // ---------------------------------------------------------------------
  // Media drop
  // ---------------------------------------------------------------------

  /**
   * A tile dragged out of the media library. The material is already in the
   * pool, so this is a plain insert.
   *
   * Handled on the whole viewport rather than per lane: once a lane has a clip
   * in it, the clip covers the lane, and a drop aimed at an occupied stretch
   * would otherwise land on the clip and do nothing. The lane comes from the
   * same pointer hit test the desktop-file path uses.
   */
  const handleLibraryDrop = useCallback(
    (event: React.DragEvent) => {
      const materialId = event.dataTransfer.getData(MEDIA_DRAG_MIME);
      if (!materialId) return;
      event.preventDefault();

      const current = useProjectStore.getState().project;
      if (!current) return;

      const item = useMediaStore.getState().items.find((entry) => entry.id === materialId);
      if (!item) {
        setError("That library item is no longer available; import it again.");
        return;
      }
      void insertMaterial(
        current,
        item,
        timeAtClientX(event.clientX),
        laneAtClientY(event.clientY),
      );
    },
    [laneAtClientY, setError, timeAtClientX],
  );

  const handleDragOver = useCallback((event: React.DragEvent) => {
    if (!event.dataTransfer.types.includes(MEDIA_DRAG_MIME)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  }, []);

  // Desktop file drops arrive as a window event, not a DOM event, so the
  // timeline publishes a hit test and the shell routes the drop to it.
  useEffect(
    () =>
      registerTimelineDropTarget((clientX, clientY) => {
        const element = scrollRef.current;
        if (!element) return null;
        const rect = element.getBoundingClientRect();
        const inside =
          clientX >= rect.left &&
          clientX <= rect.right &&
          clientY >= rect.top &&
          clientY <= rect.bottom;
        if (!inside) return null;
        return { trackId: laneAtClientY(clientY), at: timeAtClientX(clientX) };
      }),
    [laneAtClientY, timeAtClientX],
  );

  // ---------------------------------------------------------------------
  // The files behind the clips
  // ---------------------------------------------------------------------

  /**
   * One descriptor per material, rebuilt only when the document is.
   *
   * Every clip below receives the same object it received last render, which is
   * what lets `React.memo` stop a pointer move at the props comparison. Note
   * that the strips and waveforms themselves are *not* read here: each clip
   * holds and subscribes to its own file, so a batch of thumbnails landing
   * repaints the clips that use that file and nothing else — and a clip that is
   * deleted mid-decode releases the job rather than leaving it to finish for a
   * lane that no longer exists.
   */
  const materials = useMemo(() => buildMaterialIndex(project), [project]);

  /**
   * The time window clips draw into.
   *
   * Snapped to a pixel grid with overscan on both sides: see
   * `VIEWPORT_QUANTUM`. The window is a prop on every clip, so what matters is
   * not that it is right to the pixel but that it stops changing on every
   * wheel tick.
   */
  // Snapped first, memoised second. Keying the memo on `scrollX` itself would
  // hand out a fresh object on every wheel tick carrying the same two numbers,
  // which is the whole failure this is here to avoid.
  const windowLeft = Math.max(
    0,
    Math.floor((scrollX - VIEWPORT_OVERSCAN) / VIEWPORT_QUANTUM) * VIEWPORT_QUANTUM,
  );
  const windowRight =
    Math.ceil((scrollX + Math.max(viewportWidth, 600) + VIEWPORT_OVERSCAN) / VIEWPORT_QUANTUM) *
    VIEWPORT_QUANTUM;
  const viewport = useMemo(
    () => ({ from: windowLeft / zoom, to: windowRight / zoom }),
    [windowLeft, windowRight, zoom],
  );

  // Per-clip handlers, hoisted so that the clips' props stay identical between
  // renders. An arrow function in the JSX below would defeat the memo on every
  // clip on the timeline, on every pointer move.
  const handleSplitSegment = useCallback((segmentId: Id) => {
    void splitAt(segmentId, useTimelineStore.getState().playhead);
  }, []);

  const handleDuplicateSegment = useCallback((segmentId: Id) => {
    const current = useProjectStore.getState().project;
    if (current) void duplicateSegments(current, [segmentId]);
  }, []);

  const handleDeleteSegment = useCallback(
    (segmentId: Id) => {
      const current = useProjectStore.getState().project;
      if (!current) return;
      select(null);
      void deleteSegments(current, [segmentId]);
    },
    [select],
  );

  /**
   * What a click on a clip does to the selection.
   *
   * Three gestures, and the third one is why the anchor exists:
   *
   * - **Plain.** Selects that clip alone — *unless* it is already selected,
   *   which leaves the selection as it is so that a group can be picked up and
   *   dragged. Narrowing a group back down to one clip is a click on empty
   *   space and then a click on the clip.
   * - **Ctrl/Cmd.** Adds it, or takes it out if it was in.
   * - **Shift.** Takes the run between the anchor and this clip, along the one
   *   lane they share. Across lanes there is no run, so it takes the one clip.
   */
  const handleSelectSegment = useCallback((segmentId: Id, event: React.PointerEvent) => {
    const store = useTimelineStore.getState();
    if (event.ctrlKey || event.metaKey) {
      store.toggleSelection(segmentId);
      return;
    }
    if (event.shiftKey) {
      const current = useProjectStore.getState().project;
      store.extendSelection(runBetween(current, store.selectionAnchor, segmentId));
      return;
    }
    if (store.selection.includes(segmentId)) return;
    store.select(segmentId);
  }, []);

  const handleUnlinkSegment = useCallback((segmentId: Id) => {
    void unlinkSegment(segmentId);
  }, []);

  const handleRippleDeleteSegment = useCallback(
    (segmentId: Id) => {
      const current = useProjectStore.getState().project;
      if (!current) return;
      select(null);
      void rippleDeleteSegment(current, segmentId);
    },
    [select],
  );

  /**
   * A fade handle was released. The segment is re-read from the store rather
   * than trusted from the clip's props: the fade diff has to be computed
   * against the document the edit will land on.
   */
  const handleFadeSegment = useCallback((segmentId: Id, fades: Fades) => {
    const current = useProjectStore.getState().project;
    const found = current ? findSegment(current, segmentId) : null;
    if (found) void applyFades(found.segment, fades);
  }, []);

  // ---------------------------------------------------------------------
  // The rubber band
  // ---------------------------------------------------------------------

  /**
   * Press on empty lane space.
   *
   * A click seeks and clears the selection, which is what it always did. A
   * *drag* is a rubber band — dragging across the lanes no longer scrubs, and
   * that is the trade: scrubbing has the ruler strip and the playhead handle,
   * and there is nowhere else a band could start. Selecting as the band moves
   * rather than only on release is what makes it possible to tell what it has
   * caught before letting go.
   */
  const beginBand = useCallback(
    (event: React.PointerEvent) => {
      if (useTimelineStore.getState().tool === "razor") return;
      const element = scrollRef.current;
      if (!element) return;
      event.preventDefault();

      const rect = element.getBoundingClientRect();
      const toContent = (clientX: number, clientY: number) => ({
        x: clientX - rect.left + element.scrollLeft,
        y: clientY - rect.top + element.scrollTop,
      });
      const origin = toContent(event.clientX, event.clientY);
      const originClientY = event.clientY;
      const zoom0 = useTimelineStore.getState().zoom;

      // A modifier means "as well as what is already selected", the same as it
      // does on a clip.
      const additive = event.shiftKey || event.ctrlKey || event.metaKey;
      const kept = additive ? useTimelineStore.getState().selection : [];
      if (!additive) select(null);

      const at = timeAtClientX(event.clientX);
      setPlayhead(at);
      void preview.seek(at);

      let live: BandState | null = null;
      const onMove = (moveEvent: PointerEvent) => {
        const point = toContent(moveEvent.clientX, moveEvent.clientY);
        if (
          !live &&
          Math.abs(point.x - origin.x) < BAND_THRESHOLD_PX &&
          Math.abs(point.y - origin.y) < BAND_THRESHOLD_PX
        ) {
          // Still a click. A band that appeared on a one-pixel wobble would
          // flash on every click on an empty lane.
          return;
        }
        live = {
          fromX: origin.x,
          fromY: origin.y,
          toX: point.x,
          toY: point.y,
          fromClientY: originClientY,
          toClientY: moveEvent.clientY,
        };
        setBand(live);

        const band: SelectionBand = {
          from: Math.min(live.fromX, live.toX) / zoom0,
          to: Math.max(live.fromX, live.toX) / zoom0,
          trackIds: lanesBetween(live.fromClientY, live.toClientY),
        };
        selectMany([...kept, ...segmentsInBand(useProjectStore.getState().project, band)]);
      };

      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        setBand(null);
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp, { once: true });
    },
    [lanesBetween, select, selectMany, setPlayhead, timeAtClientX],
  );

  // ---------------------------------------------------------------------
  // Render
  // ---------------------------------------------------------------------

  /**
   * A project with lanes but nothing in them.
   *
   * Distinct from "no tracks": the lanes are there, striped and ready, and a
   * first-time user has no way of knowing they are a drop target. The hint sits
   * over the viewport rather than inside the scrolling content so it stays
   * centred whatever the zoom is.
   */
  const hasClips = Boolean(project?.tracks.some((track) => track.segments.length > 0));
  const empty = Boolean(project && project.tracks.length > 0 && !hasClips);

  const gridStyle = useMemo(() => {
    const secondPx = MICROS_PER_SECOND * zoom;
    const step = secondPx < 24 ? secondPx * 10 : secondPx;
    return {
      backgroundImage:
        "linear-gradient(to right, var(--timeline-gridline) 0 1px, transparent 1px 100%)",
      backgroundSize: `${step}px 100%`,
    };
  }, [zoom]);

  return (
    <section
      data-slot="timeline"
      className="flex h-full min-h-0 flex-col bg-timeline-background"
      aria-label="Timeline"
    >
      <TimelineToolbar
        duration={duration}
        fps={fps}
        zoom={zoom}
        tool={tool}
        snapping={snapping}
        canUndo={canUndo}
        canRedo={canRedo}
        selectionCount={selection.length}
        hasClips={hasClips}
        onZoom={setZoom}
        onZoomToFit={zoomToFit}
        onSetTool={setTool}
        onToggleSnapping={toggleSnapping}
        onUndo={() => void undo()}
        onRedo={() => void redo()}
        onSplit={handleSplit}
        onDuplicate={handleDuplicate}
        onDelete={handleDelete}
        onLink={handleLink}
      />

      <div className="relative flex min-h-0 flex-1">
        <div
          className="flex shrink-0 flex-col border-r border-border"
          style={{ width: TRACK_HEADER_WIDTH }}
        >
          <div
            className="shrink-0 border-b border-border bg-timeline-ruler"
            style={{ height: RULER_HEIGHT }}
          />
          <div ref={headerScrollRef} className="relative flex-1 overflow-hidden">
            {project?.tracks.map((track) => (
              <TrackHeader
                key={track.id}
                track={track}
                onToggle={handleToggleFlag}
                onDelete={handleDeleteTrack}
                onReorderStart={beginTrackReorder}
              />
            ))}

            {/* Where a dragged lane would land: a line above the header it
                would push down, or under the last one. */}
            {trackDrop && trackDrop.to !== trackDrop.from && project ? (
              <span
                aria-hidden
                data-slot="track-drop-indicator"
                className="pointer-events-none absolute inset-x-0 z-10 h-[2px] bg-primary"
                style={{
                  top: project.tracks
                    .slice(0, trackDrop.to < trackDrop.from ? trackDrop.to : trackDrop.to + 1)
                    .reduce((sum, track) => sum + trackHeight(track.kind), 0),
                }}
              />
            ) : null}

            {project ? (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="mx-2 my-1.5 w-[calc(100%-16px)] justify-start text-muted-foreground"
                    aria-label="Add track"
                  >
                    <PlusIcon />
                    Add track
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start">
                  {ADDABLE_KINDS.map((kind) => (
                    <DropdownMenuItem key={kind} onSelect={() => handleAddTrack(kind)}>
                      {kind === "video"
                        ? "Video track"
                        : kind === "audio"
                          ? "Audio track"
                          : "Text track"}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            ) : null}
          </div>
        </div>

        {/* biome-ignore lint/a11y/noStaticElementInteractions: a drop surface is not a control — there is no keyboard equivalent of "drop here", and the lane buttons inside it remain focusable */}
        <div
          ref={scrollRef}
          onScroll={handleScroll}
          onDragOver={handleDragOver}
          onDrop={handleLibraryDrop}
          onPointerMove={trackRazor}
          onPointerLeave={clearRazor}
          className="relative flex-1 overflow-auto"
          data-slot="timeline-viewport"
        >
          <div className="relative" style={{ width: contentWidth }}>
            <TimeRuler
              zoom={zoom}
              fps={fps}
              width={contentWidth}
              scrollX={scrollX}
              viewportWidth={viewportWidth}
            />

            {/* Scrub band: the whole ruler strip is a click target. Out of the
                tab order like the lane surfaces below it — there is no keyboard
                equivalent of dragging, and a focus ring on an invisible strip
                is a stop on the way to the controls that do have one. */}
            <button
              type="button"
              tabIndex={-1}
              aria-label="Scrub"
              onPointerDown={scrub}
              className="absolute left-0 top-0 z-20 h-[26px] w-full cursor-ew-resize border-none bg-transparent p-0 outline-none"
              style={{ width: contentWidth }}
            />

            <div style={gridStyle}>
              {project?.tracks.map((track) => {
                const external =
                  drag?.kind === "move" &&
                  drag.toTrackId === track.id &&
                  drag.fromTrackId !== track.id
                    ? findSegment(project, drag.segmentId)
                    : null;

                return (
                  <TrackLane
                    key={track.id}
                    track={track}
                    dropTarget={external !== null}
                    registerLane={registerLane}
                  >
                    <LaneSurface
                      track={track}
                      onPointerDown={beginBand}
                      timeAtClientX={timeAtClientX}
                    />

                    {track.segments.map((segment) => {
                      const dragging = drag?.kind === "move" && drag.segmentId === segment.id;
                      const trimming = drag?.kind === "trim" && drag.segmentId === segment.id;
                      const preview = dragging
                        ? { start: drag.start, duration: drag.duration }
                        : trimming
                          ? {
                              start: drag.afterTarget.start,
                              duration: drag.afterTarget.duration,
                            }
                          : // A clip linked to the one under the pointer moves
                            // with it, so it has to be drawn moving with it.
                            drag
                            ? partnerPreview(drag, segment.id)
                            : null;
                      return (
                        <Segment
                          key={segment.id}
                          segment={segment}
                          kind={track.kind}
                          label={segmentLabel(project, segment)}
                          selected={selected.has(segment.id)}
                          locked={track.locked}
                          muted={track.muted || segment.volume <= 0}
                          razor={tool === "razor"}
                          linked={linkGroupOf(project, segment) !== null}
                          soundOnPartnerLane={soundIsOnALinkedLane(project, track, segment)}
                          linkable={selection.length > 1 && selected.has(segment.id)}
                          zoom={zoom}
                          preview={dragging && drag.toTrackId !== track.id ? null : preview}
                          ghosted={Boolean(dragging && drag.toTrackId !== track.id)}
                          laneHeight={trackHeight(track.kind)}
                          material={materials.get(segment.material_id) ?? null}
                          viewport={viewport}
                          onGesture={beginGesture}
                          onSelect={handleSelectSegment}
                          onSplit={handleSplitSegment}
                          onDuplicate={handleDuplicateSegment}
                          onDelete={handleDeleteSegment}
                          onRippleDelete={handleRippleDeleteSegment}
                          onUnlink={handleUnlinkSegment}
                          onLink={handleLink}
                          onFade={handleFadeSegment}
                        />
                      );
                    })}

                    {external && drag?.kind === "move" ? (
                      <Segment
                        key={`${external.segment.id}-ghost`}
                        segment={external.segment}
                        kind={track.kind}
                        label={segmentLabel(project, external.segment)}
                        selected
                        locked={false}
                        muted={track.muted || external.segment.volume <= 0}
                        razor={false}
                        linked={linkGroupOf(project, external.segment) !== null}
                        soundOnPartnerLane={soundIsOnALinkedLane(project, track, external.segment)}
                        linkable={false}
                        zoom={zoom}
                        preview={{ start: drag.start, duration: drag.duration }}
                        ghosted={false}
                        laneHeight={trackHeight(track.kind)}
                        material={materials.get(external.segment.material_id) ?? null}
                        viewport={viewport}
                        onGesture={beginGesture}
                        onSelect={handleSelectSegment}
                        onSplit={handleSplitSegment}
                        onDuplicate={handleDuplicateSegment}
                        onDelete={handleDeleteSegment}
                        onRippleDelete={handleRippleDeleteSegment}
                        onUnlink={handleUnlinkSegment}
                        onLink={handleLink}
                        onFade={handleFadeSegment}
                      />
                    ) : null}

                    {/* Transitions sit above the clips they join: the marker
                        straddles the cut, so it cannot belong to either
                        neighbour's box. Drawn after the segments so its drag
                        handles win the pointer over the clip edges beneath. */}
                    {project ? (
                      <TransitionLane
                        project={project}
                        track={track}
                        zoom={zoom}
                        laneHeight={trackHeight(track.kind)}
                        timeAtClientX={timeAtClientX}
                      />
                    ) : null}

                    <RazorGuide trackId={track.id} zoom={zoom} />
                  </TrackLane>
                );
              })}

              {project && project.tracks.length === 0 ? (
                <p className="px-4 py-8 text-center text-[12px] text-muted-foreground">
                  This project has no tracks.
                </p>
              ) : null}
            </div>

            {/* The rubber band. Drawn over the lanes and under the playhead,
                and never a pointer target itself — the press that started it is
                still being tracked on the window. */}
            {band ? (
              <span
                aria-hidden
                data-slot="selection-band"
                className="pointer-events-none absolute z-20 rounded-[2px] border border-timeline-clip-selected bg-timeline-clip-selected/15"
                style={{
                  left: Math.min(band.fromX, band.toX),
                  top: Math.min(band.fromY, band.toY),
                  width: Math.abs(band.toX - band.fromX),
                  height: Math.abs(band.toY - band.fromY),
                }}
              />
            ) : null}

            {/* Snap guide: without it, magnetism reads as the drag fighting
                back. It marks the candidate, not the clip, so it stays put
                while the pointer keeps moving. */}
            {drag?.snapAt != null ? (
              <span
                aria-hidden
                className="pointer-events-none absolute bottom-0 z-30 w-px bg-timeline-snap"
                style={{ left: drag.snapAt * zoom, top: RULER_HEIGHT }}
              />
            ) : null}

            <Playhead zoom={zoom} onGrab={scrub} />
          </div>
        </div>

        {empty ? (
          <div
            className="pointer-events-none absolute bottom-0 right-0 grid place-items-center px-6 text-center"
            style={{ left: TRACK_HEADER_WIDTH, top: RULER_HEIGHT }}
          >
            <div className="max-w-[320px]">
              <p className="text-[12px] text-muted-foreground">Nothing on the timeline yet</p>
              <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground/70">
                Drag a clip from the media library onto a lane, or drop files straight from your
                file manager.
              </p>
            </div>
          </div>
        ) : null}
      </div>
    </section>
  );
}
