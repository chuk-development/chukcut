import type React from "react";

import { cn } from "@/lib/utils";

interface PanelDividerProps {
  /** `vertical` is a vertical line dragged left/right. */
  orientation: "vertical" | "horizontal";
  onResize: (delta: number) => void;
}

const KEYBOARD_STEP = 16;

/**
 * A divider is 1px of visible border with a 5px grab strip on top of it, so it
 * looks like a hairline and behaves like a handle.
 */
export function PanelDivider({ orientation, onResize }: PanelDividerProps) {
  const vertical = orientation === "vertical";

  const handlePointerDown = (event: React.PointerEvent) => {
    event.preventDefault();
    let last = vertical ? event.clientX : event.clientY;

    const onMove = (moveEvent: PointerEvent) => {
      const current = vertical ? moveEvent.clientX : moveEvent.clientY;
      onResize(current - last);
      last = current;
    };
    const onUp = () => window.removeEventListener("pointermove", onMove);

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp, { once: true });
  };

  const handleKeyDown = (event: React.KeyboardEvent) => {
    const decrease = vertical ? "ArrowLeft" : "ArrowUp";
    const increase = vertical ? "ArrowRight" : "ArrowDown";
    if (event.key === decrease) {
      event.preventDefault();
      onResize(-KEYBOARD_STEP);
    } else if (event.key === increase) {
      event.preventDefault();
      onResize(KEYBOARD_STEP);
    }
  };

  return (
    <button
      type="button"
      data-slot="panel-divider"
      aria-label={vertical ? "Resize panel width" : "Resize panel height"}
      onPointerDown={handlePointerDown}
      onKeyDown={handleKeyDown}
      className={cn(
        "group relative z-10 shrink-0 border-none bg-border p-0 outline-none",
        vertical ? "w-px cursor-col-resize" : "h-px cursor-row-resize",
      )}
    >
      <span
        className={cn(
          "absolute bg-primary/0 transition-colors group-hover:bg-primary/60 group-focus-visible:bg-primary",
          vertical ? "-inset-x-[3px] inset-y-0" : "-inset-y-[3px] inset-x-0",
        )}
      />
    </button>
  );
}
