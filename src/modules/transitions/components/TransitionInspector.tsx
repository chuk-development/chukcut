/**
 * The parameter panel for one transition.
 *
 * Which controls appear is decided by the **catalogue**, not by a table here: a
 * dissolve has no direction and never will, a wipe has softness, and those are
 * properties of the shader. Rust ships the flags with the list, so adding a
 * kind to `TransitionKind` puts it in the picker with the right controls
 * without this file changing.
 *
 * Every control commits on release — one edit, one undo step — the same
 * arrangement `PropertySlider` was built for. The whole material is sent each
 * time, through `transitions_set`, because that is the command's shape: it
 * replaces the parameters wholesale and keeps the transition where it is.
 */

import { useEffect } from "react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { MICROS_PER_SECOND } from "@/lib/time";
import { EasingPicker } from "@/modules/inspector/components/EasingPicker";
import { PropertySlider } from "@/modules/inspector/components/PropertySlider";
import type {
  Id,
  Project,
  TransitionDirection,
  TransitionKind,
  TransitionMaterial,
} from "@/modules/project/types";
import { removeTransition, setTransition } from "@/modules/transitions/lib/edits";
import { joinAt } from "@/modules/transitions/lib/geometry";
import { useTransitionStore } from "@/modules/transitions/store";

const DIRECTIONS: { value: TransitionDirection; label: string }[] = [
  { value: "left", label: "Left" },
  { value: "right", label: "Right" },
  { value: "up", label: "Up" },
  { value: "down", label: "Down" },
];

export interface TransitionInspectorProps {
  project: Project;
  /** The segment the transition is the entrance to. */
  segmentId: Id;
}

export function TransitionInspector({ project, segmentId }: TransitionInspectorProps) {
  const catalog = useTransitionStore((state) => state.catalog);
  const loadCatalog = useTransitionStore((state) => state.loadCatalog);

  useEffect(() => {
    void loadCatalog();
  }, [loadCatalog]);

  const join = joinAt(project, segmentId);
  const transition = join?.transition;
  if (!join || !transition) return null;

  const descriptor = catalog.find((entry) => entry.kind === transition.kind);
  const commit = (changes: Partial<TransitionMaterial>) =>
    void setTransition(segmentId, { ...transition, ...changes });

  // The window is clamped into the two clips, so the slider's ceiling is what
  // they can carry rather than a round number. A transition longer than that is
  // legal in the document and renders asymmetrically — the slider simply does
  // not offer it, because a control that silently does nothing at its top end
  // is worse than one that stops.
  const maxSeconds = Math.max(0.1, join.max / MICROS_PER_SECOND);

  return (
    <section data-slot="transition-inspector" className="flex flex-col gap-3 p-3">
      <header className="flex items-center justify-between gap-2">
        <h2 className="text-[11px] font-medium text-panel-foreground">Transition</h2>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void removeTransition(segmentId)}
          aria-label="Remove transition"
        >
          Remove
        </Button>
      </header>

      <Select
        value={transition.kind}
        onValueChange={(kind) => commit({ kind: kind as TransitionKind })}
      >
        <SelectTrigger aria-label="Transition type">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {catalog.map((entry) => (
            <SelectItem key={entry.kind} value={entry.kind}>
              {entry.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {descriptor ? (
        <p className="text-[11px] text-muted-foreground">{descriptor.description}</p>
      ) : null}

      <PropertySlider
        label="Duration"
        value={transition.duration / MICROS_PER_SECOND}
        min={0.05}
        max={maxSeconds}
        step={0.05}
        format={(value) => `${value.toFixed(2)}s`}
        onCommit={(value) => commit({ duration: Math.round(value * MICROS_PER_SECOND) })}
      />

      {descriptor?.directional ? (
        <Select
          value={transition.direction}
          onValueChange={(direction) => commit({ direction: direction as TransitionDirection })}
        >
          <SelectTrigger aria-label="Direction">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {DIRECTIONS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : null}

      {descriptor?.has_softness ? (
        <PropertySlider
          label="Softness"
          value={transition.softness}
          min={0}
          max={0.5}
          step={0.01}
          onCommit={(softness) => commit({ softness })}
        />
      ) : null}

      {descriptor?.has_zoom ? (
        <PropertySlider
          label="Zoom"
          value={transition.zoom}
          min={0}
          max={1}
          step={0.05}
          onCommit={(zoom) => commit({ zoom })}
        />
      ) : null}

      {descriptor?.has_color ? (
        <label className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
          Colour
          <input
            type="color"
            aria-label="Dip colour"
            className="h-6 w-10 rounded border border-border bg-transparent"
            value={hexOf(transition.color)}
            onChange={(event) =>
              commit({ color: [...rgbOf(event.target.value), transition.color[3]] })
            }
          />
        </label>
      ) : null}

      <EasingPicker value={transition.easing} onChange={(easing) => commit({ easing })} />
    </section>
  );
}

/**
 * The document's colours are **linear** 0..1 and an `<input type="color">` is
 * sRGB hex, so the two conversions are here rather than being a `* 255`.
 * Skipping them makes a mid grey picked in the panel come out visibly dark in
 * the dip, which is the kind of wrongness that survives review because it still
 * looks like a colour.
 */
function hexOf(color: readonly number[]): string {
  const channel = (linear: number) => {
    const encoded = linear <= 0.0031308 ? linear * 12.92 : 1.055 * linear ** (1 / 2.4) - 0.055;
    return Math.round(Math.min(1, Math.max(0, encoded)) * 255)
      .toString(16)
      .padStart(2, "0");
  };
  return `#${channel(color[0])}${channel(color[1])}${channel(color[2])}`;
}

function rgbOf(hex: string): [number, number, number] {
  const channel = (at: number) => {
    const encoded = Number.parseInt(hex.slice(at, at + 2), 16) / 255;
    return encoded <= 0.04045 ? encoded / 12.92 : ((encoded + 0.055) / 1.055) ** 2.4;
  };
  return [channel(1), channel(3), channel(5)];
}
