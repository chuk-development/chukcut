/**
 * The frames laid across a clip body.
 *
 * The strip spans the whole material and is indexed by source time, so a clip
 * that has been trimmed or split shows the frames it actually contains rather
 * than restarting from the file's first frame.
 *
 * The state that matters here is the one in between: tiles stream in, so at any
 * moment some of them exist and some do not. A slot with no tile yet draws a
 * placeholder in the clip's own colour instead of a hole, because a clip with
 * gaps in it reads as the editor being broken, and a clip that stays flat until
 * the last tile lands reads as nothing happening at all.
 */

import { memo } from "react";

import { cn } from "@/lib/utils";
import { frameAt, useThumbnailStore } from "@/modules/media/lib/thumbnails";
import type { Micros, Segment as SegmentModel } from "@/modules/project/types";

/** A tile narrower than this is not a frame any more, it is a stripe. */
const MIN_TILE_WIDTH = 20;

export interface Tile {
  key: number;
  /** Pixels from the clip's left edge. */
  left: number;
  width: number;
  /** Empty when the tile for this position has not decoded yet. */
  url: string;
}

export interface TileOptions {
  /** The strip as it stands, holes and all. */
  tiles: readonly (string | null)[];
  /** Display aspect ratio, which sets tile width. */
  aspect: number;
  /** Length of the whole source file, which the strip spans. */
  materialDuration: Micros;
  sourceStart: Micros;
  speed: number;
  /** Pixels per timeline microsecond. */
  zoom: number;
  /** Full width of the clip in pixels. */
  clipWidth: number;
  /** First visible pixel, from the clip's left edge. */
  fromPx: number;
  visibleWidth: number;
  /** Height available for a tile. */
  height: number;
}

/**
 * Lay frames across a clip body.
 *
 * Only tiles intersecting the visible window are produced. A ten-minute clip at
 * frame-level zoom is a hundred thousand pixels wide, and putting a div per
 * tile in the DOM for all of it would cost more than the decode did.
 */
export function filmstripTiles(options: TileOptions): Tile[] {
  const { clipWidth, height, zoom, sourceStart, materialDuration } = options;
  if (clipWidth <= 0 || height <= 0) return [];

  const tileWidth = Math.max(MIN_TILE_WIDTH, Math.round(height * (options.aspect || 16 / 9)));
  const count = Math.ceil(clipWidth / tileWidth);
  const speed = options.speed > 0 ? options.speed : 1;

  const first = Math.max(0, Math.floor(options.fromPx / tileWidth));
  const last = Math.min(count - 1, Math.ceil((options.fromPx + options.visibleWidth) / tileWidth));
  if (last < first) return [];

  const tiles: Tile[] = [];
  for (let index = first; index <= last; index++) {
    const left = index * tileWidth;
    // Speed makes the source advance faster than the timeline does.
    const sourceTime = sourceStart + (left / zoom) * speed;
    tiles.push({
      key: index,
      left,
      width: Math.min(tileWidth, clipWidth - left),
      url: frameAt(options.tiles, sourceTime, materialDuration),
    });
  }
  return tiles;
}

export interface FilmstripProps {
  segment: SegmentModel;
  /** The material's file, which is what the cache is keyed by. */
  path: string;
  aspect: number;
  materialDuration: Micros;
  zoom: number;
  clipWidth: number;
  fromPx: number;
  visibleWidth: number;
  height: number;
}

function FilmstripLayer({
  segment,
  path,
  aspect,
  materialDuration,
  zoom,
  clipWidth,
  fromPx,
  visibleWidth,
  height,
}: FilmstripProps) {
  // Subscribed here rather than in the timeline: a batch landing for one file
  // must repaint the clips that use it and nothing else.
  const strip = useThumbnailStore((state) => state.strips[path]);
  const status = strip?.status ?? "pending";

  // A clip whose strip has not even been asked for yet still lays out its
  // tiles: every one of them comes back empty and draws as a placeholder, so a
  // clip that has just been dropped looks like a clip waiting for frames rather
  // than like a clip that failed to get any.
  const tiles = filmstripTiles({
    tiles: strip?.tiles ?? [],
    aspect,
    materialDuration,
    sourceStart: segment.source_range.start,
    speed: segment.speed,
    zoom,
    clipWidth,
    fromPx,
    visibleWidth,
    height,
  });

  return (
    <span
      data-slot="filmstrip"
      data-status={status}
      className="pointer-events-none absolute inset-x-0 bottom-0 top-[2px] overflow-hidden"
    >
      {tiles.map((tile) =>
        tile.url ? (
          <span
            key={tile.key}
            className="absolute inset-y-0 bg-cover bg-center"
            style={{ left: tile.left, width: tile.width, backgroundImage: `url("${tile.url}")` }}
          />
        ) : (
          // A tile that has not arrived. It pulses while the decoder is still
          // working and settles into a flat panel once it is not, so "coming"
          // and "never" are different pictures rather than the same blank one.
          <span
            key={tile.key}
            data-slot="filmstrip-placeholder"
            className={cn(
              "absolute inset-y-0 border-r border-black/20",
              status === "failed" ? "bg-timeline-clip-missing" : "bg-timeline-clip-pending",
              status === "pending" || status === "partial" ? "animate-pulse" : null,
            )}
            style={{ left: tile.left, width: tile.width }}
          />
        ),
      )}
    </span>
  );
}

export const Filmstrip = memo(FilmstripLayer);
