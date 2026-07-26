import { type ReactNode, useEffect, useState } from "react";

import { Slider } from "@/components/ui/slider";
import { cn } from "@/lib/utils";

interface PropertySliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  format?: (value: number) => string;
  /** Called once, on release. Dragging is local state and never reaches Rust. */
  onCommit: (value: number) => void;
  disabled?: boolean;
  /** Keyframe controls, in a column of their own ahead of the label. */
  leading?: ReactNode;
}

export function PropertySlider({
  label,
  value,
  min,
  max,
  step,
  format,
  onCommit,
  disabled,
  leading,
}: PropertySliderProps) {
  const [local, setLocal] = useState(value);
  const [dragging, setDragging] = useState(false);

  // While the thumb is down the document is stale by design; adopt its value
  // again the moment the gesture ends so an undo is reflected here.
  useEffect(() => {
    if (!dragging) setLocal(value);
  }, [value, dragging]);

  return (
    <div
      data-slot="property-slider"
      className={cn(
        "grid items-center gap-2",
        leading ? "grid-cols-[auto_66px_1fr_44px]" : "grid-cols-[76px_1fr_48px]",
      )}
    >
      {leading}
      <span className="truncate text-[11px] text-muted-foreground">{label}</span>
      <Slider
        aria-label={label}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        value={[local]}
        onValueChange={([next]) => {
          setDragging(true);
          setLocal(next);
        }}
        onValueCommit={([next]) => {
          setDragging(false);
          setLocal(next);
          if (next !== value) onCommit(next);
        }}
      />
      <span className="text-right font-mono text-[11px] tabular-nums text-foreground/80">
        {format ? format(local) : local.toFixed(2)}
      </span>
    </div>
  );
}
