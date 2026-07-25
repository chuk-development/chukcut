import { AlertTriangleIcon, FolderOpenIcon, Loader2Icon, UploadIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

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
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { saveFileDialog } from "@/lib/dialog";
import { formatTimecode } from "@/lib/time";
import type { Container, HwEncoder } from "@/modules/export/lib/api";
import {
  AUDIO_CODEC_LABELS,
  buildRequest,
  CODEC_LABELS,
  CONTAINERS,
  crfRange,
  defaultFileName,
  type ExportForm,
  exportBlockedReason,
  FRAME_RATES,
  formatFps,
  formFromPreset,
  resolveSettings,
  withExtension,
} from "@/modules/export/lib/settings";
import { useExportStore } from "@/modules/export/store";
import { useProjectStore } from "@/modules/project/store";
import { projectDuration } from "@/modules/project/types";

const SOFTWARE = "software";

interface ExportDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** One labelled row. The 104px gutter is what keeps the column of values straight. */
function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[104px_1fr] items-center gap-3 text-[12px]">
      <span className="text-muted-foreground">{label}</span>
      <div className="min-w-0">{children}</div>
    </div>
  );
}

/**
 * Collect the settings, hand them to Rust, get out of the way.
 *
 * The dialog closes the moment the job is accepted. Progress belongs to
 * `ExportProgressDock`, which is not modal, because an export of a five minute
 * timeline is minutes of work and the editor stays usable throughout.
 */
export function ExportDialog({ open, onOpenChange }: ExportDialogProps) {
  const project = useProjectStore((s) => s.project);
  const projectPath = useProjectStore((s) => s.path);

  const options = useExportStore((s) => s.options);
  const loadingOptions = useExportStore((s) => s.loadingOptions);
  const optionsError = useExportStore((s) => s.optionsError);
  const loadOptions = useExportStore((s) => s.loadOptions);
  const start = useExportStore((s) => s.start);
  const starting = useExportStore((s) => s.starting);
  const startError = useExportStore((s) => s.startError);
  const clearStartError = useExportStore((s) => s.clearStartError);

  const [form, setForm] = useState<ExportForm | null>(null);

  useEffect(() => {
    if (open) void loadOptions();
  }, [open, loadOptions]);

  // Seed the form once the presets are known. Re-opening keeps whatever the
  // user chose last time; only a first open, or a reload, builds it fresh.
  useEffect(() => {
    if (!open || !options || form) return;
    const preset =
      options.presets.find((candidate) => candidate.id === options.default_preset_id) ??
      options.presets[0];
    if (preset) setForm(formFromPreset(preset, project));
  }, [open, options, form, project]);

  const hardware = useMemo(() => options?.hardware ?? [], [options]);
  const resolved = useMemo(() => (form ? resolveSettings(form, hardware) : null), [form, hardware]);

  const duration = project ? projectDuration(project) : 0;
  const totalFrames =
    resolved && duration > 0 ? Math.ceil((duration / 1_000_000) * resolved.fps) : 0;

  const displayPath =
    form?.outputPath && resolved ? withExtension(form.outputPath, resolved.container) : null;

  const chooseLocation = useCallback(async () => {
    if (!resolved) return;
    // A saved project is the strongest hint there is: the same folder, the same
    // stem, the container's extension. Its `.chukcut` path is one the webview
    // already has, so nothing here invents a location.
    const suggested = projectPath
      ? withExtension(projectPath, resolved.container)
      : defaultFileName(project?.name, resolved.container);
    const chosen = await saveFileDialog({
      title: "Export video",
      filters: [{ name: resolved.container.toUpperCase(), extensions: [resolved.container] }],
      defaultPath: suggested,
    });
    if (chosen) setForm((previous) => (previous ? { ...previous, outputPath: chosen } : previous));
  }, [project?.name, projectPath, resolved]);

  const blocked = exportBlockedReason(project, duration, form?.outputPath ?? null);

  const beginExport = useCallback(async () => {
    if (!form || !resolved || blocked) return;
    clearStartError();
    const jobId = await start(buildRequest(form, resolved));
    // A refused export leaves the dialog open with the reason in it; there is
    // nothing to watch and closing would throw the message away.
    if (jobId) onOpenChange(false);
  }, [blocked, clearStartError, form, onOpenChange, resolved, start]);

  const [crfLow, crfHigh] = resolved ? crfRange(resolved.videoCodec) : [0, 51];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[540px]">
        <DialogHeader>
          <DialogTitle>Export</DialogTitle>
          <DialogDescription>
            {project
              ? `${project.name} · ${formatTimecode(duration, project.fps)}${
                  totalFrames > 0 ? ` · ${totalFrames.toLocaleString()} frames` : ""
                }`
              : "No project open"}
          </DialogDescription>
        </DialogHeader>

        <DialogBody className="flex max-h-[62vh] flex-col gap-3 overflow-y-auto">
          {loadingOptions && !options ? (
            <p className="flex items-center gap-2 py-6 text-[12px] text-muted-foreground">
              <Loader2Icon className="size-3.5 animate-spin" />
              Asking Rust what this machine can encode…
            </p>
          ) : null}

          {optionsError ? (
            <div className="flex items-start gap-2 rounded-sm border border-destructive/40 bg-destructive/15 px-2.5 py-2 text-[11px] leading-snug">
              <AlertTriangleIcon className="mt-px size-3 shrink-0 text-destructive" />
              <span className="min-w-0 flex-1">{optionsError}</span>
              <Button size="sm" variant="outline" onClick={() => void loadOptions(true)}>
                Retry
              </Button>
            </div>
          ) : null}

          {form && resolved && options ? (
            <>
              <Row label="Preset">
                <Select
                  value={form.presetId}
                  onValueChange={(id) => {
                    const preset = options.presets.find((candidate) => candidate.id === id);
                    if (preset)
                      setForm((previous) => formFromPreset(preset, project, previous ?? {}));
                  }}
                >
                  <SelectTrigger aria-label="Preset">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {options.presets.map((preset) => (
                      <SelectItem key={preset.id} value={preset.id}>
                        {preset.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Row>

              <Row label="Resolution">
                <div className="flex items-center gap-2">
                  <Input
                    aria-label="Width"
                    type="number"
                    min={2}
                    step={2}
                    className="w-[84px]"
                    value={form.width}
                    onChange={(event) =>
                      setForm((previous) =>
                        previous ? { ...previous, width: Number(event.target.value) } : previous,
                      )
                    }
                  />
                  <span className="text-muted-foreground">×</span>
                  <Input
                    aria-label="Height"
                    type="number"
                    min={2}
                    step={2}
                    className="w-[84px]"
                    value={form.height}
                    onChange={(event) =>
                      setForm((previous) =>
                        previous ? { ...previous, height: Number(event.target.value) } : previous,
                      )
                    }
                  />
                  {resolved.width !== form.width || resolved.height !== form.height ? (
                    <span className="truncate text-[11px] text-muted-foreground">
                      encodes as {resolved.width}×{resolved.height}
                    </span>
                  ) : null}
                </div>
              </Row>

              <Row label="Frame rate">
                <Select
                  value={String(form.fps)}
                  onValueChange={(value) =>
                    setForm((previous) =>
                      previous ? { ...previous, fps: Number(value) } : previous,
                    )
                  }
                >
                  <SelectTrigger aria-label="Frame rate">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {(FRAME_RATES.includes(form.fps)
                      ? FRAME_RATES
                      : [...FRAME_RATES, form.fps].sort((a, b) => a - b)
                    ).map((rate) => (
                      <SelectItem key={rate} value={String(rate)}>
                        {formatFps(rate)} fps
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Row>

              <Row label="Quality">
                <div className="flex items-center gap-2">
                  <Select
                    value={form.quality.kind}
                    onValueChange={(kind) =>
                      setForm((previous) =>
                        previous
                          ? {
                              ...previous,
                              quality:
                                kind === "crf"
                                  ? { kind: "crf", value: 20 }
                                  : { kind: "bitrate", value: 12_000_000 },
                            }
                          : previous,
                      )
                    }
                  >
                    <SelectTrigger aria-label="Quality mode" className="w-[112px]">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="crf">Constant</SelectItem>
                      <SelectItem value="bitrate">Bitrate</SelectItem>
                    </SelectContent>
                  </Select>

                  {form.quality.kind === "crf" ? (
                    <>
                      <Slider
                        className="min-w-0 flex-1"
                        aria-label="Constant rate factor"
                        min={crfLow}
                        max={crfHigh}
                        step={1}
                        value={[resolved.quality.value]}
                        onValueChange={([value]) =>
                          setForm((previous) =>
                            previous ? { ...previous, quality: { kind: "crf", value } } : previous,
                          )
                        }
                      />
                      <span className="w-[104px] shrink-0 text-right font-mono text-[11px] tabular-nums text-muted-foreground">
                        CRF {resolved.quality.value} · lower is better
                      </span>
                    </>
                  ) : (
                    <>
                      <Input
                        aria-label="Video bitrate in megabits per second"
                        type="number"
                        min={0.1}
                        step={0.5}
                        className="w-[96px]"
                        value={resolved.quality.value / 1_000_000}
                        onChange={(event) =>
                          setForm((previous) =>
                            previous
                              ? {
                                  ...previous,
                                  quality: {
                                    kind: "bitrate",
                                    value: Math.round(Number(event.target.value) * 1_000_000),
                                  },
                                }
                              : previous,
                          )
                        }
                      />
                      <span className="text-[11px] text-muted-foreground">Mbit/s</span>
                    </>
                  )}
                </div>
              </Row>

              <Row label="Format">
                <div className="flex items-center gap-2">
                  <Select
                    value={resolved.container}
                    onValueChange={(container) =>
                      setForm((previous) =>
                        previous ? { ...previous, container: container as Container } : previous,
                      )
                    }
                  >
                    <SelectTrigger aria-label="Container" className="w-[112px]">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {CONTAINERS.map((container) => (
                        <SelectItem key={container.value} value={container.value}>
                          {container.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <span className="truncate text-[11px] text-muted-foreground">
                    {CODEC_LABELS[resolved.videoCodec]}
                    {form.includeAudio ? ` · ${AUDIO_CODEC_LABELS[resolved.audioCodec]}` : ""}
                  </span>
                </div>
              </Row>

              <Row label="Encoder">
                <EncoderSelect
                  value={form.hardwareId ?? SOFTWARE}
                  hardware={hardware}
                  onChange={(id) =>
                    setForm((previous) =>
                      previous
                        ? { ...previous, hardwareId: id === SOFTWARE ? null : id }
                        : previous,
                    )
                  }
                />
              </Row>

              <Row label="Audio">
                <div className="flex items-center gap-2 text-[12px]">
                  <Switch
                    id="export-include-audio"
                    checked={form.includeAudio}
                    onCheckedChange={(includeAudio) =>
                      setForm((previous) => (previous ? { ...previous, includeAudio } : previous))
                    }
                    aria-label="Include audio"
                  />
                  <label
                    htmlFor="export-include-audio"
                    className="cursor-pointer text-muted-foreground"
                  >
                    {form.includeAudio ? "Include the mixed audio track" : "Video only, no audio"}
                  </label>
                </div>
              </Row>

              <Row label="Save to">
                <div className="flex items-center gap-2">
                  <span
                    data-slot="export-destination"
                    className="min-w-0 flex-1 truncate rounded-sm border border-input bg-background px-2 py-1 font-mono text-[11px] text-foreground/85"
                    title={displayPath ?? undefined}
                  >
                    {displayPath ?? (
                      <span className="font-sans text-muted-foreground">Not chosen yet</span>
                    )}
                  </span>
                  <Button size="default" variant="outline" onClick={() => void chooseLocation()}>
                    <FolderOpenIcon />
                    Choose…
                  </Button>
                </div>
              </Row>
            </>
          ) : null}

          {startError ? (
            <p className="flex items-start gap-1.5 rounded-sm border border-destructive/40 bg-destructive/15 px-2.5 py-2 text-[11px] leading-snug">
              <AlertTriangleIcon className="mt-px size-3 shrink-0 text-destructive" />
              <span className="min-w-0 flex-1">{startError}</span>
            </p>
          ) : null}
        </DialogBody>

        <DialogFooter className="items-center justify-between">
          <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
            {blocked ?? "Encoding runs in the background; you can keep editing."}
          </span>
          <Button variant="outline" size="md" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            variant="default"
            size="md"
            onClick={() => void beginExport()}
            disabled={Boolean(blocked) || starting || !form}
          >
            {starting ? <Loader2Icon className="animate-spin" /> : <UploadIcon />}
            {starting ? "Starting…" : "Export"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Software plus whatever hardware this machine has.
 *
 * An encoder that is present but cannot be driven yet is listed and disabled
 * with its reason, rather than hidden: "your GPU is not supported yet" is a
 * better answer than an option that silently is not there.
 */
function EncoderSelect({
  value,
  hardware,
  onChange,
}: {
  value: string;
  hardware: HwEncoder[];
  onChange: (id: string) => void;
}) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger aria-label="Encoder">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={SOFTWARE}>Software (predictable, slower)</SelectItem>
        {hardware.map((encoder) => (
          <SelectItem key={encoder.id} value={encoder.id} disabled={!encoder.usable}>
            <span className="flex flex-col items-start">
              <span>{encoder.label}</span>
              {encoder.note ? (
                <span className="text-[10px] text-muted-foreground">{encoder.note}</span>
              ) : null}
            </span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
