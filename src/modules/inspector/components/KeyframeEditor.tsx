import { Trash2Icon } from "lucide-react";
import { useCallback } from "react";

import { Button } from "@/components/ui/button";
import { formatTimecode } from "@/lib/time";
import { EasingPicker } from "@/modules/inspector/components/EasingPicker";
import { KeyframeCurve } from "@/modules/inspector/components/KeyframeCurve";
import {
  forProperties,
  type KeyframeEditCommand,
  runKeyframeEdit,
} from "@/modules/inspector/lib/commands";
import {
  findTrack,
  isAnimated,
  keyframeAt,
  keyframeTolerance,
  relativeTime,
  timelineTime,
} from "@/modules/inspector/lib/keyframes";
import {
  ANIMATABLE_PROPERTIES,
  type PropertyDef,
  propertyById,
} from "@/modules/inspector/lib/properties";
import { useInspectorStore } from "@/modules/inspector/store";
import type {
  AnimatableProperty,
  Easing,
  Keyframe,
  Micros,
  Project,
  Segment,
} from "@/modules/project/types";
import { useTimelineStore } from "@/modules/timeline/store";

export interface KeyframeEditorProps {
  project: Project;
  segment: Segment;
}

/**
 * The curve view and everything that acts on a single keyframe.
 *
 * Only properties that are actually animated get a chip: an editor that lists
 * every property whether or not it moves buries the two that do. The chips are
 * how the curve is pointed at a property, and toggling a keyframe on a row
 * points it there too, so the curve is already showing the right thing by the
 * time the user looks down.
 */
export function KeyframeEditor({ project, segment }: KeyframeEditorProps) {
  // From the store rather than a prop: the playhead changes on every position
  // event and every scrub move, and a prop makes that the parent's render rate.
  // The parent was the whole inspector.
  const playhead = useTimelineStore((s) => s.playhead);
  const curveProperty = useInspectorStore((s) => s.curveProperty);
  const showCurve = useInspectorStore((s) => s.showCurve);
  const selectedKeyframeTime = useInspectorStore((s) => s.selectedKeyframeTime);
  const selectKeyframe = useInspectorStore((s) => s.selectKeyframe);
  const setPlayhead = useTimelineStore((s) => s.setPlayhead);

  const animated = ANIMATABLE_PROPERTIES.filter((def) => isAnimated(segment, def.properties[0]));
  const chosen = curveProperty ? propertyById(curveProperty) : null;
  const def: PropertyDef | null =
    chosen && animated.includes(chosen) ? chosen : (animated[0] ?? null);

  const tolerance = keyframeTolerance(project.fps);
  const relative = relativeTime(segment, playhead);
  const track = def ? findTrack(segment, def.properties[0]) : null;

  // The keyframe the easing picker and the delete button act on: whatever was
  // last clicked, and otherwise whatever the playhead is parked on — which is
  // the one the user just created or navigated to.
  const selected: Keyframe | null =
    (track && selectedKeyframeTime !== null
      ? (track.keyframes.find((k) => k.time === selectedKeyframeTime) ?? null)
      : null) ?? keyframeAt(track, relative, tolerance);

  /** Every command here maps over the row's tracks, so a scale gesture is one edit. */
  const forRow = useCallback(
    (label: string, build: (property: AnimatableProperty) => KeyframeEditCommand) =>
      def ? forProperties(label, def.properties, build) : null,
    [def],
  );

  const move = useCallback(
    (keyframe: Keyframe, toTime: Micros, toValue: number) => {
      const command = forRow("Move keyframe", (property) => {
        const own = keyframeAt(findTrack(segment, property), keyframe.time, 0) ?? keyframe;
        return {
          type: "move_keyframe",
          segment_id: segment.id,
          property,
          from_time: own.time,
          to_time: toTime,
          before_value: own.value,
          after_value: toValue,
        };
      });
      if (command) {
        selectKeyframe(toTime);
        void runKeyframeEdit(command);
      }
    },
    [forRow, segment, selectKeyframe],
  );

  const setEasing = useCallback(
    (easing: Easing) => {
      if (!selected) return;
      const command = forRow("Change easing", (property) => {
        const own = keyframeAt(findTrack(segment, property), selected.time, 0) ?? selected;
        return {
          type: "set_keyframe_easing",
          segment_id: segment.id,
          property,
          time: own.time,
          before: own.easing,
          after: easing,
        };
      });
      if (command) void runKeyframeEdit(command);
    },
    [forRow, segment, selected],
  );

  const remove = useCallback(() => {
    if (!selected) return;
    const command = forRow("Delete keyframe", (property) => {
      const own = keyframeAt(findTrack(segment, property), selected.time, 0) ?? selected;
      return { type: "remove_keyframe", segment_id: segment.id, property, keyframe: own };
    });
    if (command) {
      selectKeyframe(null);
      void runKeyframeEdit(command);
    }
  }, [forRow, segment, selected, selectKeyframe]);

  const clear = useCallback(() => {
    if (!def) return;
    const commands: KeyframeEditCommand[] = [];
    for (const property of def.properties) {
      const own = findTrack(segment, property);
      for (const keyframe of own?.keyframes ?? []) {
        commands.push({ type: "remove_keyframe", segment_id: segment.id, property, keyframe });
      }
    }
    if (commands.length === 0) return;
    selectKeyframe(null);
    void runKeyframeEdit(
      commands.length === 1
        ? commands[0]
        : { type: "composite", label: "Remove animation", commands },
    );
  }, [def, segment, selectKeyframe]);

  if (!def || !track) {
    return (
      <p className="text-[11px] leading-relaxed text-muted-foreground">
        Nothing on this clip is animated yet. Click the diamond next to a property to add a keyframe
        at the playhead.
      </p>
    );
  }

  const last = track.keyframes[track.keyframes.length - 1];
  const index = selected ? track.keyframes.findIndex((k) => k.time === selected.time) : -1;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-1">
        {animated.map((option) => (
          <Button
            key={option.id}
            variant="toggle"
            size="sm"
            data-active={option.id === def.id}
            onClick={() => showCurve(option.id)}
          >
            {option.label}
          </Button>
        ))}
        <Button
          variant="ghost"
          size="icon-sm"
          className="ml-auto text-muted-foreground hover:text-destructive"
          aria-label={`Remove ${def.label} animation`}
          title={`Remove every ${def.label} keyframe`}
          onClick={clear}
        >
          <Trash2Icon />
        </Button>
      </div>

      <KeyframeCurve
        track={track}
        def={def}
        duration={segment.target_range.duration}
        fps={project.fps}
        playhead={relative}
        selectedTime={selected?.time ?? null}
        onSelect={selectKeyframe}
        onMove={move}
        onScrub={(time) => setPlayhead(timelineTime(segment, time))}
      />

      <div className="flex items-baseline justify-between gap-2">
        <span className="text-[11px] text-muted-foreground">
          {selected
            ? `Keyframe ${index + 1} of ${track.keyframes.length} · ${formatTimecode(selected.time, project.fps)}`
            : `${track.keyframes.length} keyframes`}
        </span>
        {selected ? (
          <span className="font-mono text-[11px] tabular-nums text-foreground/80">
            {def.format ? def.format(selected.value) : selected.value.toFixed(2)}
          </span>
        ) : null}
      </div>

      <EasingPicker
        value={selected?.easing ?? "linear"}
        disabled={!selected}
        onChange={setEasing}
        note={
          !selected
            ? "Select a keyframe on the curve to change how it reaches the next one."
            : selected.time === last.time
              ? "Nothing follows this keyframe, so its easing does not apply yet."
              : undefined
        }
      />

      {selected ? (
        <Button
          variant="outline"
          size="sm"
          className="self-start"
          onClick={remove}
          aria-label={`Delete ${def.label} keyframe`}
        >
          <Trash2Icon />
          Delete keyframe
        </Button>
      ) : null}
    </div>
  );
}
