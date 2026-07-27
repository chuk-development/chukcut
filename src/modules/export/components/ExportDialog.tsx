import {
  AlertTriangleIcon,
  CameraIcon,
  FolderOpenIcon,
  Loader2Icon,
  UploadIcon,
} from "lucide-react";
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
import { openFileDialog, saveFileDialog } from "@/lib/dialog";
import { formatTimecode } from "@/lib/time";
import type { Container, ExportPreset, HwEncoder } from "@/modules/export/lib/api";
import { exportSnapshot } from "@/modules/export/lib/api";
import {
  AUDIO_CODEC_LABELS,
  applyRemembered,
  buildRequest,
  CODEC_LABELS,
  CONTAINERS,
  CUSTOM_PRESET_ID,
  crfRange,
  defaultFileName,
  dirOf,
  type ExportForm,
  estimateFileSize,
  estimateVideoBitrate,
  exportBlockedReason,
  FRAME_RATES,
  formatBitrate,
  formatBytes,
  formatFps,
  formFromPreset,
  joinPath,
  matchingResolution,
  normalizeExportRange,
  qualityCaption,
  rememberedPresetId,
  rememberForm,
  resolutionOptions,
  resolveSettings,
  sanitizeFileName,
  stemOf,
  withExtension,
} from "@/modules/export/lib/settings";
import { useExportStore } from "@/modules/export/store";
import { describeError, useProjectStore } from "@/modules/project/store";
import type { Project } from "@/modules/project/types";
import { projectDuration } from "@/modules/project/types";
import { useTimelineStore } from "@/modules/timeline/store";
import { useWorkspaceStore } from "@/modules/workspace/store";
import type { Settings } from "@/modules/workspace/types";

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

/** The canvas the resolution options derive from: the project for "custom". */
function baseSize(
  preset: ExportPreset,
  project: Project | null,
): { width: number; height: number } {
  return preset.id === CUSTOM_PRESET_ID && project
    ? { width: project.canvas.width, height: project.canvas.height }
    : { width: preset.width, height: preset.height };
}

/**
 * The workspace settings, seen past the typed mirror.
 *
 * `export_remember` and `export_defaults` exist in Rust's `Settings` but not
 * in the workspace module's TS mirror, which belongs to other work — so this
 * module reads them off the loaded struct itself, where they arrive with
 * every `workspace_settings_get`, and tolerates their absence.
 */
function rememberedSettings(): { active: boolean; defaults: unknown } {
  const settings = useWorkspaceStore.getState().settings as unknown as {
    export_remember?: unknown;
    export_defaults?: unknown;
  };
  return {
    active: settings.export_remember === true,
    defaults: settings.export_defaults ?? null,
  };
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
  const [remember, setRemember] = useState(false);
  /** What the name input shows while it is being edited; null when at rest. */
  const [nameDraft, setNameDraft] = useState<string | null>(null);
  const [snapshot, setSnapshot] = useState<{ busy: boolean; note: string | null; failed: boolean }>(
    { busy: false, note: null, failed: false },
  );

  useEffect(() => {
    if (open) void loadOptions();
  }, [open, loadOptions]);

  // Seed the form once the presets are known. Re-opening keeps whatever the
  // user chose last time; only a first open, or a reload, builds it fresh —
  // from the remembered settings when the user asked for that, and always
  // with the project's own folder and name as the destination.
  useEffect(() => {
    if (!open || !options || form) return;
    const memory = rememberedSettings();
    const rememberedId = memory.active ? rememberedPresetId(memory.defaults) : null;
    const preset =
      (rememberedId ? options.presets.find((candidate) => candidate.id === rememberedId) : null) ??
      options.presets.find((candidate) => candidate.id === options.default_preset_id) ??
      options.presets[0];
    if (!preset) return;

    let seeded = formFromPreset(preset, project);
    if (memory.active) seeded = applyRemembered(seeded, memory.defaults, baseSize(preset, project));
    if (!seeded.outputPath && projectPath) {
      // A saved project is the strongest hint there is: the same folder, the
      // same stem, the container's extension.
      seeded = { ...seeded, outputPath: withExtension(projectPath, seeded.container) };
    }
    setRemember(memory.active);
    setForm(seeded);
  }, [open, options, form, project, projectPath]);

  const hardware = useMemo(() => options?.hardware ?? [], [options]);
  const resolved = useMemo(() => (form ? resolveSettings(form, hardware) : null), [form, hardware]);

  const duration = project ? projectDuration(project) : 0;

  // The in/out marks live in the timeline store and are being added by other
  // work; read them defensively — absent, null and stale all mean "no marks".
  const rawRange = useTimelineStore(
    (s) => (s as unknown as { exportRange?: unknown }).exportRange ?? null,
  );
  const marks = useMemo(() => normalizeExportRange(rawRange, duration), [rawRange, duration]);
  const [rangeChoice, setRangeChoice] = useState<"whole" | "marks">("whole");
  const effectiveRange = rangeChoice === "marks" && marks ? marks : null;
  const exportDuration = effectiveRange ? effectiveRange.end - effectiveRange.start : duration;

  const totalFrames =
    resolved && exportDuration > 0 ? Math.ceil((exportDuration / 1_000_000) * resolved.fps) : 0;

  const preset = options?.presets.find((candidate) => candidate.id === form?.presetId) ?? null;
  const sizeOptions = useMemo(
    () => (preset ? resolutionOptions(baseSize(preset, project)) : []),
    [preset, project],
  );
  const sizeChoice = form
    ? (matchingResolution(sizeOptions, form.width, form.height)?.id ?? "manual")
    : "manual";

  const videoBps = resolved
    ? estimateVideoBitrate(
        resolved.videoCodec,
        resolved.quality,
        resolved.width,
        resolved.height,
        resolved.fps,
      )
    : 0;
  const audioBps =
    form?.includeAudio && resolved && resolved.audioCodec !== "none"
      ? (preset?.audio_bitrate ?? 192_000)
      : 0;
  const estimatedBytes = estimateFileSize(videoBps, audioBps, exportDuration);

  const displayPath =
    form?.outputPath && resolved ? withExtension(form.outputPath, resolved.container) : null;

  const chooseFolder = useCallback(async () => {
    if (!resolved) return;
    const currentDir = form?.outputPath
      ? dirOf(form.outputPath)
      : projectPath
        ? dirOf(projectPath)
        : undefined;
    const chosen = await openFileDialog({
      title: "Choose the export folder",
      directory: true,
      defaultPath: currentDir || undefined,
    });
    const dir = chosen[0];
    if (!dir) return;
    setForm((previous) => {
      if (!previous) return previous;
      const name = previous.outputPath
        ? `${stemOf(previous.outputPath)}.${resolved.container}`
        : defaultFileName(project?.name, resolved.container);
      return { ...previous, outputPath: joinPath(dir, name) };
    });
  }, [form?.outputPath, project?.name, projectPath, resolved]);

  const renameFile = useCallback((typed: string) => {
    setNameDraft(typed);
    if (!typed.trim()) return; // an empty box mid-edit is not a decision yet
    setForm((previous) => {
      if (!previous?.outputPath) return previous;
      const name = sanitizeFileName(typed);
      const extension = previous.outputPath.split(".").pop() ?? "mp4";
      return {
        ...previous,
        outputPath: joinPath(dirOf(previous.outputPath), `${name}.${extension}`),
      };
    });
  }, []);

  const takeSnapshot = useCallback(async () => {
    if (!project || duration <= 0) return;
    // Read at click time rather than subscribing: the playhead is the fastest
    // moving value in the app and this dialog has no reason to re-render with it.
    const playhead = useTimelineStore.getState().playhead ?? 0;
    const stem = sanitizeFileName(
      `${(project.name || "Untitled").replace(/\.chukcut$/i, "")} frame`,
    );
    const folder =
      (form?.outputPath ? dirOf(form.outputPath) : "") || (projectPath ? dirOf(projectPath) : "");
    const chosen = await saveFileDialog({
      title: "Save frame as PNG",
      filters: [{ name: "PNG", extensions: ["png"] }],
      defaultPath: folder ? joinPath(folder, `${stem}.png`) : `${stem}.png`,
    });
    if (!chosen) return;
    setSnapshot({ busy: true, note: null, failed: false });
    try {
      const written = await exportSnapshot(playhead, chosen);
      setSnapshot({ busy: false, note: `Saved ${written}`, failed: false });
    } catch (error) {
      setSnapshot({ busy: false, note: describeError(error), failed: true });
    }
  }, [project, duration, form?.outputPath, projectPath]);

  const blocked = exportBlockedReason(project, duration, form?.outputPath ?? null);

  const beginExport = useCallback(async () => {
    if (!form || !resolved || blocked) return;
    clearStartError();
    const jobId = await start(buildRequest(form, resolved, effectiveRange));
    // A refused export leaves the dialog open with the reason in it; there is
    // nothing to watch and closing would throw the message away.
    if (jobId) {
      // Settings are remembered when an export actually started with them —
      // a form somebody abandoned is not a preference. The cast reaches the
      // two fields Rust's `Settings` carries beyond the TS mirror; see
      // `rememberedSettings`.
      const workspace = useWorkspaceStore.getState();
      const wasRemembered = rememberedSettings().active;
      if (remember && preset) {
        void workspace.updateSettings({
          export_remember: true,
          export_defaults: rememberForm(form, baseSize(preset, project)),
        } as unknown as Partial<Settings>);
      } else if (!remember && wasRemembered) {
        void workspace.updateSettings({
          export_remember: false,
        } as unknown as Partial<Settings>);
      }
      onOpenChange(false);
    }
  }, [
    blocked,
    clearStartError,
    effectiveRange,
    form,
    onOpenChange,
    preset,
    project,
    remember,
    resolved,
    start,
  ]);

  const [crfLow, crfHigh] = resolved ? crfRange(resolved.videoCodec) : [0, 51];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[540px]">
        <DialogHeader>
          <DialogTitle>Export</DialogTitle>
          <DialogDescription>
            {project
              ? `${project.name} · ${formatTimecode(exportDuration, project.fps)}${
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
                    const next = options.presets.find((candidate) => candidate.id === id);
                    if (next) setForm((previous) => formFromPreset(next, project, previous ?? {}));
                  }}
                >
                  <SelectTrigger aria-label="Preset">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {options.presets.map((candidate) => (
                      <SelectItem key={candidate.id} value={candidate.id}>
                        {candidate.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Row>

              <Row label="Range">
                {marks ? (
                  <Select
                    value={rangeChoice}
                    onValueChange={(choice) => setRangeChoice(choice as "whole" | "marks")}
                  >
                    <SelectTrigger aria-label="Range">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="whole">Whole project</SelectItem>
                      <SelectItem value="marks">
                        {`Between the in/out marks · ${formatTimecode(
                          marks.start,
                          project?.fps ?? 30,
                        )} – ${formatTimecode(marks.end, project?.fps ?? 30)}`}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                ) : (
                  <span className="text-muted-foreground">Whole project</span>
                )}
              </Row>

              <Row label="Resolution">
                <div className="flex items-center gap-2">
                  <Select
                    value={sizeChoice}
                    onValueChange={(id) => {
                      const option = sizeOptions.find((candidate) => candidate.id === id);
                      if (option)
                        setForm((previous) =>
                          previous
                            ? { ...previous, width: option.width, height: option.height }
                            : previous,
                        );
                    }}
                  >
                    <SelectTrigger aria-label="Size" className="w-[168px]">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {sizeOptions.map((option) => (
                        <SelectItem key={option.id} value={option.id}>
                          {option.label}
                        </SelectItem>
                      ))}
                      {/* Hand-typed numbers show as "Manual"; it is a state,
                          not a choice, so it cannot be picked. */}
                      <SelectItem value="manual" disabled>
                        Manual
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <Input
                    aria-label="Width"
                    type="number"
                    min={2}
                    step={2}
                    className="w-[76px]"
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
                    className="w-[76px]"
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
                      <span
                        data-slot="export-quality-caption"
                        className="w-[170px] shrink-0 text-right text-[11px] leading-tight text-muted-foreground"
                      >
                        CRF {resolved.quality.value} ·{" "}
                        {qualityCaption(resolved.videoCodec, resolved.quality.value)}
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

              <Row label="Estimate">
                <span
                  data-slot="export-estimate"
                  className="text-[11px] leading-snug text-muted-foreground"
                >
                  {exportDuration > 0
                    ? `≈ ${formatBytes(estimatedBytes)} · video ${formatBitrate(videoBps)}` +
                      (audioBps > 0 ? ` + audio ${formatBitrate(audioBps)}` : "") +
                      " — an estimate; the real size depends on the footage"
                    : "Nothing to estimate yet"}
                </span>
              </Row>

              <Row label="File name">
                <div className="flex items-center gap-1.5">
                  <Input
                    aria-label="File name"
                    className="w-[200px]"
                    value={nameDraft ?? (form.outputPath ? stemOf(form.outputPath) : "")}
                    disabled={!form.outputPath}
                    placeholder={defaultFileName(project?.name, resolved.container)}
                    onChange={(event) => renameFile(event.target.value)}
                    onBlur={() => setNameDraft(null)}
                  />
                  <span className="text-[11px] text-muted-foreground">.{resolved.container}</span>
                </div>
              </Row>

              <Row label="Save to">
                <div className="flex items-center gap-2">
                  <span
                    data-slot="export-destination"
                    className="min-w-0 flex-1 truncate rounded-sm border border-input bg-background px-2 py-1 font-mono text-[11px] text-foreground/85"
                    title={displayPath ?? undefined}
                  >
                    {displayPath ? (
                      dirOf(displayPath) || displayPath
                    ) : (
                      <span className="font-sans text-muted-foreground">Not chosen yet</span>
                    )}
                  </span>
                  <Button size="default" variant="outline" onClick={() => void chooseFolder()}>
                    <FolderOpenIcon />
                    Choose…
                  </Button>
                </div>
              </Row>

              <Row label="Remember">
                <div className="flex items-center gap-2 text-[12px]">
                  <Switch
                    id="export-remember"
                    checked={remember}
                    onCheckedChange={setRemember}
                    aria-label="Remember these settings"
                  />
                  <label htmlFor="export-remember" className="cursor-pointer text-muted-foreground">
                    Use these settings for the next export
                  </label>
                </div>
              </Row>

              <Row label="Snapshot">
                <div className="flex min-w-0 items-center gap-2">
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => void takeSnapshot()}
                    disabled={!project || duration <= 0 || snapshot.busy}
                  >
                    {snapshot.busy ? <Loader2Icon className="animate-spin" /> : <CameraIcon />}
                    Save frame as PNG
                  </Button>
                  <span
                    data-slot="export-snapshot-note"
                    className={`min-w-0 flex-1 truncate text-[11px] ${
                      snapshot.failed ? "text-destructive" : "text-muted-foreground"
                    }`}
                    title={snapshot.note ?? undefined}
                  >
                    {snapshot.note ?? "The frame at the playhead, at full canvas resolution."}
                  </span>
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
