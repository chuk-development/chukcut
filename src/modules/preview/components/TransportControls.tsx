import {
  ChevronFirstIcon,
  ChevronLastIcon,
  Maximize2Icon,
  MaximizeIcon,
  Minimize2Icon,
  PauseIcon,
  PlayIcon,
  SkipBackIcon,
  SkipForwardIcon,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { IconTooltip } from "@/components/ui/tooltip";
import { formatTimecode } from "@/lib/time";
import { cn } from "@/lib/utils";
import type { PreviewZoom } from "@/modules/preview/store";
import type { Micros } from "@/modules/project/types";

interface TransportControlsProps {
  playing: boolean;
  playhead: Micros;
  duration: Micros;
  fps: number;
  zoom: PreviewZoom;
  fullscreen: boolean;
  /**
   * The overlay form: no panel chrome and no zoom control, because a fullscreen
   * picture is already at the only scale that matters.
   */
  compact?: boolean;
  onTogglePlay: () => void;
  onStep: (frames: number) => void;
  onJump: (position: "start" | "end") => void;
  onZoom: (zoom: PreviewZoom) => void;
  onToggleFullscreen: () => void;
}

const ZOOM_OPTIONS: { value: string; label: string }[] = [
  { value: "fit", label: "Fit" },
  { value: "0.25", label: "25%" },
  { value: "0.5", label: "50%" },
  { value: "1", label: "100%" },
  { value: "2", label: "200%" },
];

export function TransportControls({
  playing,
  playhead,
  duration,
  fps,
  zoom,
  fullscreen,
  compact = false,
  onTogglePlay,
  onStep,
  onJump,
  onZoom,
  onToggleFullscreen,
}: TransportControlsProps) {
  return (
    <div
      data-slot="transport-controls"
      className={cn(
        "flex h-9 shrink-0 items-center gap-0.5",
        compact ? "rounded-md px-2" : "border-t border-border bg-panel px-2",
      )}
    >
      <div className="flex items-baseline gap-1.5 font-mono text-[11px] tabular-nums">
        <span className="text-primary">{formatTimecode(playhead, fps)}</span>
        <span className="text-muted-foreground/60">/</span>
        <span className="text-muted-foreground">{formatTimecode(duration, fps)}</span>
      </div>

      <div className={cn("flex items-center gap-0.5", compact ? "ml-3" : "mx-auto")}>
        <IconTooltip label="Jump to start" hint="Home" side={compact ? "top" : "bottom"}>
          <Button size="icon" onClick={() => onJump("start")}>
            <ChevronFirstIcon />
          </Button>
        </IconTooltip>
        <IconTooltip label="Previous frame" hint="←" side={compact ? "top" : "bottom"}>
          <Button size="icon" onClick={() => onStep(-1)}>
            <SkipBackIcon />
          </Button>
        </IconTooltip>
        <IconTooltip
          label={playing ? "Pause" : "Play"}
          hint="Space"
          side={compact ? "top" : "bottom"}
        >
          <Button variant="secondary" size="icon-lg" onClick={onTogglePlay}>
            {playing ? <PauseIcon /> : <PlayIcon />}
          </Button>
        </IconTooltip>
        <IconTooltip label="Next frame" hint="→" side={compact ? "top" : "bottom"}>
          <Button size="icon" onClick={() => onStep(1)}>
            <SkipForwardIcon />
          </Button>
        </IconTooltip>
        <IconTooltip label="Jump to end" hint="End" side={compact ? "top" : "bottom"}>
          <Button size="icon" onClick={() => onJump("end")}>
            <ChevronLastIcon />
          </Button>
        </IconTooltip>
      </div>

      <Separator orientation="vertical" className="mx-1.5 h-4" />

      <IconTooltip
        label={fullscreen ? "Leave fullscreen" : "Fullscreen"}
        hint={fullscreen ? "Esc" : "F"}
        side={compact ? "top" : "bottom"}
      >
        <Button
          size="icon"
          aria-label={fullscreen ? "Leave fullscreen" : "Fullscreen"}
          onClick={onToggleFullscreen}
        >
          {fullscreen ? <Minimize2Icon /> : <Maximize2Icon />}
        </Button>
      </IconTooltip>

      {compact ? null : (
        <>
          <MaximizeIcon className="mx-1 size-3 text-muted-foreground" />
          <Select
            value={String(zoom)}
            onValueChange={(value) => onZoom(value === "fit" ? "fit" : Number(value))}
          >
            <SelectTrigger className="h-[22px] w-[72px] text-[11px]" aria-label="Preview zoom">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {ZOOM_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </>
      )}
    </div>
  );
}
