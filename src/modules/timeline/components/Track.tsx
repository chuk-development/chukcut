import {
  AudioLinesIcon,
  EyeIcon,
  EyeOffIcon,
  FilmIcon,
  LockIcon,
  SparklesIcon,
  StickerIcon,
  TypeIcon,
  UnlockIcon,
  Volume2Icon,
  VolumeXIcon,
} from "lucide-react";
import type React from "react";

import { Button } from "@/components/ui/button";
import { IconTooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { TrackKind, Track as TrackModel } from "@/modules/project/types";
import { trackHeight } from "@/modules/timeline/store";

const KIND_ICON: Record<TrackKind, typeof FilmIcon> = {
  video: FilmIcon,
  audio: AudioLinesIcon,
  text: TypeIcon,
  sticker: StickerIcon,
  effect: SparklesIcon,
};

const KIND_ACCENT: Record<TrackKind, string> = {
  video: "text-track-video",
  audio: "text-track-audio",
  text: "text-track-text",
  sticker: "text-track-sticker",
  effect: "text-track-effect",
};

export type TrackFlag = "muted" | "locked" | "hidden";

interface TrackHeaderProps {
  track: TrackModel;
  onToggle: (track: TrackModel, flag: TrackFlag) => void;
}

export function TrackHeader({ track, onToggle }: TrackHeaderProps) {
  const Icon = KIND_ICON[track.kind];
  const hasAudio = track.kind === "video" || track.kind === "audio";
  const hasVisual = track.kind !== "audio";

  return (
    <div
      data-slot="track-header"
      className="flex items-center gap-1.5 border-b border-border/60 bg-panel px-2"
      style={{ height: trackHeight(track.kind) }}
    >
      <Icon className={cn("size-3.5 shrink-0", KIND_ACCENT[track.kind])} />
      <span className="min-w-0 flex-1 truncate text-[11px] font-medium text-panel-foreground">
        {track.name}
      </span>
      <div className="flex shrink-0 items-center">
        {hasVisual ? (
          <IconTooltip label={track.hidden ? "Show track" : "Hide track"}>
            <Button
              variant="toggle"
              size="icon-sm"
              data-active={track.hidden}
              onClick={() => onToggle(track, "hidden")}
            >
              {track.hidden ? <EyeOffIcon /> : <EyeIcon />}
            </Button>
          </IconTooltip>
        ) : null}
        {hasAudio ? (
          <IconTooltip label={track.muted ? "Unmute track" : "Mute track"}>
            <Button
              variant="toggle"
              size="icon-sm"
              data-active={track.muted}
              onClick={() => onToggle(track, "muted")}
            >
              {track.muted ? <VolumeXIcon /> : <Volume2Icon />}
            </Button>
          </IconTooltip>
        ) : null}
        <IconTooltip label={track.locked ? "Unlock track" : "Lock track"}>
          <Button
            variant="toggle"
            size="icon-sm"
            data-active={track.locked}
            onClick={() => onToggle(track, "locked")}
          >
            {track.locked ? <LockIcon /> : <UnlockIcon />}
          </Button>
        </IconTooltip>
      </div>
    </div>
  );
}

interface TrackLaneProps {
  track: TrackModel;
  /** Highlighted while a clip is being dragged over this lane. */
  dropTarget: boolean;
  registerLane: (trackId: string, element: HTMLDivElement | null) => void;
  children: React.ReactNode;
}

export function TrackLane({ track, dropTarget, registerLane, children }: TrackLaneProps) {
  return (
    <div
      data-slot="track-lane"
      data-track-id={track.id}
      ref={(element) => registerLane(track.id, element)}
      className={cn(
        "relative border-b border-border/40",
        track.locked && "opacity-60",
        dropTarget ? "bg-primary/10" : "bg-timeline-background",
      )}
      style={{ height: trackHeight(track.kind) }}
    >
      {children}
    </div>
  );
}
