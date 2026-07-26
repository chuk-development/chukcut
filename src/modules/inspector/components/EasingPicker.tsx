import { cn } from "@/lib/utils";
import { easingApply } from "@/modules/inspector/lib/keyframes";
import type { Easing } from "@/modules/project/types";

/**
 * The five easings, drawn.
 *
 * Each one is its own curve — progress against time, the same function the
 * renderer applies — because "ease in-out" is not a thing anyone can pick from
 * a list of words. A curve that starts flat and ends steep is a thing you can
 * point at.
 */
const OPTIONS: { easing: Easing; label: string; description: string }[] = [
  {
    easing: "hold",
    label: "Hold",
    description: "No motion at all: the value steps to the next keyframe when it arrives.",
  },
  { easing: "linear", label: "Linear", description: "Constant speed the whole way." },
  { easing: "ease_in", label: "In", description: "Leaves this keyframe slowly, arrives fast." },
  { easing: "ease_out", label: "Out", description: "Leaves fast, settles into the next keyframe." },
  {
    easing: "ease_in_out",
    label: "In-out",
    description: "Slow at both ends, quickest in the middle.",
  },
];

const WIDTH = 40;
const HEIGHT = 26;
const PAD = 3;

/** The easing as a path: horizontal is time, vertical is how far the value has travelled. */
function easingPath(easing: Easing): string {
  const x = (t: number) => PAD + t * (WIDTH - 2 * PAD);
  const y = (progress: number) => HEIGHT - PAD - progress * (HEIGHT - 2 * PAD);

  if (easing === "hold") {
    // `Easing::Hold` returns zero progress for every t, so the jump happens at
    // the next keyframe rather than along the way. Drawn as the step it is.
    return `M ${x(0)} ${y(0)} L ${x(1)} ${y(0)} L ${x(1)} ${y(1)}`;
  }

  const steps = 16;
  const points: string[] = [];
  for (let i = 0; i <= steps; i++) {
    const t = i / steps;
    points.push(`${x(t).toFixed(2)} ${y(easingApply(easing, t)).toFixed(2)}`);
  }
  return `M ${points.join(" L ")}`;
}

export interface EasingPickerProps {
  value: Easing;
  onChange: (easing: Easing) => void;
  disabled?: boolean;
  /** Why the choice does not matter here — shown instead of the description. */
  note?: string;
}

export function EasingPicker({ value, onChange, disabled, note }: EasingPickerProps) {
  const active = OPTIONS.find((option) => option.easing === value) ?? OPTIONS[1];

  return (
    <div data-slot="easing-picker" className="flex flex-col gap-1.5">
      <div className="grid grid-cols-5 gap-1">
        {OPTIONS.map((option) => (
          <button
            key={option.easing}
            type="button"
            aria-label={`Easing ${option.label}`}
            aria-pressed={option.easing === value}
            data-active={option.easing === value}
            disabled={disabled}
            title={option.description}
            onClick={() => onChange(option.easing)}
            className={cn(
              "flex flex-col items-center gap-0.5 rounded-sm border border-border/60 bg-surface/40 px-0.5 py-1",
              "text-[9px] leading-none text-muted-foreground transition-colors",
              "hover:bg-accent hover:text-accent-foreground",
              "data-[active=true]:border-primary/70 data-[active=true]:bg-accent data-[active=true]:text-foreground",
              "disabled:pointer-events-none disabled:opacity-40",
            )}
          >
            <svg
              viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
              className="w-full"
              role="presentation"
              aria-hidden="true"
            >
              <path
                d={easingPath(option.easing)}
                fill="none"
                stroke="currentColor"
                strokeWidth={1.5}
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
            {option.label}
          </button>
        ))}
      </div>
      <p className="text-[10px] leading-snug text-muted-foreground/80">
        {note ?? active.description}
      </p>
    </div>
  );
}
