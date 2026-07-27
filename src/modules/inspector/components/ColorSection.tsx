/**
 * The colour panel: brightness, contrast, saturation, temperature, a .cube
 * LUT — and opacity, which belongs with them in the panel but lives on the
 * transform in the document (it scales blend coverage, not colour, and it was
 * keyframable long before grading existed). The caller renders the opacity
 * row alongside; this component owns the grade sliders and the LUT row.
 *
 * Each release commits the whole grade with one field changed. Rust mints a
 * fresh material per commit and swaps the segment's reference, so every
 * commit is one undo step and identity values clear the grade entirely —
 * which is why Reset just sends `null`.
 *
 * The LUT applies *after* the sliders (grade first, look second — the order
 * `quad.wgsl` implements), and its file is validated by `inspector_lut_probe`
 * at pick time, so a malformed .cube is refused with the parser's
 * line-numbered message before it ever reaches the document.
 */

import { RotateCcwIcon, XIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { openFileDialog } from "@/lib/dialog";
import { PropertySlider } from "@/modules/inspector/components/PropertySlider";
import {
  type ColorEdit,
  colorAdjustOf,
  IDENTITY_COLOR,
  inspectorLutProbe,
  runSetColor,
} from "@/modules/inspector/lib/adjust";
import { describeError, useProjectStore } from "@/modules/project/store";
import { basename, type Project, type Segment } from "@/modules/project/types";

const signedPercent = (value: number) => {
  const rounded = Math.round(value * 100);
  return rounded > 0 ? `+${rounded}%` : `${rounded}%`;
};
const percent = (value: number) => `${Math.round(value * 100)}%`;

interface SliderDef {
  key: "brightness" | "contrast" | "saturation" | "temperature";
  label: string;
  min: number;
  max: number;
  format: (value: number) => string;
}

const SLIDERS: SliderDef[] = [
  { key: "brightness", label: "Brightness", min: -1, max: 1, format: signedPercent },
  { key: "contrast", label: "Contrast", min: 0, max: 2, format: percent },
  { key: "saturation", label: "Saturation", min: 0, max: 2, format: percent },
  { key: "temperature", label: "Temperature", min: -1, max: 1, format: signedPercent },
];

const LUT_FILTERS = [{ name: "Cube LUT", extensions: ["cube", "CUBE"] }];

export function ColorSection({ project, segment }: { project: Project; segment: Segment }) {
  const applied = colorAdjustOf(project, segment);
  const current: ColorEdit = applied
    ? {
        brightness: applied.brightness,
        contrast: applied.contrast,
        saturation: applied.saturation,
        temperature: applied.temperature,
        lut: applied.lut,
      }
    : IDENTITY_COLOR;

  const commit = (patch: Partial<ColorEdit>) => {
    void runSetColor(segment.id, { ...current, ...patch });
  };

  const pickLut = async () => {
    const [path] = await openFileDialog({ title: "Choose a LUT", filters: LUT_FILTERS });
    if (!path) return;
    try {
      // Validated before it touches the document, so a malformed file is
      // refused here with the parser's line-numbered message.
      await inspectorLutProbe(path);
    } catch (error) {
      useProjectStore.getState().setError(describeError(error));
      return;
    }
    commit({ lut: { path, intensity: 1 } });
  };

  return (
    <>
      {SLIDERS.map(({ key, label, min, max, format }) => (
        <PropertySlider
          key={key}
          label={label}
          value={current[key]}
          min={min}
          max={max}
          step={0.01}
          format={format}
          onCommit={(value) => commit({ [key]: value })}
        />
      ))}

      <div className="grid grid-cols-[76px_1fr_auto] items-center gap-2">
        <span className="truncate text-[11px] text-muted-foreground">LUT</span>
        {current.lut ? (
          <>
            <span
              className="truncate font-mono text-[11px] text-foreground/85"
              title={current.lut.path}
            >
              {basename(current.lut.path)}
            </span>
            <Button
              size="sm"
              variant="ghost"
              aria-label="Remove LUT"
              onClick={() => commit({ lut: null })}
            >
              <XIcon />
            </Button>
          </>
        ) : (
          <Button
            size="sm"
            className="col-span-2 justify-self-start"
            onClick={() => void pickLut()}
          >
            Choose LUT…
          </Button>
        )}
      </div>
      {current.lut ? (
        <PropertySlider
          label="LUT intensity"
          value={current.lut.intensity}
          min={0}
          max={1}
          step={0.01}
          format={percent}
          onCommit={(value) =>
            commit({ lut: current.lut ? { ...current.lut, intensity: value } : null })
          }
        />
      ) : null}

      <div className="flex items-center pt-0.5">
        <Button
          size="sm"
          className="ml-auto"
          disabled={applied === null}
          onClick={() => void runSetColor(segment.id, null)}
        >
          <RotateCcwIcon />
          Reset colour
        </Button>
      </div>
    </>
  );
}
