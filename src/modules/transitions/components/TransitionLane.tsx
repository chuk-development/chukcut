/**
 * Every transition on one lane, drawn over the clips.
 *
 * One component per lane rather than one per timeline, so that a lane that has
 * no cuts on it renders nothing at all and a lane that does re-renders only
 * when its own segments move.
 *
 * The timeline owns the lane's geometry, so this takes the two things only it
 * knows — the zoom and how to turn a client x into an instant — and asks for
 * nothing else. Dropping it into a lane is one element:
 *
 * ```tsx
 * <TransitionLane
 *   project={project}
 *   track={track}
 *   zoom={zoom}
 *   laneHeight={trackHeight(track.kind)}
 *   timeAtClientX={timeAtClientX}
 * />
 * ```
 */

import { memo } from "react";

import type { Micros, Project, Track } from "@/modules/project/types";
import { TransitionMarker } from "@/modules/transitions/components/TransitionMarker";
import { joins } from "@/modules/transitions/lib/geometry";

export interface TransitionLaneProps {
  project: Project;
  track: Track;
  zoom: number;
  laneHeight: number;
  timeAtClientX: (clientX: number) => Micros;
}

/**
 * Which lanes get transitions at all.
 *
 * The renderer only composites video-bearing lanes, so a transition on an audio
 * lane would be a control that changes nothing. Audio crossfades are a separate
 * feature and a separate shape — they belong to the mixer, not to the
 * compositor.
 */
function laneTakesTransitions(track: Track): boolean {
  return track.kind !== "audio";
}

export const TransitionLane = memo(function TransitionLane({
  project,
  track,
  zoom,
  laneHeight,
  timeAtClientX,
}: TransitionLaneProps) {
  if (!laneTakesTransitions(track) || track.locked) return null;
  const cuts = joins(project, track);
  if (cuts.length === 0) return null;

  return (
    <>
      {cuts.map((join) => (
        <TransitionMarker
          key={join.to.id}
          join={join}
          zoom={zoom}
          laneHeight={laneHeight}
          timeAtClientX={timeAtClientX}
        />
      ))}
    </>
  );
});
