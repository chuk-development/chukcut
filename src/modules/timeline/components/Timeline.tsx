import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { MEDIA_DRAG_MIME } from "@/lib/dnd";
import { clamp, MICROS_PER_SECOND } from "@/lib/time";
import { useThumbnailStore } from "@/modules/media/lib/thumbnails";
import { useMediaStore } from "@/modules/media/store";
import { preview } from "@/modules/preview/lib/session";
import { runEdit, useProjectStore } from "@/modules/project/store";
import type { Id, Micros, TimeRange, Track } from "@/modules/project/types";
import {
  findSegment,
  materialAspect,
  materialDuration,
  materialPath,
  projectDuration,
  rangeEnd,
  segmentLabel,
} from "@/modules/project/types";
import { Playhead } from "@/modules/timeline/components/Playhead";
import {
  type Filmstrip,
  Segment,
  type SegmentGesture,
} from "@/modules/timeline/components/Segment";
import { TimelineToolbar } from "@/modules/timeline/components/TimelineToolbar";
import { TimeRuler } from "@/modules/timeline/components/TimeRuler";
import { TrackHeader, TrackLane } from "@/modules/timeline/components/Track";
import { timelineApply } from "@/modules/timeline/lib/api";
import { registerTimelineDropTarget } from "@/modules/timeline/lib/dropTarget";
import {
  deleteSegment,
  duplicateSegment,
  insertMaterial,
  minClipDuration,
  redo,
  segmentUnderPlayhead,
  splitAt,
  toggleTrackFlag,
  undo,
} from "@/modules/timeline/lib/edits";
import { freeSpan, nearestFreeStart } from "@/modules/timeline/lib/placement";
import {
  buildSnapContext,
  type SnapContext,
  snapInstant,
  snapRadius,
  snapRange,
} from "@/modules/timeline/lib/snapping";
import {
  MAX_ZOOM,
  MIN_ZOOM,
  RULER_HEIGHT,
  TRACK_HEADER_WIDTH,
  trackHeight,
  useTimelineStore,
} from "@/modules/timeline/store";

/** Empty timeline still shows this much time, so the ruler is never a stub. */
const MIN_VISIBLE_SPAN = 20 * MICROS_PER_SECOND;
/** Room past the last clip to drag into. */
const TRAILING_PX = 480;

type DragState =
  | {
      kind: "move";
      segmentId: Id;
      fromTrackId: Id;
      toTrackId: Id;
      originStart: Micros;
      start: Micros;
      duration: Micros;
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
      snapAt: Micros | null;
    }
  | null;

/** What the razor would cut, given where the pointer is. */
interface RazorTarget {
  trackId: Id;
  segmentId: Id;
  at: Micros;
}

export function Timeline() {
  const project = useProjectStore((s) => s.project);
  const canUndo = useProjectStore((s) => s.canUndo);
  const canRedo = useProjectStore((s) => s.canRedo);
  const setError = useProjectStore((s) => s.setError);

  const zoom = useTimelineStore((s) => s.zoom);
  const scrollX = useTimelineStore((s) => s.scrollX);
  const playhead = useTimelineStore((s) => s.playhead);
  const selectedSegmentId = useTimelineStore((s) => s.selectedSegmentId);
  const tool = useTimelineStore((s) => s.tool);
  const snapping = useTimelineStore((s) => s.snapping);
  const setZoom = useTimelineStore((s) => s.setZoom);
  const setScrollX = useTimelineStore((s) => s.setScrollX);
  const setPlayhead = useTimelineStore((s) => s.setPlayhead);
  const select = useTimelineStore((s) => s.select);
  const setTool = useTimelineStore((s) => s.setTool);
  const toggleSnapping = useTimelineStore((s) => s.toggleSnapping);

  const scrollRef = useRef<HTMLDivElement>(null);
  const headerScrollRef = useRef<HTMLDivElement>(null);
  const lanesRef = useRef(new Map<Id, HTMLDivElement>());
  const pendingScrollRef = useRef<number | null>(null);
  const [viewportWidth, setViewportWidth] = useState(0);
  const [drag, setDrag] = useState<DragState>(null);
  const [razorTarget, setRazorTarget] = useState<RazorTarget | null>(null);

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
    [razorTargetAt],
  );

  // The cut line is a property of the razor, so it goes away with the tool.
  useEffect(() => {
    if (tool !== "razor") setRazorTarget(null);
  }, [tool]);

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
      // How far the trim handles can travel before they overlap a neighbour.
      const room = freeSpan(track.segments, segmentId, segment.target_range);

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

          const laneId = laneAtClientY(moveEvent.clientY);
          const lane = current.tracks.find((t) => t.id === laneId && !t.locked) ?? track;
          // A drag never ends in "target range is occupied": if the drop lands
          // on material, it butts up against the side it was dropped on.
          const landed = nearestFreeStart(lane.segments, start, duration, segmentId);

          live = {
            kind: "move",
            segmentId,
            fromTrackId: track.id,
            toTrackId: lane.id,
            originStart: segment.target_range.start,
            start: landed,
            duration,
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
          // Cannot swallow the whole clip, and cannot run off the head of the source.
          delta = Math.min(delta, segment.target_range.duration - minDuration);
          delta = Math.max(delta, -Math.floor(segment.source_range.start / speed));
          delta = Math.max(delta, room.min - originalStart);
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
            // Only claim a snap the clamps did not take back.
            snapAt: hit && hit.at === start ? hit.at : null,
          };
        } else {
          const originalEnd = rangeEnd(segment.target_range);
          const wanted = originalEnd + deltaMicros;
          const hit = magnetic ? snapInstant(wanted, context, radius) : null;
          let delta = Math.round((hit?.value ?? wanted) - originalEnd);
          delta = Math.max(delta, minDuration - segment.target_range.duration);
          if (sourceLimit !== null) {
            const spare = sourceLimit - rangeEnd(segment.source_range);
            delta = Math.min(delta, Math.floor(spare / speed));
          }
          delta = Math.min(delta, room.max - originalEnd);
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

        if (final.kind === "move") {
          const moved = final.start !== final.originStart || final.toTrackId !== final.fromTrackId;
          if (!moved) return;
          void runEdit(() =>
            timelineApply({
              type: "move_segment",
              segment_id: final.segmentId,
              from_track: final.fromTrackId,
              to_track: final.toTrackId,
              from_start: final.originStart,
              to_start: final.start,
            }),
          );
          return;
        }

        const unchanged =
          final.afterTarget.start === final.beforeTarget.start &&
          final.afterTarget.duration === final.beforeTarget.duration;
        if (unchanged) return;
        void runEdit(() =>
          timelineApply({
            type: "trim_segment",
            segment_id: final.segmentId,
            before_target: final.beforeTarget,
            before_source: final.beforeSource,
            after_target: final.afterTarget,
            after_source: final.afterSource,
          }),
        );
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
    const { playhead: at, selectedSegmentId: preferred } = useTimelineStore.getState();
    const hit = segmentUnderPlayhead(current, at, preferred);
    if (!hit) {
      setError("Nothing to split: put the playhead over a clip first.");
      return;
    }
    void splitAt(hit.segmentId, at);
  }, [setError]);

  const handleDelete = useCallback(() => {
    const current = useProjectStore.getState().project;
    const id = useTimelineStore.getState().selectedSegmentId;
    if (!current || !id) return;
    select(null);
    void deleteSegment(current, id);
  }, [select]);

  const handleDuplicate = useCallback(() => {
    const current = useProjectStore.getState().project;
    const id = useTimelineStore.getState().selectedSegmentId;
    if (!current || !id) return;
    void duplicateSegment(current, id);
  }, []);

  const handleToggleFlag = useCallback((track: Track, flag: "muted" | "locked" | "hidden") => {
    void toggleTrackFlag(track, flag);
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
      if (event.ctrlKey || event.metaKey || event.altKey) return;

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
  }, [handleDelete, handleSplit, select, setTool, toggleSnapping]);

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
  // Filmstrips
  // ---------------------------------------------------------------------

  const strips = useThumbnailStore((s) => s.strips);
  const requestStrip = useThumbnailStore((s) => s.request);

  // Off the render path deliberately: media_thumbnails blocks Rust while it
  // decodes, so asking for a strip is a side effect, never something a render
  // triggers synchronously.
  useEffect(() => {
    if (!project) return;
    const wanted = new Set<string>();
    for (const track of project.tracks) {
      if (track.kind === "audio") continue;
      for (const segment of track.segments) {
        const path = materialPath(project, segment.material_id);
        if (path) wanted.add(path);
      }
    }
    for (const path of wanted) requestStrip(path);
  }, [project, requestStrip]);

  const filmstripFor = useCallback(
    (materialId: Id): Filmstrip | null => {
      if (!project) return null;
      const path = materialPath(project, materialId);
      if (!path) return null;
      const urls = strips[path];
      if (!urls || urls.length === 0) return null;
      return {
        urls,
        aspect: materialAspect(project, materialId) ?? 16 / 9,
        materialDuration: materialDuration(project, materialId) ?? 0,
      };
    },
    [project, strips],
  );

  /** The time window on screen, so clips only build the tiles that are visible. */
  const viewport = useMemo(
    () => ({
      from: scrollX / zoom,
      to: (scrollX + Math.max(viewportWidth, 600)) / zoom,
    }),
    [scrollX, zoom, viewportWidth],
  );

  // ---------------------------------------------------------------------
  // Render
  // ---------------------------------------------------------------------

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
        playhead={playhead}
        duration={duration}
        fps={fps}
        zoom={zoom}
        tool={tool}
        snapping={snapping}
        canUndo={canUndo}
        canRedo={canRedo}
        hasSelection={selectedSegmentId !== null}
        onZoom={setZoom}
        onZoomToFit={zoomToFit}
        onSetTool={setTool}
        onToggleSnapping={toggleSnapping}
        onUndo={() => void undo()}
        onRedo={() => void redo()}
        onSplit={handleSplit}
        onDuplicate={handleDuplicate}
        onDelete={handleDelete}
      />

      <div className="flex min-h-0 flex-1">
        <div
          className="flex shrink-0 flex-col border-r border-border"
          style={{ width: TRACK_HEADER_WIDTH }}
        >
          <div
            className="shrink-0 border-b border-border bg-timeline-ruler"
            style={{ height: RULER_HEIGHT }}
          />
          <div ref={headerScrollRef} className="flex-1 overflow-hidden">
            {project?.tracks.map((track) => (
              <TrackHeader key={track.id} track={track} onToggle={handleToggleFlag} />
            ))}
          </div>
        </div>

        {/* biome-ignore lint/a11y/noStaticElementInteractions: a drop surface is not a control — there is no keyboard equivalent of "drop here", and the lane buttons inside it remain focusable */}
        <div
          ref={scrollRef}
          onScroll={handleScroll}
          onDragOver={handleDragOver}
          onDrop={handleLibraryDrop}
          onPointerMove={trackRazor}
          onPointerLeave={() => setRazorTarget(null)}
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

            {/* Scrub band: the whole ruler strip is a click target. */}
            <button
              type="button"
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
                    {/* Empty lane space: clicking it deselects and moves the
                        playhead — except under the razor, where a click on a
                        gap is a cut that has nothing to cut. */}
                    <button
                      type="button"
                      tabIndex={-1}
                      aria-label={`${track.name} lane`}
                      className="absolute inset-0 cursor-default border-none bg-transparent p-0 outline-none"
                      onPointerDown={(event) => {
                        if (tool === "razor") return;
                        select(null);
                        scrub(event);
                      }}
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
                          : null;
                      return (
                        <Segment
                          key={segment.id}
                          segment={segment}
                          kind={track.kind}
                          label={segmentLabel(project, segment)}
                          selected={selectedSegmentId === segment.id}
                          locked={track.locked}
                          razor={tool === "razor"}
                          zoom={zoom}
                          preview={dragging && drag.toTrackId !== track.id ? null : preview}
                          ghosted={Boolean(dragging && drag.toTrackId !== track.id)}
                          laneHeight={trackHeight(track.kind)}
                          filmstrip={filmstripFor(segment.material_id)}
                          viewport={viewport}
                          onGesture={beginGesture}
                          onSelect={select}
                          onSplit={(id) => void splitAt(id, playhead)}
                          onDuplicate={handleDuplicate}
                          onDelete={(id) => {
                            select(null);
                            void deleteSegment(project, id);
                          }}
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
                        razor={false}
                        zoom={zoom}
                        preview={{ start: drag.start, duration: drag.duration }}
                        ghosted={false}
                        laneHeight={trackHeight(track.kind)}
                        filmstrip={filmstripFor(external.segment.material_id)}
                        viewport={viewport}
                        onGesture={beginGesture}
                        onSelect={select}
                        onSplit={(id) => void splitAt(id, playhead)}
                        onDuplicate={handleDuplicate}
                        onDelete={(id) => void deleteSegment(project, id)}
                      />
                    ) : null}

                    {/* The razor's cut line: exactly where a click would land,
                        snapped the same way a drag is. */}
                    {razorTarget?.trackId === track.id ? (
                      <span
                        aria-hidden
                        className="pointer-events-none absolute inset-y-0 z-20 w-px bg-timeline-razor"
                        style={{ left: razorTarget.at * zoom }}
                      />
                    ) : null}
                  </TrackLane>
                );
              })}

              {project && project.tracks.length === 0 ? (
                <p className="px-4 py-8 text-center text-[12px] text-muted-foreground">
                  This project has no tracks.
                </p>
              ) : null}
            </div>

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

            <Playhead time={playhead} zoom={zoom} onGrab={scrub} />
          </div>
        </div>
      </div>
    </section>
  );
}
