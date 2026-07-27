import { useCallback } from "react";

import { clamp } from "@/lib/time";
import { KeyframeControls } from "@/modules/inspector/components/KeyframeControls";
import { PropertySlider } from "@/modules/inspector/components/PropertySlider";
import {
  forProperties,
  type KeyframeEditCommand,
  runKeyframeEdit,
} from "@/modules/inspector/lib/commands";
import {
  easingForInsertion,
  findTrack,
  keyframeAt,
  keyframeTolerance,
  nextKeyframe,
  previousKeyframe,
  propertyValue,
  relativeTime,
  snapToFrame,
  staticValue,
  timelineTime,
} from "@/modules/inspector/lib/keyframes";
import type { PropertyDef } from "@/modules/inspector/lib/properties";
import { useInspectorStore } from "@/modules/inspector/store";
import { runEdit } from "@/modules/project/store";
import type { Project, Segment } from "@/modules/project/types";
import { timelineApply } from "@/modules/timeline/lib/api";
import { useTimelineStore } from "@/modules/timeline/store";

export interface KeyframeRowProps {
  project: Project;
  segment: Segment;
  def: PropertyDef;
  disabled?: boolean;
}

/**
 * One property: its keyframe controls, its slider, its readout.
 *
 * The rule this row exists to get right is what a value change *means*. While
 * the property is static it means what it always meant — a `set_transform` or
 * `set_volume` on the document. The moment it is animated, the same gesture
 * means "the property is this at the playhead", which is a keyframe: the one at
 * the playhead if there is one, a new one if the playhead is between two. An
 * editor that wrote the static value instead would appear to do nothing,
 * because the animation overrides it on the very next frame.
 */
export function KeyframeRow({ project, segment, def, disabled }: KeyframeRowProps) {
  // From the store, not a prop — see KeyframeEditor for why.
  const playhead = useTimelineStore((s) => s.playhead);
  const showCurve = useInspectorStore((s) => s.showCurve);
  const setPlayhead = useTimelineStore((s) => s.setPlayhead);

  const tolerance = keyframeTolerance(project.fps);
  const relative = relativeTime(segment, playhead);
  const duration = segment.target_range.duration;
  const inside = relative >= 0 && relative < duration;

  const track = findTrack(segment, def.properties[0]);
  const animated = track !== null && track.keyframes.length > 0;
  const current = keyframeAt(track, relative, tolerance);
  const value = animated ? propertyValue(segment, def.properties[0], relative) : def.read(segment);

  // A keyframe lands on the frame grid. The playhead can sit between frames
  // after a click on the ruler, and a keyframe half a frame off the frame it
  // was authored against is a keyframe nobody can hit again.
  const insertTime = snapToFrame(clamp(relative, 0, duration), project.fps);

  const commit = useCallback(
    (next: number) => {
      if (disabled) return;

      if (!animated) {
        void runEdit(() => timelineApply(def.staticCommand(segment, next)));
        return;
      }

      const at = keyframeAt(findTrack(segment, def.properties[0]), relative, tolerance);
      const command = forProperties(
        at ? "Set keyframe" : "Add keyframe",
        def.properties,
        (property): KeyframeEditCommand => {
          const own = keyframeAt(findTrack(segment, property), relative, tolerance);
          if (own) {
            return {
              type: "move_keyframe",
              segment_id: segment.id,
              property,
              from_time: own.time,
              to_time: own.time,
              before_value: own.value,
              after_value: next,
            };
          }
          return {
            type: "add_keyframe",
            segment_id: segment.id,
            property,
            keyframe: {
              time: insertTime,
              value: next,
              easing: easingForInsertion(findTrack(segment, property), insertTime),
            },
          };
        },
      );
      void runKeyframeEdit(command);
    },
    [animated, def, disabled, insertTime, relative, segment, tolerance],
  );

  const toggle = useCallback(() => {
    if (disabled || !inside) return;

    if (current) {
      void runKeyframeEdit(
        forProperties("Delete keyframe", def.properties, (property) => {
          const own = keyframeAt(findTrack(segment, property), relative, tolerance) ?? current;
          return { type: "remove_keyframe", segment_id: segment.id, property, keyframe: own };
        }),
      );
      return;
    }

    showCurve(def.id);
    void runKeyframeEdit(
      forProperties("Add keyframe", def.properties, (property) => ({
        type: "add_keyframe",
        segment_id: segment.id,
        property,
        keyframe: {
          time: insertTime,
          value: animated
            ? propertyValue(segment, property, relative)
            : staticValue(segment, property),
          easing: easingForInsertion(findTrack(segment, property), insertTime),
        },
      })),
    );
  }, [
    animated,
    current,
    def,
    disabled,
    insertTime,
    inside,
    relative,
    segment,
    showCurve,
    tolerance,
  ]);

  const previous = previousKeyframe(track, relative, tolerance);
  const next = nextKeyframe(track, relative, tolerance);

  return (
    <PropertySlider
      label={def.label}
      value={value}
      min={def.min}
      max={def.max}
      step={def.step}
      format={def.format}
      // While animated the playhead has to be over the clip for a value change
      // to have a time to belong to.
      disabled={disabled || (animated && !inside)}
      onCommit={commit}
      leading={
        <KeyframeControls
          label={def.label}
          animated={animated}
          onKeyframe={current !== null}
          canKeyframe={!disabled && inside}
          hasPrevious={previous !== null}
          hasNext={next !== null}
          onToggle={toggle}
          onPrevious={() => {
            if (previous) setPlayhead(timelineTime(segment, previous.time));
          }}
          onNext={() => {
            if (next) setPlayhead(timelineTime(segment, next.time));
          }}
        />
      }
    />
  );
}
