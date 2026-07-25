import { FileVideoIcon, ImageIcon, MusicIcon, XIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { MEDIA_DRAG_MIME } from "@/lib/dnd";
import { formatDuration } from "@/lib/time";
import { cn } from "@/lib/utils";
import { useThumbnailStore } from "@/modules/media/lib/thumbnails";
import type { ImportedMaterial, MaterialKind } from "@/modules/project/types";

const KIND_ICON: Record<MaterialKind, typeof FileVideoIcon> = {
  video: FileVideoIcon,
  audio: MusicIcon,
  image: ImageIcon,
  text: FileVideoIcon,
};

const KIND_TINT: Record<MaterialKind, string> = {
  video: "text-track-video",
  audio: "text-track-audio",
  image: "text-track-sticker",
  text: "text-track-text",
};

interface MediaItemProps {
  item: ImportedMaterial;
  onRemove: (id: string) => void;
}

export function MediaItem({ item, onRemove }: MediaItemProps) {
  const Icon = KIND_ICON[item.kind];
  // Middle of the strip: the first frame of a video is usually black or a slate.
  const strip = useThumbnailStore((s) => s.strips[item.path]);
  const poster = strip && strip.length > 0 ? strip[Math.floor(strip.length / 2)] : null;

  const resolution =
    item.width > 0 && item.height > 0
      ? `${item.width}×${item.height}`
      : item.kind === "audio"
        ? "audio"
        : "unknown size";

  return (
    <div data-slot="media-item" className="group relative flex flex-col gap-1">
      {/* The tile itself is the drag source; the remove button is a sibling
          layered over it rather than a child, so the markup stays valid. */}
      <button
        type="button"
        draggable
        aria-label={`${item.name} — drag onto the timeline`}
        onDragStart={(event) => {
          // The material id, not the path: it is already in the pool, and the
          // timeline should not have to search by filename to find it.
          event.dataTransfer.setData(MEDIA_DRAG_MIME, item.id);
          event.dataTransfer.setData("text/plain", item.path);
          event.dataTransfer.effectAllowed = "copy";
        }}
        className="relative block aspect-video w-full cursor-grab overflow-hidden rounded-sm border border-border bg-surface p-0 outline-none transition-colors group-hover:border-primary/70 focus-visible:border-primary focus-visible:ring-[2px] focus-visible:ring-ring/60 active:cursor-grabbing"
      >
        {poster ? (
          <span
            className="block size-full bg-cover bg-center"
            style={{ backgroundImage: `url("${poster}")` }}
          />
        ) : (
          <span className="grid size-full place-items-center">
            <Icon className={cn("size-5 opacity-70", KIND_TINT[item.kind])} />
          </span>
        )}

        {item.duration > 0 ? (
          <span className="absolute bottom-1 right-1 rounded-[3px] bg-black/70 px-1 py-px font-mono text-[10px] leading-tight text-white/90">
            {formatDuration(item.duration)}
          </span>
        ) : null}

        <span className="pointer-events-none absolute inset-x-0 bottom-0 h-6 bg-gradient-to-t from-black/45 to-transparent opacity-0 transition-opacity group-hover:opacity-100" />
      </button>

      <Button
        size="icon-sm"
        variant="secondary"
        aria-label={`Remove ${item.name}`}
        onClick={() => onRemove(item.id)}
        // Hover alone would make this unreachable without a mouse: the button is
        // in the tab order whether it is painted or not, so it also appears when
        // anything inside the tile takes focus.
        className="absolute right-1 top-1 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100 focus-visible:opacity-100"
      >
        <XIcon />
      </Button>

      <div className="min-w-0 px-px">
        <p className="truncate text-[11px] leading-tight text-foreground/90" title={item.path}>
          {item.name}
        </p>
        <p className="truncate text-[10px] leading-tight text-muted-foreground">
          {resolution}
          {item.has_audio && item.kind === "video" ? " · sound" : ""}
        </p>
      </div>
    </div>
  );
}
