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
import { createProject } from "@/modules/workspace/lib/lifecycle";
import { useWorkspaceStore } from "@/modules/workspace/store";

const PRESETS = [
  { value: "1080x1920", label: "Vertical · 1080 × 1920" },
  { value: "1920x1080", label: "Landscape · 1920 × 1080" },
  { value: "1080x1080", label: "Square · 1080 × 1080" },
  { value: "2160x3840", label: "Vertical 4K · 2160 × 3840" },
  { value: "3840x2160", label: "Landscape 4K · 3840 × 2160" },
];

const FRAME_RATES = ["24", "25", "30", "50", "60"];

interface NewProjectDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function NewProjectDialog({ open, onOpenChange }: NewProjectDialogProps) {
  const settings = useWorkspaceStore((s) => s.settings);

  const [name, setName] = useState("Untitled");
  const [preset, setPreset] = useState(PRESETS[0].value);
  const [fps, setFps] = useState("30");
  const [busy, setBusy] = useState(false);

  // Adopt the defaults each time it opens rather than once at mount: settings
  // can have been changed in between, and a dialog showing yesterday's default
  // is how a preference silently stops meaning anything.
  useEffect(() => {
    if (!open) return;
    setPreset(`${settings.default_canvas[0]}x${settings.default_canvas[1]}`);
    setFps(String(settings.default_fps));
  }, [open, settings.default_canvas, settings.default_fps]);

  const create = async () => {
    const [width, height] = preset.split("x").map(Number);
    setBusy(true);
    try {
      const made = await createProject({
        name: name.trim() || "Untitled",
        width,
        height,
        fps: Number(fps),
      });
      if (made) onOpenChange(false);
    } finally {
      setBusy(false);
    }
  };

  const presets = PRESETS.some((option) => option.value === preset)
    ? PRESETS
    : // A default canvas set to something this list does not carry still has to
      // be selectable, or the trigger renders blank.
      [{ value: preset, label: preset.replace("x", " × ") }, ...PRESETS];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New project</DialogTitle>
          <DialogDescription>
            The canvas and frame rate can be changed later; clips keep their normalized transforms
            across a resize.
          </DialogDescription>
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
            <Select value={preset} onValueChange={setPreset}>
              <SelectTrigger aria-label="Canvas">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {presets.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

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
        </DialogBody>

        <DialogFooter>
          <Button variant="outline" size="md" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="default" size="md" onClick={create} disabled={busy}>
            Create
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
