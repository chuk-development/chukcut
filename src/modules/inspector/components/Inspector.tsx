import { FlipHorizontalIcon, FlipVerticalIcon, RotateCcwIcon } from "lucide-react";
import { useCallback } from "react";

import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { formatDuration, formatTimecode } from "@/lib/time";
import { PropertySlider } from "@/modules/inspector/components/PropertySlider";
import { runEdit, useProjectStore } from "@/modules/project/store";
import type { Transform } from "@/modules/project/types";
import { findSegment, projectDuration, segmentLabel } from "@/modules/project/types";
import { timelineApply } from "@/modules/timeline/lib/api";
import { useTimelineStore } from "@/modules/timeline/store";

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
  const selectedSegmentId = useTimelineStore((s) => s.selectedSegmentId);

  const found = project && selectedSegmentId ? findSegment(project, selectedSegmentId) : null;
  const segment = found?.segment ?? null;

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

  const commitVolume = useCallback(
    (volume: number) => {
      if (!segment) return;
      void runEdit(() =>
        timelineApply({
          type: "set_volume",
          segment_id: segment.id,
          before: segment.volume,
          after: volume,
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
        <div className="grid flex-1 place-items-center px-6 text-center text-[12px] text-muted-foreground">
          No project open
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
          <p className="px-3 py-3 text-[11px] leading-relaxed text-muted-foreground">
            Select a clip on the timeline to edit its transform, speed and volume.
          </p>
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

          <Separator />

          <Section title="Transform">
            <PropertySlider
              label="Position X"
              value={segment.transform.position[0]}
              min={-2}
              max={2}
              step={0.01}
              onCommit={(value) =>
                commitTransform({ position: [value, segment.transform.position[1]] })
              }
            />
            <PropertySlider
              label="Position Y"
              value={segment.transform.position[1]}
              min={-2}
              max={2}
              step={0.01}
              onCommit={(value) =>
                commitTransform({ position: [segment.transform.position[0], value] })
              }
            />
            <PropertySlider
              label="Scale"
              value={segment.transform.scale[0]}
              min={0.05}
              max={4}
              step={0.01}
              format={(value) => `${Math.round(value * 100)}%`}
              onCommit={(value) => commitTransform({ scale: [value, value] })}
            />
            <PropertySlider
              label="Rotation"
              value={segment.transform.rotation}
              min={-180}
              max={180}
              step={1}
              format={(value) => `${Math.round(value)}°`}
              onCommit={(value) => commitTransform({ rotation: value })}
            />
            <PropertySlider
              label="Opacity"
              value={segment.transform.opacity}
              min={0}
              max={1}
              step={0.01}
              format={(value) => `${Math.round(value * 100)}%`}
              onCommit={(value) => commitTransform({ opacity: value })}
            />

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
            <PropertySlider
              label="Volume"
              value={segment.volume}
              min={0}
              max={2}
              step={0.01}
              format={(value) => `${Math.round(value * 100)}%`}
              onCommit={commitVolume}
              disabled={found.track.kind !== "video" && found.track.kind !== "audio"}
            />
          </Section>
        </ScrollArea>
      )}
    </aside>
  );
}
