/**
 * The Text tab of the media library: where a title comes from.
 *
 * Adding a title is one press and needs no dialog. It lands at the playhead on
 * a text lane, with a placeholder in it, selected — so the next thing the user
 * does is type into the inspector, which is the only thing they wanted to do.
 * Rust chooses the lane, the length and the defaults; see
 * `src-tauri/src/modules/text/edit.rs` for why that policy is not here.
 */

import { PlusIcon, TypeIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useProjectStore } from "@/modules/project/store";
import { TITLE_PRESETS } from "@/modules/text/lib/material";
import { addTitle, useTextStore } from "@/modules/text/store";

export function TextPanel() {
  const project = useProjectStore((s) => s.project);
  const adding = useTextStore((s) => s.adding);
  const titles = project?.materials.texts.length ?? 0;

  return (
    <ScrollArea className="h-full">
      <div className="flex flex-col gap-2 p-2.5">
        <Button
          variant="default"
          size="default"
          disabled={!project || adding}
          aria-busy={adding}
          onClick={() => void addTitle()}
        >
          <PlusIcon />
          Add title
        </Button>
        <p className="text-[11px] leading-relaxed text-muted-foreground/80">
          Lands at the playhead on a text lane, above the picture. Edit the words, font, colour and
          outline in Details.
        </p>

        <div className="mt-1 grid grid-cols-2 gap-1.5">
          {TITLE_PRESETS.map((preset) => (
            <button
              key={preset.id}
              type="button"
              disabled={!project || adding}
              onClick={() => void addTitle(preset.content)}
              className="flex h-16 flex-col items-center justify-center gap-1 rounded-sm border border-border bg-surface px-2 text-center transition-colors hover:border-primary/60 hover:bg-accent disabled:pointer-events-none disabled:opacity-40"
            >
              <TypeIcon className="size-3.5 text-muted-foreground" />
              <span className="line-clamp-2 text-[11px] leading-tight text-foreground/85">
                {preset.label}
              </span>
            </button>
          ))}
        </div>

        {titles > 0 ? (
          <p className="pt-1 text-[11px] text-muted-foreground">
            {titles} title{titles === 1 ? "" : "s"} in this project
          </p>
        ) : null}
      </div>
    </ScrollArea>
  );
}
