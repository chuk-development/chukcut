import {
  ClipboardPasteIcon,
  CopyIcon,
  GaugeIcon,
  ImageOffIcon,
  LinkIcon,
  LockIcon,
  PencilIcon,
  ScissorsIcon,
  SplitIcon,
  Trash2Icon,
  Unlink2Icon,
  Volume2Icon,
  VolumeXIcon,
} from "lucide-react";
import type React from "react";
import { memo, useEffect, useMemo, useState } from "react";

import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { formatDuration } from "@/lib/time";
import { cn } from "@/lib/utils";
import { useThumbnailStore } from "@/modules/media/lib/thumbnails";
import type { Id, Micros, Segment as SegmentModel, TrackKind } from "@/modules/project/types";
import { rangeEnd } from "@/modules/project/types";
import { Filmstrip } from "@/modules/timeline/components/Filmstrip";
import { Waveform } from "@/modules/timeline/components/Waveform";
import { clampFades, type Fades, fadesOf } from "@/modules/timeline/lib/fades";

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
  /**
   * The file is gone from disk. The clip draws in the missing state — the
   * same state a `material` of null (removed from the pool) draws, because to
   * the user both mean "this clip has no media right now".
   */
  missing: boolean;
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
   * This clip's sound lives on a linked audio lane, so it is heard from there
   * and drawn there. Suppresses the waveform on this one; see
   * `soundIsOnALinkedLane`, whose Rust twin decides the matching mixer rule.
   */
  soundOnPartnerLane: boolean;
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
  /** Delete this clip and pull everything later on its lane left by its duration. */
  onRippleDelete: (segmentId: string) => void;
  onUnlink: (segmentId: string) => void;
  onLink: () => void;
  /** A fade handle was released: write these fades as volume keyframes. */
  onFade: (segmentId: string, fades: Fades) => void;
  /** The user-given clip name, when one is set; the label falls back to it already. */
  name: string | null;
  /** This clip's sound can be split onto a linked audio lane. */
  canDetachAudio: boolean;
  /** There is a copied clip whose attributes could be pasted here. */
  canPasteAttributes: boolean;
  /** The rename field is open on this clip. */
  renaming: boolean;
  onSetSpeed: (segmentId: string, speed: number) => void;
  /** Speed → Custom…: focus the inspector's slider. */
  onCustomSpeed: (segmentId: string) => void;
  onToggleMute: (segmentId: string) => void;
  onRenameStart: (segmentId: string) => void;
  /** Null clears the name; the empty string is treated the same. */
  onRenameCommit: (segmentId: string, name: string | null) => void;
  onRenameCancel: () => void;
  onDetachAudio: (segmentId: string) => void;
  onReattachAudio: (segmentId: string) => void;
  onPasteAttributes: (segmentId: string) => void;
}

/** The submenu's presets, in menu order. */
const SPEED_PRESETS = [0.5, 1, 1.5, 2] as const;

/**
 * The inline rename input, resolved exactly once.
 *
 * Enter commits, Escape cancels, and clicking elsewhere commits — but the
 * commit unmounts the field, which fires its blur, so without the one-shot
 * guard every Enter would commit twice and the second, now-stale edit would
 * come back refused in the error bar.
 */
function RenameField({
  defaultValue,
  placeholder,
  onCommit,
  onCancel,
}: {
  defaultValue: string;
  placeholder: string;
  onCommit: (value: string) => void;
  onCancel: () => void;
}) {
  const [done, setDone] = useState(false);
  const finish = (value: string | null) => {
    if (done) return;
    setDone(true);
    if (value === null) onCancel();
    else onCommit(value);
  };

  return (
    <input
      // biome-ignore lint/a11y/noAutofocus: the field exists because the user just asked to type into it
      autoFocus
      type="text"
      aria-label="Clip name"
      defaultValue={defaultValue}
      placeholder={placeholder}
      data-slot="clip-rename"
      onPointerDown={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          finish((event.target as HTMLInputElement).value);
        } else if (event.key === "Escape") {
          event.preventDefault();
          finish(null);
        }
      }}
      onBlur={(event) => finish(event.target.value)}
      className="absolute left-1 top-[2px] z-30 h-[14px] w-[calc(100%-8px)] max-w-[220px] rounded-[2px] border border-primary/60 bg-background/95 px-1 text-[11px] text-foreground outline-none"
    />
  );
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
  soundOnPartnerLane,
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
  onRippleDelete,
  onUnlink,
  onLink,
  onFade,
  name,
  canDetachAudio,
  canPasteAttributes,
  renaming,
  onSetSpeed,
  onCustomSpeed,
  onToggleMute,
  onRenameStart,
  onRenameCommit,
  onRenameCancel,
  onDetachAudio,
  onReattachAudio,
  onPasteAttributes,
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

  // The clip's media is gone — its material was removed from the pool
  // (`material` is null; every pool entry, text included, has an index row),
  // or the file behind it is gone from disk. The clip itself stays: it draws
  // in an unmistakable offline state instead of pretending to be playable.
  const missing = material === null || material.missing;

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
  const showsFilmstrip = onScreen && material?.path != null && !soundOnly && !missing;

  // The clip is what wants a filmstrip, not the filmstrip layer: scrolling past
  // a clip must not cancel a decode that is halfway done, whereas deleting the
  // clip should. Held here, released when the clip goes.
  const stripPath = material?.path && !soundOnly && !missing ? material.path : null;
  const retainStrip = useThumbnailStore((state) => state.retain);
  const releaseStrip = useThumbnailStore((state) => state.release);
  useEffect(() => {
    if (!stripPath) return;
    retainStrip(stripPath);
    return () => releaseStrip(stripPath);
  }, [stripPath, retainStrip, releaseStrip]);

  // A clip whose sound was split onto its own linked lane must not draw the
  // waveform too — the audio clip below is already drawing exactly it, and two
  // copies of one waveform read as two pieces of audio.
  const showsWaveform =
    onScreen && material?.path != null && material.hasAudio && !soundOnPartnerLane && !missing;
  const waveformHeight = soundOnly
    ? Math.max(1, height - LABEL_BAND)
    : Math.min(STRIP_WAVEFORM_HEIGHT, Math.max(0, height - LABEL_BAND));

  // -----------------------------------------------------------------------
  // Audio fades
  //
  // Two drag handles at the clip's top corners; dragging inward writes volume
  // keyframes on release (`fades.ts`). Only a clip that is *heard from here*
  // gets them — a video clip whose sound lives on its linked audio lane fades
  // there, next to the waveform, like everything else about its sound.
  // -----------------------------------------------------------------------
  const carriesSound = material?.hasAudio === true && !soundOnPartnerLane && !missing;
  const fades = useMemo(() => fadesOf(segment), [segment]);
  /** Live handle position while a fade is being dragged. Never reaches Rust. */
  const [fadeDrag, setFadeDrag] = useState<Fades | null>(null);
  const shownFades = fadeDrag ?? fades;
  const duration0 = segment.target_range.duration;
  const showsFades = carriesSound && onScreen && !ghosted && !preview;
  const showsFadeHandles = showsFades && !locked && !razor && width >= 24;

  const beginFadeDrag = (event: React.PointerEvent, edge: "in" | "out") => {
    if (event.button !== 0) return;
    event.stopPropagation();
    event.preventDefault();
    const startX = event.clientX;
    const base = fades;
    let live = base;

    const onMove = (moveEvent: PointerEvent) => {
      // A fade-in grows rightward, a fade-out leftward: same axis, mirrored.
      const delta = (moveEvent.clientX - startX) / zoom;
      live = clampFades(
        duration0,
        edge === "in"
          ? { fadeIn: base.fadeIn + delta, fadeOut: base.fadeOut }
          : { fadeIn: base.fadeIn, fadeOut: base.fadeOut - delta },
      );
      setFadeDrag(live);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      setFadeDrag(null);
      if (live.fadeIn !== base.fadeIn || live.fadeOut !== base.fadeOut) {
        onFade(segment.id, live);
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp, { once: true });
  };

  const fadeInPx = Math.min(shownFades.fadeIn * zoom, width);
  const fadeOutPx = Math.min(shownFades.fadeOut * zoom, width);

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          data-slot="segment"
          data-selected={selected || undefined}
          data-locked={locked || undefined}
          data-muted={muted || undefined}
          data-missing={missing || undefined}
          className={cn(
            "group absolute top-[3px] bottom-[3px] overflow-hidden rounded-[3px] border text-left",
            KIND_STYLE[kind],
            // An offline clip overrides the lane colour outright: the red has
            // to read at a glance as "no media here", not as another lane
            // kind. Selection still wins the border below.
            missing && "border-destructive/80 bg-destructive/25",
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

          {/* Fade ramps, drawn as the wedge of clip the fade silences: a
              triangle from the corner down to where the ramp reaches full
              volume. Over the waveform, under the label, and never a pointer
              target — the handles below are the control. */}
          {showsFades && fadeInPx > 0 ? (
            <span
              aria-hidden
              data-slot="fade"
              data-edge="in"
              className="pointer-events-none absolute inset-y-0 left-0 bg-timeline-clip-scrim/60"
              style={{
                width: fadeInPx,
                clipPath: "polygon(0 100%, 0 0, 100% 0)",
              }}
            />
          ) : null}
          {showsFades && fadeOutPx > 0 ? (
            <span
              aria-hidden
              data-slot="fade"
              data-edge="out"
              className="pointer-events-none absolute inset-y-0 right-0 bg-timeline-clip-scrim/60"
              style={{
                width: fadeOutPx,
                clipPath: "polygon(0 0, 100% 0, 100% 100%)",
              }}
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
            className={cn(
              "pointer-events-none absolute inset-x-0 top-0 h-[2px]",
              missing ? "bg-destructive" : KIND_BAR[kind],
            )}
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
            {/* The offline badge sits in front of the label and survives any
                clip width: a red sliver with no explanation is a paint bug,
                a red sliver with this icon is a diagnosis. */}
            {missing ? (
              <ImageOffIcon
                className="size-3 shrink-0 text-destructive"
                aria-label="Media offline"
              />
            ) : null}
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

          {/* The rename field, over the label it replaces. A pointer press in
              it must not start a drag, and the timeline's key handler already
              ignores keys whose target is an input. */}
          {renaming ? (
            <RenameField
              defaultValue={name ?? ""}
              placeholder={label}
              onCommit={(value) => onRenameCommit(segment.id, value)}
              onCancel={onRenameCancel}
            />
          ) : null}

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

          {/* Fade handles: the CapCut/Premiere idiom, one at each top corner,
              sitting where its ramp ends so the handle *is* the fade length.
              Above the trim handles at the corners — a fade of zero puts them
              in the same place, and the top sliver belongs to the fade. */}
          {showsFadeHandles ? (
            <>
              <button
                type="button"
                aria-label="Fade in"
                title="Drag to fade the sound in"
                data-slot="fade-handle"
                data-edge="in"
                onPointerDown={(event) => beginFadeDrag(event, "in")}
                className="absolute top-[1px] z-20 size-3 cursor-ew-resize border-none bg-transparent p-0 opacity-0 outline-none transition-opacity group-hover:opacity-100 data-[shown=true]:opacity-100"
                data-shown={selected || shownFades.fadeIn > 0}
                style={{ left: Math.max(0, Math.min(fadeInPx - 6, width - 12)) }}
              >
                <span className="mx-auto block size-2 rounded-full border border-background/60 bg-foreground/90" />
              </button>
              <button
                type="button"
                aria-label="Fade out"
                title="Drag to fade the sound out"
                data-slot="fade-handle"
                data-edge="out"
                onPointerDown={(event) => beginFadeDrag(event, "out")}
                className="absolute top-[1px] z-20 size-3 cursor-ew-resize border-none bg-transparent p-0 opacity-0 outline-none transition-opacity group-hover:opacity-100 data-[shown=true]:opacity-100"
                data-shown={selected || shownFades.fadeOut > 0}
                style={{ right: Math.max(0, Math.min(fadeOutPx - 6, width - 12)) }}
              >
                <span className="mx-auto block size-2 rounded-full border border-background/60 bg-foreground/90" />
              </button>
            </>
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
        <ContextMenuSeparator />
        <ContextMenuSub>
          <ContextMenuSubTrigger>
            <GaugeIcon />
            Speed
          </ContextMenuSubTrigger>
          <ContextMenuSubContent>
            {SPEED_PRESETS.map((preset) => (
              <ContextMenuItem key={preset} onSelect={() => onSetSpeed(segment.id, preset)}>
                {`${preset}×`}
                {segment.speed === preset ? (
                  <span aria-hidden className="ml-auto text-[10px]" title="Current speed">
                    •
                  </span>
                ) : null}
              </ContextMenuItem>
            ))}
            <ContextMenuSeparator />
            <ContextMenuItem onSelect={() => onCustomSpeed(segment.id)}>Custom…</ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        {/* The clip's own volume, not the lane switch: the `before` on the
            command is what makes undo restore the old level. */}
        <ContextMenuItem onSelect={() => onToggleMute(segment.id)}>
          {segment.volume <= 0 ? <Volume2Icon /> : <VolumeXIcon />}
          {segment.volume <= 0 ? "Unmute clip" : "Mute clip"}
        </ContextMenuItem>
        <ContextMenuItem onSelect={() => onRenameStart(segment.id)}>
          <PencilIcon />
          Rename clip
        </ContextMenuItem>
        <ContextMenuItem
          disabled={!canPasteAttributes}
          onSelect={() => onPasteAttributes(segment.id)}
        >
          <ClipboardPasteIcon />
          Paste attributes
        </ContextMenuItem>
        {canDetachAudio ? (
          <ContextMenuItem onSelect={() => onDetachAudio(segment.id)}>
            <SplitIcon />
            Detach audio
          </ContextMenuItem>
        ) : null}
        {/* Only when the sound really is a linked clip of this file on an
            audio lane — the shape "Detach audio" creates. Anything looser and
            "Re-attach" would delete a clip it cannot substitute for. */}
        {soundOnPartnerLane ? (
          <ContextMenuItem onSelect={() => onReattachAudio(segment.id)}>
            <Volume2Icon />
            Re-attach audio
          </ContextMenuItem>
        ) : null}
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
        {/* Delete plus close-the-hole, in one undo step. Linked partners come
            along on the Rust side, so what the menu promises is what happens
            on both lanes. */}
        <ContextMenuItem onSelect={() => onRippleDelete(segment.id)}>
          <Trash2Icon />
          Ripple delete
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
