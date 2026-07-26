import {
  CopyIcon,
  LinkIcon,
  MagnetIcon,
  MousePointer2Icon,
  Redo2Icon,
  ScissorsIcon,
  Trash2Icon,
  Undo2Icon,
  ZoomInIcon,
  ZoomOutIcon,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Slider } from "@/components/ui/slider";
import { IconTooltip } from "@/components/ui/tooltip";
import { formatTimecode } from "@/lib/time";
import type { Micros } from "@/modules/project/types";
import { MAX_ZOOM, MIN_ZOOM } from "@/modules/timeline/store";

interface TimelineToolbarProps {
  playhead: Micros;
  duration: Micros;
  fps: number;
  zoom: number;
  tool: "select" | "razor";
  snapping: boolean;
  canUndo: boolean;
  canRedo: boolean;
  /** How many clips are selected. Two or more is what makes Link worth offering. */
  selectionCount: number;
  /** Whether there is anything on the timeline at all. */
  hasClips: boolean;
  onZoom: (zoom: number) => void;
  onZoomToFit: () => void;
  onSetTool: (tool: "select" | "razor") => void;
  onToggleSnapping: () => void;
  onUndo: () => void;
  onRedo: () => void;
  onSplit: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  /** Make the selected clips move, trim and delete as one. */
  onLink: () => void;
}

/** Zoom is exponential, so the slider works in log space or it is useless. */
function zoomToSlider(zoom: number): number {
  const t = Math.log(zoom / MIN_ZOOM) / Math.log(MAX_ZOOM / MIN_ZOOM);
  return Math.round(t * 100);
}

function sliderToZoom(value: number): number {
  return MIN_ZOOM * (MAX_ZOOM / MIN_ZOOM) ** (value / 100);
}

export function TimelineToolbar({
  playhead,
  duration,
  fps,
  zoom,
  tool,
  snapping,
  canUndo,
  canRedo,
  selectionCount,
  hasClips,
  onZoom,
  onZoomToFit,
  onSetTool,
  onToggleSnapping,
  onUndo,
  onRedo,
  onSplit,
  onDuplicate,
  onDelete,
  onLink,
}: TimelineToolbarProps) {
  const hasSelection = selectionCount > 0;
  return (
    <div
      data-slot="timeline-toolbar"
      className="flex h-9 shrink-0 items-center gap-0.5 border-b border-border bg-panel px-2"
    >
      <IconTooltip label="Select" hint="V">
        <Button
          variant="toggle"
          size="icon"
          data-active={tool === "select"}
          onClick={() => onSetTool("select")}
        >
          <MousePointer2Icon />
        </Button>
      </IconTooltip>
      {/* B, not C: the tool cuts where the pointer is, the shortcut cuts at the
          playhead. Two gestures, two keys. */}
      <IconTooltip label="Razor" hint="B">
        <Button
          variant="toggle"
          size="icon"
          data-active={tool === "razor"}
          onClick={() => onSetTool("razor")}
        >
          <ScissorsIcon />
        </Button>
      </IconTooltip>

      <Separator orientation="vertical" className="mx-1.5 h-4" />

      <IconTooltip label="Undo" hint="Ctrl+Z">
        <Button size="icon" onClick={onUndo} disabled={!canUndo}>
          <Undo2Icon />
        </Button>
      </IconTooltip>
      <IconTooltip label="Redo" hint="Ctrl+Shift+Z">
        <Button size="icon" onClick={onRedo} disabled={!canRedo}>
          <Redo2Icon />
        </Button>
      </IconTooltip>

      <Separator orientation="vertical" className="mx-1.5 h-4" />

      <IconTooltip label="Split at playhead" hint="C">
        <Button size="icon" onClick={onSplit} disabled={!hasClips}>
          <ScissorsIcon />
        </Button>
      </IconTooltip>
      <IconTooltip label={selectionCount > 1 ? "Duplicate clips" : "Duplicate clip"} hint="Ctrl+D">
        <Button size="icon" onClick={onDuplicate} disabled={!hasSelection}>
          <CopyIcon />
        </Button>
      </IconTooltip>
      <IconTooltip label={selectionCount > 1 ? "Delete clips" : "Delete clip"} hint="Del">
        <Button size="icon" onClick={onDelete} disabled={!hasSelection}>
          <Trash2Icon />
        </Button>
      </IconTooltip>
      {/* Linking is only meaningful for a set, so the button appears with one.
          Greyed for a single clip would be a permanent grey button. */}
      <IconTooltip label="Link the selected clips so they move together">
        <Button size="icon" onClick={onLink} disabled={selectionCount < 2}>
          <LinkIcon />
        </Button>
      </IconTooltip>

      <Separator orientation="vertical" className="mx-1.5 h-4" />

      <IconTooltip label={snapping ? "Snapping on" : "Snapping off"} hint="S">
        <Button variant="toggle" size="icon" data-active={snapping} onClick={onToggleSnapping}>
          <MagnetIcon />
        </Button>
      </IconTooltip>

      <div className="ml-3 flex items-baseline gap-1.5 font-mono text-[11px] tabular-nums">
        <span className="text-foreground">{formatTimecode(playhead, fps)}</span>
        <span className="text-muted-foreground/60">/</span>
        <span className="text-muted-foreground">{formatTimecode(duration, fps)}</span>
      </div>

      <div className="ml-auto flex items-center gap-1.5">
        <IconTooltip label="Fit the whole timeline on screen">
          <Button size="sm" variant="ghost" onClick={onZoomToFit}>
            Fit
          </Button>
        </IconTooltip>
        <IconTooltip label="Zoom out" hint="Ctrl+wheel">
          <Button size="icon-sm" onClick={() => onZoom(zoom / 1.6)}>
            <ZoomOutIcon />
          </Button>
        </IconTooltip>
        <Slider
          className="w-28"
          min={0}
          max={100}
          step={1}
          value={[zoomToSlider(zoom)]}
          onValueChange={([value]) => onZoom(sliderToZoom(value))}
          aria-label="Timeline zoom"
        />
        <IconTooltip label="Zoom in" hint="Ctrl+wheel">
          <Button size="icon-sm" onClick={() => onZoom(zoom * 1.6)}>
            <ZoomInIcon />
          </Button>
        </IconTooltip>
      </div>
    </div>
  );
}
