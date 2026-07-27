/**
 * The crop panel: how much to take off each edge, as fractions.
 *
 * The kept region scales to fill the clip's frame (crop first, then fit, then
 * the segment's own transform — `layout::place_quad` owns that order), so
 * these four sliders never leave a hole; they re-frame. Values are *insets*
 * here and the kept rectangle in the document; `adjust.ts` translates.
 */

import { RotateCcwIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { PropertySlider } from "@/modules/inspector/components/PropertySlider";
import {
  type CropInsets,
  cropFromInsets,
  insetsOf,
  MAX_INSET,
  runSetCrop,
} from "@/modules/inspector/lib/adjust";
import type { Segment } from "@/modules/project/types";

const percent = (value: number) => `${Math.round(value * 100)}%`;

const EDGES: { key: keyof CropInsets; label: string }[] = [
  { key: "left", label: "Crop left" },
  { key: "right", label: "Crop right" },
  { key: "top", label: "Crop top" },
  { key: "bottom", label: "Crop bottom" },
];

export function CropSection({ segment }: { segment: Segment }) {
  const insets = insetsOf(segment.crop);

  const commit = (key: keyof CropInsets, value: number) => {
    void runSetCrop(segment.id, cropFromInsets({ ...insets, [key]: value }));
  };

  return (
    <>
      {EDGES.map(({ key, label }) => (
        <PropertySlider
          key={key}
          label={label}
          value={insets[key]}
          min={0}
          max={MAX_INSET}
          step={0.01}
          format={percent}
          onCommit={(value) => commit(key, value)}
        />
      ))}
      <div className="flex items-center pt-0.5">
        <Button
          size="sm"
          className="ml-auto"
          disabled={segment.crop === null}
          onClick={() => void runSetCrop(segment.id, null)}
        >
          <RotateCcwIcon />
          Reset crop
        </Button>
      </div>
    </>
  );
}
