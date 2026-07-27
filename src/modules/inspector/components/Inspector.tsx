import {
  FlipHorizontalIcon,
  FlipVerticalIcon,
  RotateCcwIcon,
  RotateCcwSquareIcon,
  RotateCwSquareIcon,
  SlidersHorizontalIcon,
} from "lucide-react";
import { useCallback } from "react";

import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { formatDuration, formatTimecode } from "@/lib/time";
import { ColorSection } from "@/modules/inspector/components/ColorSection";
import { CropSection } from "@/modules/inspector/components/CropSection";
import { KeyframeEditor } from "@/modules/inspector/components/KeyframeEditor";
import { KeyframeRow } from "@/modules/inspector/components/KeyframeRow";
import { PropertySlider } from "@/modules/inspector/components/PropertySlider";
import { TextInspector } from "@/modules/inspector/components/TextInspector";
import { rotateBy } from "@/modules/inspector/lib/adjust";
import { OPACITY, TRANSFORM_PROPERTIES, VOLUME } from "@/modules/inspector/lib/properties";
import { runEdit, useProjectStore } from "@/modules/project/store";
import type { Transform } from "@/modules/project/types";
import { findSegment, projectDuration, segmentLabel } from "@/modules/project/types";
import { textMaterialOf } from "@/modules/text/lib/material";
import { timelineApply } from "@/modules/timeline/lib/api";
import { soleSelection, useTimelineStore } from "@/modules/timeline/store";
import { TransitionInspector } from "@/modules/transitions/components/TransitionInspector";

const DEFAULT_TRANSFORM: Transform = {
  position: [0, 0],
  scale: [1, 1],
  rotation: 0,
  opacity: 1,
  flip_h: false,
  flip_v: false,
};

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="px-3 py-2.5">
      <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        {title}
      </h3>
      <div className="flex flex-col gap-2">{children}</div>
    </section>
  );
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid grid-cols-[76px_1fr] items-baseline gap-2">
      <span className="truncate text-[11px] text-muted-foreground">{label}</span>
      <span className="truncate font-mono text-[11px] text-foreground/85" title={value}>
        {value}
      </span>
    </div>
  );
}

export function Inspector() {
  const project = useProjectStore((s) => s.project);
  // The sole selection, not the first of several: with four clips selected
  // there is no "the" clip, and editing one of them would change something the
  // user is not looking at. See `soleSelection`.
  const selection = useTimelineStore((s) => s.selection);
  const selectedSegmentId = soleSelection(selection);

  const found = project && selectedSegmentId ? findSegment(project, selectedSegmentId) : null;
  const segment = found?.segment ?? null;
  const hasAudio = found?.track.kind === "video" || found?.track.kind === "audio";
  // A title is a segment like any other; what makes it one is the kind of
  // material it names, not the lane it happens to sit on.
  const title = project && segment ? textMaterialOf(project, segment) : null;

  const commitTransform = useCallback(
    (patch: Partial<Transform>) => {
      if (!segment) return;
      void runEdit(() =>
        timelineApply({
          type: "set_transform",
          segment_id: segment.id,
          before: segment.transform,
          after: { ...segment.transform, ...patch },
        }),
      );
    },
    [segment],
  );

  const commitSpeed = useCallback(
    (speed: number) => {
      if (!segment) return;
      void runEdit(() =>
        timelineApply({
          type: "set_speed",
          segment_id: segment.id,
          before: segment.speed,
          after: speed,
        }),
      );
    },
    [segment],
  );

  return (
    <aside
      data-slot="inspector"
      className="flex h-full min-h-0 flex-col bg-panel"
      aria-label="Inspector"
    >
      <header className="flex h-8 shrink-0 items-center justify-between border-b border-border px-3">
        <h2 className="text-[12px] font-semibold tracking-tight text-panel-foreground">Details</h2>
        {found ? (
          <span className="truncate text-[11px] text-muted-foreground">{found.track.name}</span>
        ) : null}
      </header>

      {!project ? (
        <div className="flex h-full flex-col items-center justify-center gap-1.5 px-6 text-center">
          <SlidersHorizontalIcon className="size-5 text-muted-foreground/50" />
          <p className="text-[12px] text-muted-foreground">No project open</p>
          <p className="max-w-[220px] text-[11px] leading-relaxed text-muted-foreground/70">
            Clip properties show up here once a project is loaded.
          </p>
        </div>
      ) : !segment || !found ? (
        <ScrollArea className="flex-1">
          <Section title="Project">
            <Field label="Name" value={project.name} />
            <Field label="Canvas" value={`${project.canvas.width} × ${project.canvas.height}`} />
            <Field label="Frame rate" value={`${project.fps.toFixed(2)} fps`} />
            <Field label="Duration" value={formatTimecode(projectDuration(project), project.fps)} />
            <Field label="Tracks" value={String(project.tracks.length)} />
          </Section>
          <Separator />
          {/* Several clips selected is not "nothing selected", and saying so is
              the difference between the panel looking broken and it saying what
              it can and cannot do. The multi-clip edits live on the timeline —
              move, trim, delete, link — and are all one undo step; per-property
              editing needs one clip, because a slider has one value. */}
          {selection.length > 1 ? (
            <p className="px-3 py-3 text-[11px] leading-relaxed text-muted-foreground">
              {selection.length} clips selected. Move, trim, copy, delete or link them together on
              the timeline; select a single clip to edit its transform, speed and volume.
            </p>
          ) : (
            <p className="px-3 py-3 text-[11px] leading-relaxed text-muted-foreground">
              Select a clip on the timeline to edit its transform, speed and volume.
            </p>
          )}
        </ScrollArea>
      ) : (
        <ScrollArea className="flex-1">
          <Section title="Clip">
            <Field label="Name" value={segmentLabel(project, segment)} />
            <Field label="Duration" value={formatDuration(segment.target_range.duration)} />
            <Field
              label="Position"
              value={formatTimecode(segment.target_range.start, project.fps)}
            />
            <Field
              label="Source in"
              value={formatTimecode(segment.source_range.start, project.fps)}
            />
          </Section>

          {title ? (
            <>
              <Separator />
              <Section title="Text">
                <TextInspector project={project} segment={segment} material={title} />
              </Section>
            </>
          ) : null}

          {/* A transition belongs to the clip it is the entrance to, so it is
              edited from that clip's panel and renders nothing when there is
              none. Selecting the marker on the timeline selects the same clip,
              which is what makes the two routes land in one place. */}
          <TransitionInspector project={project} segmentId={segment.id} />

          <Separator />

          <Section title="Transform">
            {TRANSFORM_PROPERTIES.map((def) => (
              <KeyframeRow key={def.id} project={project} segment={segment} def={def} />
            ))}

            <div className="flex items-center gap-1 pt-0.5">
              <Button
                variant="toggle"
                size="sm"
                data-active={segment.transform.flip_h}
                onClick={() => commitTransform({ flip_h: !segment.transform.flip_h })}
              >
                <FlipHorizontalIcon />
                Flip H
              </Button>
              <Button
                variant="toggle"
                size="sm"
                data-active={segment.transform.flip_v}
                onClick={() => commitTransform({ flip_v: !segment.transform.flip_v })}
              >
                <FlipVerticalIcon />
                Flip V
              </Button>
              {/* Quarter turns, through the same set_transform edit the
                  rotation slider commits, wrapped so four of them land back
                  on exactly 0°. */}
              <Button
                size="sm"
                aria-label="Rotate left 90 degrees"
                onClick={() =>
                  commitTransform({ rotation: rotateBy(segment.transform.rotation, -90) })
                }
              >
                <RotateCcwSquareIcon />
                90°
              </Button>
              <Button
                size="sm"
                aria-label="Rotate right 90 degrees"
                onClick={() =>
                  commitTransform({ rotation: rotateBy(segment.transform.rotation, 90) })
                }
              >
                <RotateCwSquareIcon />
                90°
              </Button>
              <Button
                size="sm"
                className="ml-auto"
                onClick={() => commitTransform(DEFAULT_TRANSFORM)}
              >
                <RotateCcwIcon />
                Reset
              </Button>
            </div>
          </Section>

          <Separator />

          <Section title="Crop">
            <CropSection segment={segment} />
          </Section>

          <Separator />

          <Section title="Colour">
            <ColorSection project={project} segment={segment} />
            <KeyframeRow project={project} segment={segment} def={OPACITY} />
          </Section>

          <Separator />

          <Section title="Speed">
            <PropertySlider
              label="Speed"
              value={segment.speed}
              min={0.1}
              max={4}
              step={0.05}
              format={(value) => `${value.toFixed(2)}×`}
              onCommit={commitSpeed}
            />
          </Section>

          <Separator />

          <Section title="Audio">
            <KeyframeRow project={project} segment={segment} def={VOLUME} disabled={!hasAudio} />
          </Section>

          <Separator />

          <Section title="Keyframes">
            <KeyframeEditor project={project} segment={segment} />
          </Section>
        </ScrollArea>
      )}
    </aside>
  );
}
