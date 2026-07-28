import {
  AlertTriangleIcon,
  FolderOpenIcon,
  ImportIcon,
  Loader2Icon,
  MusicIcon,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { openFileDialog, VIDEO_FILTERS } from "@/lib/dialog";
import { MediaItem } from "@/modules/media/components/MediaItem";
import { libraryItems, materialUseCount } from "@/modules/media/lib/library";
import { useMediaStore } from "@/modules/media/store";
import { describeError, useProjectStore } from "@/modules/project/store";
import type { Id, ImportedMaterial } from "@/modules/project/types";
import { TextPanel } from "@/modules/text/components/TextPanel";

function EmptyState({
  message,
  hint,
  icon: Icon = FolderOpenIcon,
}: {
  message: string;
  hint?: string;
  icon?: typeof FolderOpenIcon;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-1.5 px-6 text-center">
      <Icon className="size-5 text-muted-foreground/50" />
      <p className="text-[12px] text-muted-foreground">{message}</p>
      {hint ? (
        <p className="max-w-[220px] text-[11px] leading-relaxed text-muted-foreground/70">{hint}</p>
      ) : null}
    </div>
  );
}

/**
 * Tiles the shape of the ones that are coming.
 *
 * A probe of a long file takes a moment, and a panel that stays empty for it
 * reads as an import that silently did nothing.
 */
function ImportingGrid() {
  return (
    <div
      className="grid grid-cols-[repeat(auto-fill,minmax(104px,1fr))] gap-2 p-2.5"
      aria-hidden="true"
    >
      {[0, 1, 2].map((slot) => (
        <div key={slot} className="flex flex-col gap-1">
          <div className="aspect-video w-full animate-pulse rounded-sm bg-surface" />
          <div className="h-2 w-3/4 animate-pulse rounded-sm bg-surface" />
        </div>
      ))}
    </div>
  );
}

function Grid({
  items,
  usedCounts,
  missing,
  onRemove,
}: {
  items: ImportedMaterial[];
  usedCounts: Map<Id, number>;
  missing: ReadonlySet<string>;
  onRemove: (id: string) => void;
}) {
  return (
    <ScrollArea className="h-full">
      <div className="grid grid-cols-[repeat(auto-fill,minmax(104px,1fr))] gap-2 p-2.5">
        {items.map((item) => (
          <MediaItem
            key={item.id}
            item={item}
            usedCount={usedCounts.get(item.id) ?? 0}
            missing={missing.has(item.path)}
            onRemove={onRemove}
          />
        ))}
      </div>
    </ScrollArea>
  );
}

export function MediaLibrary() {
  // The library is a view of the project's material pool — the same pool the
  // timeline resolves clips against, which is what links the two panels. A
  // saved and reopened project therefore shows its media with no session
  // state involved.
  const project = useProjectStore((s) => s.project);
  const items = useMemo(() => libraryItems(project), [project]);
  const usedCounts = useMemo(() => {
    const counts = new Map<Id, number>();
    if (!project) return counts;
    for (const item of items) counts.set(item.id, materialUseCount(project, item.id));
    return counts;
  }, [project, items]);

  const importing = useMediaStore((s) => s.importing);
  const error = useMediaStore((s) => s.error);
  const importPaths = useMediaStore((s) => s.importPaths);
  const removeFromProject = useMediaStore((s) => s.removeFromProject);
  const clearError = useMediaStore((s) => s.clearError);
  const [dialogError, setDialogError] = useState<string | null>(null);

  // Which pool files are gone from disk right now. Re-checked whenever the
  // document changes — an import, a removal or an undo all change the answer.
  const missingPaths = useMediaStore((s) => s.missingPaths);
  const missing = useMemo(() => new Set(missingPaths), [missingPaths]);
  // biome-ignore lint/correctness/useExhaustiveDependencies(project): the document is the trigger, not an input — refreshMissing reads it from the store itself
  useEffect(() => {
    void useMediaStore.getState().refreshMissing();
  }, [project]);

  const handleImport = useCallback(async () => {
    setDialogError(null);
    try {
      const paths = await openFileDialog({
        title: "Import media",
        multiple: true,
        filters: VIDEO_FILTERS,
      });
      await importPaths(paths);
    } catch (caught) {
      setDialogError(describeError(caught));
    }
  }, [importPaths]);

  const handleRemove = useCallback(
    (id: string) => {
      void removeFromProject(id);
    },
    [removeFromProject],
  );

  const audio = items.filter((item) => item.kind === "audio");
  const problem = dialogError ?? error;

  return (
    <section
      data-slot="media-library"
      className="flex h-full min-h-0 flex-col bg-panel"
      aria-label="Media library"
    >
      <Tabs defaultValue="media" className="h-full">
        <div className="flex h-8 shrink-0 items-center justify-between border-b border-border px-1.5">
          <TabsList>
            <TabsTrigger value="media">Media</TabsTrigger>
            <TabsTrigger value="audio">Audio</TabsTrigger>
            <TabsTrigger value="text">Text</TabsTrigger>
          </TabsList>
        </div>

        <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border px-2.5">
          <Button
            variant="default"
            size="sm"
            onClick={handleImport}
            disabled={importing}
            aria-busy={importing}
          >
            {importing ? <Loader2Icon className="animate-spin" /> : <ImportIcon />}
            {importing ? "Importing…" : "Import"}
          </Button>
          <span className="text-[11px] text-muted-foreground">
            {items.length > 0 ? `${items.length} item${items.length === 1 ? "" : "s"}` : "Local"}
          </span>
        </div>

        {problem ? (
          <button
            type="button"
            onClick={() => {
              clearError();
              setDialogError(null);
            }}
            className="flex shrink-0 items-start gap-1.5 border-b border-border bg-destructive/15 px-2.5 py-1.5 text-left text-[11px] leading-snug text-destructive-foreground"
          >
            <AlertTriangleIcon className="mt-px size-3 shrink-0" />
            <span className="min-w-0 flex-1">{problem}</span>
          </button>
        ) : null}

        <TabsContent value="media">
          {items.length === 0 ? (
            importing ? (
              <ImportingGrid />
            ) : (
              <EmptyState
                message="No media imported"
                hint="Click Import, or drag video, image and audio files straight from your file manager onto this panel."
              />
            )
          ) : (
            <Grid items={items} usedCounts={usedCounts} missing={missing} onRemove={handleRemove} />
          )}
        </TabsContent>

        <TabsContent value="audio">
          {audio.length === 0 ? (
            <EmptyState
              icon={MusicIcon}
              message="No audio imported"
              hint="Music and voice-over files show up here as well as in Media."
            />
          ) : (
            <Grid items={audio} usedCounts={usedCounts} missing={missing} onRemove={handleRemove} />
          )}
        </TabsContent>

        <TabsContent value="text">
          <TextPanel />
        </TabsContent>
      </Tabs>
    </section>
  );
}
