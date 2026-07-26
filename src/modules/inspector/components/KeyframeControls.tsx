import { ChevronLeftIcon, ChevronRightIcon, DiamondIcon } from "lucide-react";

import { Button } from "@/components/ui/button";

/**
 * The stopwatch, and the two arrows either side of it.
 *
 * One diamond does three jobs, which is the convention every editor shares:
 * hollow means the property is static and clicking starts animating it, filled
 * means the playhead is sitting on a keyframe and clicking removes it, and
 * outlined-but-lit means the property is animated and the playhead is between
 * keyframes, where clicking drops a new one at the sampled value.
 *
 * The arrows only appear once there is something to navigate to, but the group
 * keeps its width either way — a control that reflows the slider next to it
 * every time you keyframe something is unusable.
 */
export interface KeyframeControlsProps {
  /** Property name, for the accessible labels. */
  label: string;
  animated: boolean;
  /** The playhead is on a keyframe, within half a frame. */
  onKeyframe: boolean;
  /** The playhead is inside the clip; outside it there is nothing to keyframe. */
  canKeyframe: boolean;
  hasPrevious: boolean;
  hasNext: boolean;
  onToggle: () => void;
  onPrevious: () => void;
  onNext: () => void;
}

export function KeyframeControls({
  label,
  animated,
  onKeyframe,
  canKeyframe,
  hasPrevious,
  hasNext,
  onToggle,
  onPrevious,
  onNext,
}: KeyframeControlsProps) {
  const toggleLabel = !animated
    ? `Animate ${label}`
    : onKeyframe
      ? `Remove ${label} keyframe`
      : `Add ${label} keyframe`;

  return (
    <div
      data-slot="keyframe-controls"
      data-animated={animated}
      className="flex w-[56px] shrink-0 items-center justify-center"
    >
      {animated ? (
        <Button
          variant="ghost"
          size="icon-sm"
          className="w-[17px] px-0 text-muted-foreground disabled:opacity-25"
          aria-label={`Previous ${label} keyframe`}
          disabled={!hasPrevious}
          onClick={onPrevious}
        >
          <ChevronLeftIcon />
        </Button>
      ) : (
        <span className="w-[17px]" />
      )}

      <Button
        variant="toggle"
        size="icon-sm"
        aria-label={toggleLabel}
        title={canKeyframe ? toggleLabel : "Move the playhead over this clip to add a keyframe"}
        aria-pressed={animated}
        data-active={animated}
        data-on-keyframe={onKeyframe}
        disabled={!canKeyframe}
        className={onKeyframe ? "text-foreground [&_svg]:fill-current" : undefined}
        onClick={onToggle}
      >
        <DiamondIcon />
      </Button>

      {animated ? (
        <Button
          variant="ghost"
          size="icon-sm"
          className="w-[17px] px-0 text-muted-foreground disabled:opacity-25"
          aria-label={`Next ${label} keyframe`}
          disabled={!hasNext}
          onClick={onNext}
        >
          <ChevronRightIcon />
        </Button>
      ) : (
        <span className="w-[17px]" />
      )}
    </div>
  );
}
