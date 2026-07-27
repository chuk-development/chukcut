/**
 * File → Project Settings: the *document's* settings, as opposed to Ctrl+,
 * which is the application's.
 *
 * Everything here commits as one `project_configure` call — one undoable step
 * on the same stack as the timeline edits, labelled "Project settings" in the
 * Edit menu. The dialog edits a draft and writes nothing until Apply, so
 * Cancel is exact and free.
 */

import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { projectConfigure } from "@/modules/project/lib/api";
import { runEdit, useProjectStore } from "@/modules/project/store";
import type { Rgba } from "@/modules/project/types";

/**
 * The canvas shapes worth one click. "Custom" is the escape hatch, so an odd
 * source of truth — a client's 1080×1350 brief, a 4K master — is typed rather
 * than hunted for.
 */
const CANVAS_PRESETS = [
  { value: "1080x1920", label: "Vertical · 1080 × 1920" },
  { value: "1920x1080", label: "Landscape · 1920 × 1080" },
  { value: "1080x1350", label: "Portrait post · 1080 × 1350" },
  { value: "1080x1080", label: "Square · 1080 × 1080" },
  { value: "2160x3840", label: "Vertical 4K · 2160 × 3840" },
  { value: "3840x2160", label: "Landscape 4K · 3840 × 2160" },
];

const CUSTOM = "custom";

const FRAME_RATES = ["24", "25", "30", "50", "60"];

// ---------------------------------------------------------------------------
// Colour: the document stores linear RGBA 0..1, a colour input speaks sRGB hex.
// The sRGB transfer function, not a bare power curve, because the compositor
// blends the background in linear light and an approximate gamma would make
// the picked swatch and the rendered canvas visibly disagree on mid greys.
// ---------------------------------------------------------------------------

function srgbToLinear(channel: number): number {
  return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
}

function linearToSrgb(channel: number): number {
  const c = Math.min(1, Math.max(0, channel));
  return c <= 0.0031308 ? c * 12.92 : 1.055 * c ** (1 / 2.4) - 0.055;
}

/** `#rrggbb` for a colour input, from the document's linear RGBA. */
export function hexOfBackground(background: Rgba): string {
  const byte = (linear: number) =>
    Math.round(linearToSrgb(linear) * 255)
      .toString(16)
      .padStart(2, "0");
  return `#${byte(background[0])}${byte(background[1])}${byte(background[2])}`;
}

/** The document's linear RGBA from a colour input's `#rrggbb`. Always opaque. */
export function backgroundOfHex(hex: string): Rgba {
  const channel = (at: number) => srgbToLinear(Number.parseInt(hex.slice(at, at + 2), 16) / 255);
  return [channel(1), channel(3), channel(5), 1];
}

interface ProjectSettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ProjectSettingsDialog({ open, onOpenChange }: ProjectSettingsDialogProps) {
  const project = useProjectStore((s) => s.project);

  const [name, setName] = useState("");
  const [preset, setPreset] = useState<string>(CANVAS_PRESETS[0].value);
  const [width, setWidth] = useState("1080");
  const [height, setHeight] = useState("1920");
  const [fps, setFps] = useState("30");
  const [hex, setHex] = useState("#000000");
  const [busy, setBusy] = useState(false);

  // Re-read the document each time the dialog opens: it can have been edited,
  // undone into, or replaced since the last look, and a dialog showing the
  // previous visit's values would write them back as an edit.
  useEffect(() => {
    if (!open || !project) return;
    const size = `${project.canvas.width}x${project.canvas.height}`;
    setName(project.name);
    setPreset(CANVAS_PRESETS.some((p) => p.value === size) ? size : CUSTOM);
    setWidth(String(project.canvas.width));
    setHeight(String(project.canvas.height));
    setFps(String(project.fps));
    setHex(hexOfBackground(project.canvas.background));
  }, [open, project]);

  if (!project) return null;

  const choosePreset = (value: string) => {
    setPreset(value);
    if (value !== CUSTOM) {
      const [w, h] = value.split("x");
      setWidth(w);
      setHeight(h);
    }
  };

  const parsedWidth = Number.parseInt(width, 10);
  const parsedHeight = Number.parseInt(height, 10);
  const parsedFps = Number.parseFloat(fps);
  const sizeValid =
    Number.isFinite(parsedWidth) &&
    Number.isFinite(parsedHeight) &&
    parsedWidth >= 2 &&
    parsedHeight >= 2 &&
    parsedWidth % 2 === 0 &&
    parsedHeight % 2 === 0;

  const apply = async () => {
    setBusy(true);
    try {
      const done = await runEdit(() =>
        projectConfigure({
          name: name.trim() || project.name,
          width: parsedWidth,
          height: parsedHeight,
          fps: parsedFps,
          background: backgroundOfHex(hex),
        }),
      );
      if (done) onOpenChange(false);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Project settings</DialogTitle>
          <DialogDescription>One change, one undo step.</DialogDescription>
        </DialogHeader>

        <DialogBody className="flex flex-col gap-3">
          <div className="grid grid-cols-[88px_1fr] items-center gap-3 text-[12px]">
            <span className="text-muted-foreground">Name</span>
            <Input
              aria-label="Project name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              autoFocus
            />
          </div>

          <div className="grid grid-cols-[88px_1fr] items-center gap-3 text-[12px]">
            <span className="text-muted-foreground">Canvas</span>
            <Select value={preset} onValueChange={choosePreset}>
              <SelectTrigger aria-label="Canvas preset">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {CANVAS_PRESETS.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
                <SelectItem value={CUSTOM}>Custom…</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className="grid grid-cols-[88px_1fr] items-center gap-3 text-[12px]">
            <span className="text-muted-foreground">Size</span>
            <div className="flex items-center gap-2">
              <Input
                aria-label="Canvas width"
                inputMode="numeric"
                className="w-20"
                value={width}
                onChange={(event) => {
                  setWidth(event.target.value);
                  setPreset(CUSTOM);
                }}
              />
              <span className="text-muted-foreground">×</span>
              <Input
                aria-label="Canvas height"
                inputMode="numeric"
                className="w-20"
                value={height}
                onChange={(event) => {
                  setHeight(event.target.value);
                  setPreset(CUSTOM);
                }}
              />
              <span className="text-muted-foreground">px</span>
            </div>
          </div>
          {!sizeValid ? (
            <p className="ml-[100px] text-[11px] text-destructive-foreground">
              Width and height must be even numbers of at least 2 — every video encoder requires
              even dimensions.
            </p>
          ) : null}

          <div className="grid grid-cols-[88px_1fr] items-center gap-3 text-[12px]">
            <span className="text-muted-foreground">Frame rate</span>
            <Select value={fps} onValueChange={setFps}>
              <SelectTrigger aria-label="Frame rate">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {(FRAME_RATES.includes(fps) ? FRAME_RATES : [fps, ...FRAME_RATES]).map((rate) => (
                  <SelectItem key={rate} value={rate}>
                    {rate} fps
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="grid grid-cols-[88px_1fr] items-center gap-3 text-[12px]">
            <span className="text-muted-foreground">Background</span>
            <div className="flex items-center gap-2">
              <input
                type="color"
                aria-label="Canvas background colour"
                value={hex}
                onChange={(event) => setHex(event.target.value)}
                className="h-7 w-10 cursor-pointer rounded-sm border border-border bg-transparent p-0.5"
              />
              <span className="font-mono text-[11px] text-muted-foreground">{hex}</span>
            </div>
          </div>

          {/* Times in the document are microseconds, never frames, so the rate
              is presentation and export only. Saying so here is what stops
              "change to 60 fps" from reading as a re-time. */}
          <p className="rounded-sm border border-border bg-surface px-2.5 py-2 text-[11px] leading-relaxed text-muted-foreground">
            Changing the frame rate re-times nothing: clips keep their exact positions and
            durations, and the timeline simply renders and exports at the new rate.
          </p>
        </DialogBody>

        <DialogFooter>
          <Button variant="outline" size="md" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="default" size="md" onClick={apply} disabled={busy || !sizeValid}>
            Apply
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
