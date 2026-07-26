/**
 * Settings.
 *
 * Every control here writes the whole `Settings` struct back through
 * `workspace_settings_set` the moment it changes — there is no OK button,
 * because a settings panel with one invites the question of what Cancel does to
 * the three things you already changed. A failed write bounces the control back
 * and says why.
 *
 * Density matches the inspector: a 140px label column, 26px controls, captions
 * only where the consequence of a setting is not obvious from its name.
 */

import { AlertTriangleIcon, Loader2Icon, Trash2Icon } from "lucide-react";
import type * as React from "react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { HardwarePanel } from "@/modules/workspace/components/HardwarePanel";
import { formatBytes } from "@/modules/workspace/lib/format";
import { useWorkspaceStore } from "@/modules/workspace/store";

const GIB = 1024 * 1024 * 1024;

const PREVIEW_CAPS = [540, 720, 960, 1280, 1920];
const FRAME_RATES = [24, 25, 30, 50, 60];
const CANVASES: [number, number][] = [
  [1080, 1920],
  [1920, 1080],
  [1080, 1080],
  [2160, 3840],
  [3840, 2160],
];
const CACHE_LIMITS = [2 * GIB, 4 * GIB, 8 * GIB, 16 * GIB, 32 * GIB, 0];

interface SettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function SettingsDialog({ open, onOpenChange }: SettingsDialogProps) {
  const settings = useWorkspaceStore((s) => s.settings);
  const status = useWorkspaceStore((s) => s.settingsStatus);
  const error = useWorkspaceStore((s) => s.settingsError);
  const update = useWorkspaceStore((s) => s.updateSettings);
  const cacheSize = useWorkspaceStore((s) => s.cacheSize);
  const cacheBusy = useWorkspaceStore((s) => s.cacheBusy);
  const refreshCacheSize = useWorkspaceStore((s) => s.refreshCacheSize);
  const clearCache = useWorkspaceStore((s) => s.clearCache);

  // Local while dragging: the slider has to track the finger, and persisting on
  // every pointer move would be sixty writes a second to a file.
  const [quality, setQuality] = useState(settings.preview_quality);
  useEffect(() => setQuality(settings.preview_quality), [settings.preview_quality]);

  useEffect(() => {
    if (open) void refreshCacheSize();
  }, [open, refreshCacheSize]);

  const disabled = status === "loading";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[560px]">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>
            Saved as you change them, in your config directory. They belong to the app, not to any
            one project.
          </DialogDescription>
        </DialogHeader>

        <Tabs defaultValue="preview">
          <div className="border-b border-border px-3">
            <TabsList>
              <TabsTrigger value="preview">Preview</TabsTrigger>
              <TabsTrigger value="editing">Editing</TabsTrigger>
              <TabsTrigger value="storage">Storage</TabsTrigger>
              <TabsTrigger value="hardware">Hardware</TabsTrigger>
            </TabsList>
          </div>

          {error ? (
            <p className="flex items-start gap-1.5 border-b border-border bg-destructive/15 px-4 py-2 text-[11px] leading-snug text-destructive-foreground">
              <AlertTriangleIcon className="mt-px size-3 shrink-0" />
              <span className="min-w-0 flex-1">{error}</span>
            </p>
          ) : null}

          <DialogBody className="max-h-[min(60vh,440px)] overflow-y-auto">
            <TabsContent value="preview" className="flex flex-col gap-3.5">
              <Field
                label="Resolution cap"
                caption="The long edge the preview renders at. Lower is faster and changes nothing about what you export."
              >
                <NumberSelect
                  aria-label="Resolution cap"
                  value={settings.preview_max_edge}
                  options={PREVIEW_CAPS}
                  format={(value) => `${value} px`}
                  disabled={disabled || settings.preview_full_quality}
                  onChange={(value) => void update({ preview_max_edge: value })}
                />
              </Field>

              <Field
                label="Full resolution"
                caption="Render the preview at the project's own size, whatever that costs. For judging fine detail on a fast machine."
              >
                <Switch
                  aria-label="Full resolution"
                  checked={settings.preview_full_quality}
                  disabled={disabled}
                  onCheckedChange={(checked) => void update({ preview_full_quality: checked })}
                />
              </Field>

              <Field
                label="Frame quality"
                caption="Preview frames reach the window as JPEG. Below about 70 the compression starts showing on flat areas."
              >
                <div className="flex items-center gap-2.5">
                  <Slider
                    aria-label="Frame quality"
                    min={40}
                    max={100}
                    step={1}
                    value={[quality]}
                    disabled={disabled}
                    onValueChange={([value]) => setQuality(value)}
                    onValueCommit={([value]) => void update({ preview_quality: value })}
                  />
                  <span className="w-7 shrink-0 text-right font-mono text-[11px] text-muted-foreground">
                    {quality}
                  </span>
                </div>
              </Field>

              <p className="border-t border-border pt-3 text-[11px] leading-relaxed text-muted-foreground">
                The preview session restarts when you change these, so they take effect on the next
                frame rather than at the next launch.
              </p>
            </TabsContent>

            <TabsContent value="editing" className="flex flex-col gap-3.5">
              <Field
                label="Snapping"
                caption="Clips snap to edges, the playhead and second boundaries while dragging. S toggles it for the session without changing this."
              >
                <Switch
                  aria-label="Snapping"
                  checked={settings.snapping}
                  disabled={disabled}
                  onCheckedChange={(checked) => void update({ snapping: checked })}
                />
              </Field>

              <div className="border-t border-border pt-3.5">
                <h3 className="text-[12px] font-semibold text-panel-foreground">New projects</h3>
                <p className="mt-0.5 mb-3 text-[11px] leading-relaxed text-muted-foreground">
                  What the start screen and the New Project dialog begin with. The first video
                  imported into an empty timeline still adopts its own shape.
                </p>

                <div className="flex flex-col gap-3.5">
                  <Field label="Canvas">
                    <Select
                      value={`${settings.default_canvas[0]}x${settings.default_canvas[1]}`}
                      disabled={disabled}
                      onValueChange={(value) => {
                        const [width, height] = value.split("x").map(Number);
                        void update({ default_canvas: [width, height] });
                      }}
                    >
                      <SelectTrigger aria-label="Canvas">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {withCurrent(
                          CANVASES.map((c) => `${c[0]}x${c[1]}`),
                          `${settings.default_canvas[0]}x${settings.default_canvas[1]}`,
                        ).map((value) => (
                          <SelectItem key={value} value={value}>
                            {value.replace("x", " × ")}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </Field>

                  <Field label="Frame rate">
                    <NumberSelect
                      aria-label="Frame rate"
                      value={settings.default_fps}
                      options={FRAME_RATES}
                      format={(value) => `${value} fps`}
                      disabled={disabled}
                      onChange={(value) => void update({ default_fps: value })}
                    />
                  </Field>
                </div>
              </div>
            </TabsContent>

            <TabsContent value="storage" className="flex flex-col gap-3.5">
              <Field
                label="Cache limit"
                caption="Thumbnails, waveform peaks, proxies and rendered frames. Everything under the cache is derived and safe to lose."
              >
                <NumberSelect
                  aria-label="Cache limit"
                  value={settings.cache_limit}
                  options={CACHE_LIMITS}
                  format={(value) => (value === 0 ? "No limit" : formatBytes(value))}
                  disabled={disabled}
                  onChange={(value) => void update({ cache_limit: value })}
                />
              </Field>

              <div className="flex items-center justify-between gap-3 rounded-md border border-border bg-surface/50 px-3 py-2.5">
                <div className="min-w-0">
                  <p className="text-[12px] font-medium">
                    {cacheSize === null ? "Cache size unknown" : formatBytes(cacheSize)} on disk
                  </p>
                  <p className="mt-0.5 text-[11px] text-muted-foreground">
                    Clearing costs the next few thumbnails and nothing else.
                  </p>
                </div>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={cacheBusy || cacheSize === 0}
                  onClick={() => void clearCache()}
                >
                  {cacheBusy ? <Loader2Icon className="animate-spin" /> : <Trash2Icon />}
                  {cacheBusy ? "Clearing…" : "Clear cache"}
                </Button>
              </div>
            </TabsContent>

            <TabsContent value="hardware">
              <HardwarePanel />
            </TabsContent>
          </DialogBody>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}

function Field({
  label,
  caption,
  children,
}: {
  label: string;
  caption?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="grid grid-cols-[140px_1fr] items-center gap-x-3 gap-y-1 text-[12px]">
      <span className="text-muted-foreground">{label}</span>
      <div className="min-w-0 justify-self-start [&>*]:min-w-[160px]">{children}</div>
      {caption ? (
        <p className="col-start-2 text-[11px] leading-relaxed text-muted-foreground">{caption}</p>
      ) : null}
    </div>
  );
}

/** A select over numbers, which Radix insists on seeing as strings. */
function NumberSelect({
  value,
  options,
  format,
  onChange,
  disabled,
  "aria-label": label,
}: {
  value: number;
  options: number[];
  format: (value: number) => string;
  onChange: (value: number) => void;
  disabled?: boolean;
  "aria-label": string;
}) {
  return (
    <Select
      value={String(value)}
      disabled={disabled}
      onValueChange={(next) => onChange(Number(next))}
    >
      <SelectTrigger aria-label={label}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {withCurrent(options.map(String), String(value)).map((option) => (
          <SelectItem key={option} value={option}>
            {format(Number(option))}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/**
 * Keep whatever is already stored in the list.
 *
 * A settings file written by an older build — or by hand — can hold a value
 * this menu does not offer, and a Radix select whose value is not among its
 * items renders blank. Showing the odd value is honest; silently normalising it
 * would rewrite a preference nobody asked us to touch.
 */
function withCurrent(options: string[], current: string): string[] {
  return options.includes(current) ? options : [current, ...options];
}
