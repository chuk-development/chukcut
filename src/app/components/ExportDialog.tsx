import { InfoIcon } from "lucide-react";
import { useState } from "react";

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
import { formatTimecode } from "@/lib/time";
import { useProjectStore } from "@/modules/project/store";
import { projectDuration } from "@/modules/project/types";

const QUALITIES = [
  { value: "high", label: "High · ~16 Mbps" },
  { value: "medium", label: "Medium · ~8 Mbps" },
  { value: "low", label: "Low · ~4 Mbps" },
];

const CODECS = [
  { value: "h264", label: "H.264 / MP4" },
  { value: "hevc", label: "HEVC / MP4" },
  { value: "vp9", label: "VP9 / WebM" },
];

interface ExportDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * The export module does not exist yet, so this collects the settings and says
 * so plainly rather than pretending to start a job.
 */
export function ExportDialog({ open, onOpenChange }: ExportDialogProps) {
  const project = useProjectStore((s) => s.project);
  const [quality, setQuality] = useState("high");
  const [codec, setCodec] = useState("h264");
  const [fileName, setFileName] = useState("");

  const duration = project ? projectDuration(project) : 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Export</DialogTitle>
          <DialogDescription>
            {project
              ? `${project.canvas.width} × ${project.canvas.height} · ${project.fps.toFixed(2)} fps · ${formatTimecode(duration, project.fps)}`
              : "No project open"}
          </DialogDescription>
        </DialogHeader>

        <DialogBody className="flex flex-col gap-3">
          <div className="grid grid-cols-[88px_1fr] items-center gap-3 text-[12px]">
            <span className="text-muted-foreground">File name</span>
            <Input
              aria-label="File name"
              value={fileName}
              placeholder={project ? `${project.name}.mp4` : "export.mp4"}
              onChange={(event) => setFileName(event.target.value)}
            />
          </div>

          <div className="grid grid-cols-[88px_1fr] items-center gap-3 text-[12px]">
            <span className="text-muted-foreground">Codec</span>
            <Select value={codec} onValueChange={setCodec}>
              <SelectTrigger aria-label="Codec">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {CODECS.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="grid grid-cols-[88px_1fr] items-center gap-3 text-[12px]">
            <span className="text-muted-foreground">Quality</span>
            <Select value={quality} onValueChange={setQuality}>
              <SelectTrigger aria-label="Quality">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {QUALITIES.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <p className="flex items-start gap-1.5 rounded-sm border border-border bg-surface px-2.5 py-2 text-[11px] leading-snug text-muted-foreground">
            <InfoIcon className="mt-px size-3 shrink-0" />
            Rendering runs headless in Rust and reports progress over a channel. The{" "}
            <code className="font-mono">export</code> module is not built yet, so this dialog
            collects settings only.
          </p>
        </DialogBody>

        <DialogFooter>
          <Button variant="outline" size="md" onClick={() => onOpenChange(false)}>
            Close
          </Button>
          <Button variant="default" size="md" disabled>
            Export
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
