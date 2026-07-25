import { CopyIcon, ScissorsIcon, Trash2Icon } from "lucide-react";
import type React from "react";

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
import { frameAt } from "@/modules/media/lib/thumbnails";
import type { Micros, Segment as SegmentModel, TrackKind } from "@/modules/project/types";

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

/** Height of the strip inside a lane, after the clip's own padding and title bar. */
const TILE_INSET = 6;
const MIN_TILE_WIDTH = 20;

interface Tile {
  key: number;
  left: number;
  width: number;
  url: string;
}

/**
 * Lay frames across a clip body.
 *
 * Only tiles intersecting the visible window are produced. A ten-minute clip at
 * frame-level zoom is a hundred thousand pixels wide, and putting a div per
 * tile in the DOM for all of it would cost more than the decode did.
 */
function filmstripTiles(
  segment: SegmentModel,
  filmstrip: Filmstrip,
  start: Micros,
  duration: Micros,
  zoom: number,
  laneHeight: number,
  viewport: { from: Micros; to: Micros },
): Tile[] {
  if (filmstrip.urls.length === 0 || duration <= 0) return [];

  const width = duration * zoom;
  const tileHeight = Math.max(1, laneHeight - TILE_INSET);
  const tileWidth = Math.max(MIN_TILE_WIDTH, Math.round(tileHeight * filmstrip.aspect));
  const count = Math.ceil(width / tileWidth);

  // Clamp to what is on screen, in tile indices.
  const firstVisible = Math.max(0, Math.floor(((viewport.from - start) * zoom) / tileWidth));
  const lastVisible = Math.min(count - 1, Math.ceil(((viewport.to - start) * zoom) / tileWidth));
  if (lastVisible < firstVisible) return [];

  // Speed makes the source advance faster than the timeline does.
  const speed = segment.speed > 0 ? segment.speed : 1;

  const tiles: Tile[] = [];
  for (let index = firstVisible; index <= lastVisible; index++) {
    const left = index * tileWidth;
    const sourceTime = segment.source_range.start + (left / zoom) * speed;
    tiles.push({
      key: index,
      left,
      width: Math.min(tileWidth, width - left),
      url: frameAt(filmstrip.urls, sourceTime, filmstrip.materialDuration),
    });
  }
  return tiles;
}

export type SegmentGesture = "move" | "trim-start" | "trim-end";

/**
 * Everything needed to tile a clip body with frames from its source.
 *
 * The strip spans the whole material and is indexed by source time, so a clip
 * that has been trimmed or split shows the frames it actually contains rather
 * than restarting from the file's first frame.
 */
export interface Filmstrip {
  urls: string[];
  /** Display aspect ratio, for tile width. */
  aspect: number;
  /** Length of the whole source file, which the strip spans. */
  materialDuration: Micros;
}

interface SegmentProps {
  segment: SegmentModel;
  kind: TrackKind;
  label: string;
  selected: boolean;
  locked: boolean;
  /** The razor tool is active: this clip is something to cut, not to drag. */
  razor: boolean;
  zoom: number;
  /** Live drag position. Local until the gesture ends; Rust never sees it. */
  preview: { start: Micros; duration: Micros } | null;
  /** True while this clip is being dragged onto a different lane. */
  ghosted: boolean;
  /** Lane height in pixels, which sets how tall a filmstrip tile is. */
  laneHeight: number;
  /** Absent until the strip has been decoded; the clip shows flat colour meanwhile. */
  filmstrip: Filmstrip | null;
  /** Visible time window, so only the tiles on screen are put in the DOM. */
  viewport: { from: Micros; to: Micros };
  onGesture: (event: React.PointerEvent, gesture: SegmentGesture, segmentId: string) => void;
  onSelect: (segmentId: string) => void;
  onSplit: (segmentId: string) => void;
  onDuplicate: (segmentId: string) => void;
  onDelete: (segmentId: string) => void;
}

export function Segment({
  segment,
  kind,
  label,
  selected,
  locked,
  razor,
  zoom,
  preview,
  ghosted,
  laneHeight,
  filmstrip,
  viewport,
  onGesture,
  onSelect,
  onSplit,
  onDuplicate,
  onDelete,
}: SegmentProps) {
  const start = preview ? preview.start : segment.target_range.start;
  const duration = preview ? preview.duration : segment.target_range.duration;
  const width = Math.max(2, duration * zoom);
  const tiles = filmstrip
    ? filmstripTiles(segment, filmstrip, start, duration, zoom, laneHeight, viewport)
    : [];

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          data-slot="segment"
          className={cn(
            "group absolute top-[3px] bottom-[3px] overflow-hidden rounded-[3px] border text-left",
            KIND_STYLE[kind],
            selected && "border-foreground ring-1 ring-foreground/70",
            ghosted && "opacity-40",
            preview && "z-10 shadow-lg",
          )}
          style={{ left: start * zoom, width }}
        >
          {/* Filmstrip under everything else. Tiles are backgrounds rather than
              <img> so a cache path that fails to load degrades to the flat clip
              colour instead of a broken-image glyph. */}
          {tiles.length > 0 ? (
            <span className="pointer-events-none absolute inset-x-0 bottom-0 top-[2px] overflow-hidden">
              {tiles.map((tile) => (
                <span
                  key={tile.key}
                  className="absolute inset-y-0 bg-cover bg-center"
                  style={{
                    left: tile.left,
                    width: tile.width,
                    backgroundImage: `url("${tile.url}")`,
                  }}
                />
              ))}
              <span className="absolute inset-x-0 top-0 h-[17px] bg-gradient-to-b from-black/70 to-transparent" />
            </span>
          ) : null}

          {/* The clip body is the drag surface. It is a sibling of the trim
              handles rather than their parent, because a button inside a button
              is not valid markup. */}
          <button
            type="button"
            aria-label={label}
            aria-pressed={selected}
            onPointerDown={(event) => {
              if (event.button !== 0) return;
              // Under the razor a click is a cut, not a selection: selecting
              // the clip you are about to bisect only muddies what happens next.
              if (razor) {
                if (!locked) onGesture(event, "move", segment.id);
                return;
              }
              onSelect(segment.id);
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

          <span className="pointer-events-none flex h-full min-w-0 items-start gap-1.5 px-1.5 pt-[5px]">
            <span className="truncate text-[11px] font-medium leading-none text-foreground/90">
              {label}
            </span>
            {width > 110 ? (
              <span className="shrink-0 text-[10px] leading-none text-foreground/45">
                {formatDuration(duration)}
              </span>
            ) : null}
          </span>

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
                  onSelect(segment.id);
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
                  onSelect(segment.id);
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
        <ContextMenuItem onSelect={() => onDelete(segment.id)}>
          <Trash2Icon />
          Delete
          <ContextMenuShortcut>Del</ContextMenuShortcut>
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
