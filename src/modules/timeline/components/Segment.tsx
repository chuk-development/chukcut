import {
  CopyIcon,
  LinkIcon,
  LockIcon,
  ScissorsIcon,
  Trash2Icon,
  Unlink2Icon,
  VolumeXIcon,
} from "lucide-react";
import type React from "react";
import { memo, useEffect } from "react";

import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { formatDuration } from "@/lib/time";
import { cn } from "@/lib/utils";
import { useThumbnailStore } from "@/modules/media/lib/thumbnails";
import type { Id, Micros, Segment as SegmentModel, TrackKind } from "@/modules/project/types";
import { rangeEnd } from "@/modules/project/types";
import { Filmstrip } from "@/modules/timeline/components/Filmstrip";
import { Waveform } from "@/modules/timeline/components/Waveform";

/** Clip colour is keyed by lane kind so a glance tells you what a lane holds. */
const KIND_STYLE: Record<TrackKind, string> = {
  video: "bg-track-video/30 border-track-video/70",
  audio: "bg-track-audio/30 border-track-audio/70",
  text: "bg-track-text/30 border-track-text/70",
  sticker: "bg-track-sticker/30 border-track-sticker/70",
  effect: "bg-track-effect/30 border-track-effect/70",
};

const KIND_BAR: Record<TrackKind, string> = {
  video: "bg-track-video",
  audio: "bg-track-audio",
  text: "bg-track-text",
  sticker: "bg-track-sticker",
  effect: "bg-track-effect",
};

/** The clip's own top and bottom inset, added up: how much shorter it is than its lane. */
const CLIP_INSET = 6;
/** The band the label sits in. The waveform starts below it. */
const LABEL_BAND = 15;
/** The slim waveform along the bottom of a video clip. */
const STRIP_WAVEFORM_HEIGHT = 16;
/** A clip narrower than this has no room for a name that is not a single letter. */
const MIN_LABEL_WIDTH = 34;
/** Under a millisecond of material left is the source's edge — less than a frame at any rate. */
const LIMIT_EPSILON = 1_000;

/**
 * How many times each clip has painted, keyed by segment id.
 *
 * A counter in the component rather than a test double, because the thing worth
 * proving — that dragging one clip does not repaint the other forty-nine — can
 * only be observed from inside `React.memo`. One `Map.set` per paint, which is
 * orders below the cost of the paint it is counting.
 *
 * `Timeline.paint.test.tsx` is the reason it exists.
 */
export const clipPaintCount = new Map<Id, number>();

export type SegmentGesture = "move" | "trim-start" | "trim-end";

/**
 * What a clip needs to know about the file behind it.
 *
 * Handed down as one object, built once per document by the timeline, so that
 * every clip's props stay reference-stable across a pointer move — the whole
 * memoisation below depends on it.
 */
export interface ClipMaterial {
  /** The file the caches are keyed by. Null for material with no file, i.e. text. */
  path: string | null;
  /** Display aspect ratio, for filmstrip tile width. */
  aspect: number;
  /** Full length of the source. Zero when there is none: stills and titles stretch. */
  duration: Micros;
  /** The file carries sound worth drawing. */
  hasAudio: boolean;
  /** The file is nothing but sound, so the waveform is the clip rather than a band on it. */
  audioOnly: boolean;
}

interface SegmentProps {
  segment: SegmentModel;
  kind: TrackKind;
  label: string;
  selected: boolean;
  locked: boolean;
  /** The lane or the clip is silent. */
  muted: boolean;
  /** The razor tool is active: this clip is something to cut, not to drag. */
  razor: boolean;
  /**
   * This clip moves, trims and deletes with another one — normally the sound it
   * was imported with. Worth a badge: the clip is about to behave differently
   * from how it looks, and a drag that moves two clips when the user grabbed
   * one is otherwise indistinguishable from a bug.
   */
  linked: boolean;
  /**
   * Several clips are selected and this is one of them, so "Link" is worth
   * offering. It is a property of the selection rather than of the clip, but
   * the clip is where the context menu is.
   */
  linkable: boolean;
  zoom: number;
  /** Live drag position. Local until the gesture ends; Rust never sees it. */
  preview: { start: Micros; duration: Micros } | null;
  /** True while this clip is being dragged onto a different lane. */
  ghosted: boolean;
  /** Lane height in pixels, which sets how tall a filmstrip tile is. */
  laneHeight: number;
  /** The file behind the clip. Null when the material is not in the pool. */
  material: ClipMaterial | null;
  /** Visible time window, so only the pixels on screen are drawn. */
  viewport: { from: Micros; to: Micros };
  onGesture: (event: React.PointerEvent, gesture: SegmentGesture, segmentId: string) => void;
  /**
   * The event comes along because what a click means depends on which keys are
   * down: plain replaces the selection, Ctrl/Cmd adds or removes this one,
   * Shift takes the run between it and the last one. The clip does not decide
   * any of that — the timeline does, because only it knows what is selected.
   */
  onSelect: (segmentId: string, event: React.PointerEvent) => void;
  onSplit: (segmentId: string) => void;
  onDuplicate: (segmentId: string) => void;
  onDelete: (segmentId: string) => void;
  onUnlink: (segmentId: string) => void;
  onLink: () => void;
}

function ClipBody({
  segment,
  kind,
  label,
  selected,
  locked,
  muted,
  razor,
  linked,
  linkable,
  zoom,
  preview,
  ghosted,
  laneHeight,
  material,
  viewport,
  onGesture,
  onSelect,
  onSplit,
  onDuplicate,
  onDelete,
  onUnlink,
  onLink,
}: SegmentProps) {
  clipPaintCount.set(segment.id, (clipPaintCount.get(segment.id) ?? 0) + 1);

  const start = preview ? preview.start : segment.target_range.start;
  const duration = preview ? preview.duration : segment.target_range.duration;
  const width = Math.max(2, duration * zoom);
  const height = Math.max(1, laneHeight - CLIP_INSET);

  // The slice of this clip that is on screen, in pixels from its left edge.
  // Every layer below draws into that slice and nothing outside it, so a clip
  // ten thousand pixels wide costs what one that fits costs.
  const fromPx = Math.max(0, Math.floor((viewport.from - start) * zoom));
  const toPx = Math.min(width, Math.ceil((viewport.to - start) * zoom));
  const visibleWidth = Math.max(0, toPx - fromPx);
  const onScreen = visibleWidth > 0;

  // Trimmed to the material's own limit: there is nothing left to pull out on
  // that side and the handle will simply refuse to move. Saying so is the
  // difference between a boundary and a bug.
  const hasLimits = material !== null && material.duration > 0;
  const atSourceHead = hasLimits && segment.source_range.start <= LIMIT_EPSILON;
  const atSourceTail =
    hasLimits && material.duration - rangeEnd(segment.source_range) <= LIMIT_EPSILON;

  // The lane decides the layout, not only the material: a video dropped onto an
  // audio lane is there to be heard, so it gets the full waveform rather than a
  // filmstrip with a band under it.
  const soundOnly = kind === "audio" || material?.audioOnly === true;
  const showsFilmstrip = onScreen && material?.path != null && !soundOnly;

  // The clip is what wants a filmstrip, not the filmstrip layer: scrolling past
  // a clip must not cancel a decode that is halfway done, whereas deleting the
  // clip should. Held here, released when the clip goes.
  const stripPath = material?.path && !soundOnly ? material.path : null;
  const retainStrip = useThumbnailStore((state) => state.retain);
  const releaseStrip = useThumbnailStore((state) => state.release);
  useEffect(() => {
    if (!stripPath) return;
    retainStrip(stripPath);
    return () => releaseStrip(stripPath);
  }, [stripPath, retainStrip, releaseStrip]);

  const showsWaveform = onScreen && material?.path != null && material.hasAudio;
  const waveformHeight = soundOnly
    ? Math.max(1, height - LABEL_BAND)
    : Math.min(STRIP_WAVEFORM_HEIGHT, Math.max(0, height - LABEL_BAND));

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          data-slot="segment"
          data-selected={selected || undefined}
          data-locked={locked || undefined}
          data-muted={muted || undefined}
          className={cn(
            "group absolute top-[3px] bottom-[3px] overflow-hidden rounded-[3px] border text-left",
            KIND_STYLE[kind],
            // Selection is a ring *and* a light border: over a filmstrip a
            // border alone disappears into whatever frame happens to be under it.
            selected
              ? "border-timeline-clip-selected ring-1 ring-timeline-clip-selected/80"
              : "hover:border-foreground/40",
            ghosted && "opacity-40",
            preview && "z-10 shadow-lg",
          )}
          style={{ left: start * zoom, width }}
        >
          {/* Filmstrip under everything else. Tiles are backgrounds rather than
              <img> so a cache path that fails to load degrades to the flat clip
              colour instead of a broken-image glyph. */}
          {showsFilmstrip && material?.path ? (
            <Filmstrip
              segment={segment}
              path={material.path}
              aspect={material.aspect}
              materialDuration={material.duration}
              zoom={zoom}
              clipWidth={width}
              fromPx={fromPx}
              visibleWidth={visibleWidth}
              height={height}
            />
          ) : null}

          {showsWaveform && material?.path && waveformHeight > 0 ? (
            <Waveform
              path={material.path}
              materialDuration={material.duration}
              sourceStart={segment.source_range.start}
              speed={segment.speed}
              zoom={zoom}
              fromPx={fromPx}
              widthPx={visibleWidth}
              height={waveformHeight}
              topPx={soundOnly ? LABEL_BAND : height - waveformHeight}
              variant={soundOnly ? "full" : "strip"}
              muted={muted}
            />
          ) : null}

          {/* Hover as a fill rather than a border change: it has to read the
              same over a bright frame as over flat colour, and it costs nothing
              — no state, no render, just CSS. */}
          <span className="pointer-events-none absolute inset-0 bg-timeline-clip-hover opacity-0 transition-opacity group-hover:opacity-100" />

          {/* A locked clip is hatched rather than merely dimmed: dimming is what
              a clip being dragged to another lane does, and the two must not
              look alike. */}
          {locked ? (
            <span
              aria-hidden
              className="pointer-events-none absolute inset-0 opacity-40"
              style={{
                backgroundImage:
                  "repeating-linear-gradient(45deg, var(--timeline-clip-missing) 0 3px, transparent 3px 8px)",
              }}
            />
          ) : null}

          {/* The clip body is the drag surface. It is a sibling of the trim
              handles rather than their parent, because a button inside a button
              is not valid markup. */}
          <button
            type="button"
            aria-label={label}
            aria-pressed={selected}
            title={label}
            onPointerDown={(event) => {
              if (event.button !== 0) return;
              // Under the razor a click is a cut, not a selection: selecting
              // the clip you are about to bisect only muddies what happens next.
              if (razor) {
                if (!locked) onGesture(event, "move", segment.id);
                return;
              }
              onSelect(segment.id, event);
              // A modifier click is about the selection and nothing else.
              // Dragging on the same press would move a clip the user was in
              // the middle of adding to — or had just taken out of — the set.
              if (event.ctrlKey || event.metaKey || event.shiftKey) return;
              if (!locked) onGesture(event, "move", segment.id);
            }}
            className={cn(
              "absolute inset-0 border-none bg-transparent p-0 outline-none",
              locked
                ? "cursor-not-allowed"
                : razor
                  ? "cursor-col-resize"
                  : "cursor-grab active:cursor-grabbing",
            )}
          />

          <span
            className={cn("pointer-events-none absolute inset-x-0 top-0 h-[2px]", KIND_BAR[kind])}
          />

          {/* The scrim under the label is unconditional. A filename is
              unreadable over a bright frame and equally unreadable over a pale
              waveform, and the band is fifteen pixels of a fifty-pixel clip
              either way. */}
          <span
            aria-hidden
            className="pointer-events-none absolute inset-x-0 top-0 bg-gradient-to-b from-timeline-clip-scrim to-transparent"
            style={{ height: LABEL_BAND + 4 }}
          />

          <span className="pointer-events-none absolute inset-x-0 top-0 flex min-w-0 items-start gap-1.5 px-1.5 pt-[5px]">
            {width >= MIN_LABEL_WIDTH ? (
              <span className="truncate text-[11px] font-medium leading-none text-foreground/90">
                {label}
              </span>
            ) : null}
            {width > 110 ? (
              <span className="shrink-0 text-[10px] leading-none text-foreground/45">
                {formatDuration(duration)}
              </span>
            ) : null}
            <span className="ml-auto flex shrink-0 items-center gap-1">
              {linked && width >= 52 ? (
                <LinkIcon
                  className="size-3 text-foreground/70"
                  aria-label="Linked to another clip"
                />
              ) : null}
              {muted && width >= 52 ? (
                <VolumeXIcon className="size-3 text-foreground/70" aria-label="Silent" />
              ) : null}
              {locked && width >= 52 ? (
                <LockIcon className="size-3 text-foreground/70" aria-label="Locked" />
              ) : null}
            </span>
          </span>

          {/* Source limits, drawn on the edge that cannot move so the marker
              travels with the edge it describes. */}
          {atSourceHead && width > 12 ? (
            <span
              data-slot="source-limit"
              data-edge="start"
              title="The start of the source file: this edge cannot be pulled out further"
              className="pointer-events-none absolute inset-y-0 left-0 w-[2px] bg-timeline-clip-limit/80"
            />
          ) : null}
          {atSourceTail && width > 12 ? (
            <span
              data-slot="source-limit"
              data-edge="end"
              title="The end of the source file: this edge cannot be pulled out further"
              className="pointer-events-none absolute inset-y-0 right-0 w-[2px] bg-timeline-clip-limit/80"
            />
          ) : null}

          {/* Trim handles. Wide enough to hit, invisible until the clip
              matters, and gone under the razor — they would otherwise swallow
              the first and last few pixels of cuttable clip. */}
          {locked || razor ? null : (
            <>
              <button
                type="button"
                aria-label="Trim start"
                onPointerDown={(event) => {
                  event.stopPropagation();
                  if (event.button !== 0) return;
                  onSelect(segment.id, event);
                  onGesture(event, "trim-start", segment.id);
                }}
                className="absolute inset-y-0 left-0 z-10 w-2 cursor-ew-resize border-none bg-transparent p-0 opacity-0 transition-opacity group-hover:opacity-100 data-[shown=true]:opacity-100"
                data-shown={selected}
              >
                <span className="mx-auto block h-1/2 w-[3px] translate-y-1/2 rounded-full bg-foreground/70" />
              </button>
              <button
                type="button"
                aria-label="Trim end"
                onPointerDown={(event) => {
                  event.stopPropagation();
                  if (event.button !== 0) return;
                  onSelect(segment.id, event);
                  onGesture(event, "trim-end", segment.id);
                }}
                className="absolute inset-y-0 right-0 z-10 w-2 cursor-ew-resize border-none bg-transparent p-0 opacity-0 transition-opacity group-hover:opacity-100 data-[shown=true]:opacity-100"
                data-shown={selected}
              >
                <span className="mx-auto block h-1/2 w-[3px] translate-y-1/2 rounded-full bg-foreground/70" />
              </button>
            </>
          )}
        </div>
      </ContextMenuTrigger>

      <ContextMenuContent>
        <ContextMenuItem onSelect={() => onSplit(segment.id)}>
          <ScissorsIcon />
          Split at playhead
          <ContextMenuShortcut>C</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuItem onSelect={() => onDuplicate(segment.id)}>
          <CopyIcon />
          Duplicate
        </ContextMenuItem>
        {linked || linkable ? (
          <>
            <ContextMenuSeparator />
            {/* Offered on any multi-selection, including one that is already
                linked in part: linking is how a group is *re*-formed, and
                Rust's `link` takes each clip out of whatever group it was in
                first. */}
            {linkable ? (
              <ContextMenuItem onSelect={onLink}>
                <LinkIcon />
                Link
              </ContextMenuItem>
            ) : null}
            {/* "Unlink" rather than "Detach audio": the pair is usually a clip
                and its own sound, but a group can hold anything the user linked
                by hand, and a menu item that names the wrong thing is worse
                than one that names the general one. */}
            {linked ? (
              <ContextMenuItem onSelect={() => onUnlink(segment.id)}>
                <Unlink2Icon />
                Unlink
              </ContextMenuItem>
            ) : null}
          </>
        ) : null}
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={() => onDelete(segment.id)}>
          <Trash2Icon />
          Delete
          <ContextMenuShortcut>Del</ContextMenuShortcut>
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

/**
 * Memoised, and the memo is load-bearing rather than a precaution.
 *
 * Every pointer move during a drag sets state on the timeline, which re-renders
 * it, which re-creates the element for every clip on it. Without this, the
 * fiftieth clip recomputes its tiles and its waveform columns forty times a
 * second because a clip it has nothing to do with is moving. With it, the props
 * of every clip but the dragged one are reference-identical and the work stops
 * at the comparison — `Timeline.paint.test.tsx` fails if any of the props above
 * stops being stable.
 */
export const Segment = memo(ClipBody);
