import {
  AudioLinesIcon,
  EyeIcon,
  EyeOffIcon,
  FilmIcon,
  GripVerticalIcon,
  LockIcon,
  SparklesIcon,
  StickerIcon,
  Trash2Icon,
  TypeIcon,
  UnlockIcon,
  Volume2Icon,
  VolumeXIcon,
} from "lucide-react";
import type React from "react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
  /** Delete the lane. Only called once the user has confirmed, when it holds clips. */
  onDelete: (track: TrackModel) => void;
  /** Grab the header to drag the lane to a new place in the order. */
  onReorderStart: (event: React.PointerEvent, track: TrackModel) => void;
}

export function TrackHeader({ track, onToggle, onDelete, onReorderStart }: TrackHeaderProps) {
  const Icon = KIND_ICON[track.kind];
  const hasAudio = track.kind === "video" || track.kind === "audio";
  const hasVisual = track.kind !== "audio";
  /**
   * The delete confirmation, for a lane with clips on it. An empty lane
   * deletes without ceremony — there is nothing to lose, and undo brings the
   * lane back either way. Local state rather than a store: the dialog belongs
   * to this header and to nothing else.
   */
  const [confirming, setConfirming] = useState(false);

  const requestDelete = () => {
    if (track.segments.length > 0) setConfirming(true);
    else onDelete(track);
  };

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          data-slot="track-header"
          className="group/header flex items-center gap-1 border-b border-border/60 bg-panel pl-0.5 pr-2"
          style={{ height: trackHeight(track.kind) }}
        >
          {/* The drag surface for reordering. A dedicated grip rather than the
              whole header, because the header is already full of buttons and a
              drag that starts on "mute" must stay a click. */}
          <button
            type="button"
            aria-label={`Reorder ${track.name}`}
            title="Drag to reorder tracks"
            data-slot="track-grip"
            onPointerDown={(event) => {
              if (event.button !== 0) return;
              onReorderStart(event, track);
            }}
            className="grid h-full w-4 shrink-0 cursor-grab place-items-center border-none bg-transparent p-0 text-muted-foreground/40 outline-none hover:text-muted-foreground active:cursor-grabbing"
          >
            <GripVerticalIcon className="size-3" />
          </button>
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

          {/* The confirmation lives inside the trigger so the context menu can
              open it and go away without unmounting it. */}
          <Dialog open={confirming} onOpenChange={setConfirming}>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Delete {track.name}?</DialogTitle>
                <DialogDescription>
                  {track.segments.length === 1
                    ? "The clip on this track goes with it."
                    : `The ${track.segments.length} clips on this track go with it.`}{" "}
                  Undo brings everything back.
                </DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <DialogClose asChild>
                  <Button size="sm" variant="ghost">
                    Cancel
                  </Button>
                </DialogClose>
                <Button
                  size="sm"
                  variant="destructive"
                  onClick={() => {
                    setConfirming(false);
                    onDelete(track);
                  }}
                >
                  Delete track
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </div>
      </ContextMenuTrigger>

      <ContextMenuContent>
        <ContextMenuItem onSelect={requestDelete}>
          <Trash2Icon />
          Delete track
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
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
