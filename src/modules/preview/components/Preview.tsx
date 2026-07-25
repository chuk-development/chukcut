import { AlertTriangleIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { frameDuration } from "@/lib/time";
import { cn } from "@/lib/utils";
import { TransportControls } from "@/modules/preview/components/TransportControls";
import { fetchFrame, proxyResolution } from "@/modules/preview/lib/api";
import {
  setFullscreen,
  toggleFullscreen,
  watchWindowFullscreen,
} from "@/modules/preview/lib/fullscreen";
import { preview, watchDocumentForPreview } from "@/modules/preview/lib/session";
import { usePreviewStore } from "@/modules/preview/store";
import { useProjectStore } from "@/modules/project/store";
import type { CanvasConfig } from "@/modules/project/types";
import { useTimelineStore } from "@/modules/timeline/store";

/** How long the fullscreen overlay lingers after the pointer stops moving. */
const CONTROLS_LINGER_MS = 2_200;

/**
 * Before the first frame arrives there is nothing to paint, and a blank canvas
 * reads as a bug. Draw the canvas the project actually describes: its size, its
 * background colour, its safe centre.
 */
function drawPlaceholder(
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
  background: [number, number, number, number],
  caption: string,
) {
  const [r, g, b] = background;
  const channel = (value: number) => Math.round(Math.min(1, Math.max(0, value)) * 255);
  context.fillStyle = `rgb(${channel(r)} ${channel(g)} ${channel(b)})`;
  context.fillRect(0, 0, width, height);

  context.strokeStyle = "rgba(255,255,255,0.09)";
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(width / 2, 0);
  context.lineTo(width / 2, height);
  context.moveTo(0, height / 2);
  context.lineTo(width, height / 2);
  context.stroke();

  context.strokeStyle = "rgba(255,255,255,0.06)";
  context.strokeRect(width * 0.05, height * 0.05, width * 0.9, height * 0.9);

  context.fillStyle = "rgba(255,255,255,0.42)";
  context.font = `500 ${Math.round(Math.min(width, height) * 0.045)}px ui-sans-serif, system-ui, sans-serif`;
  context.textAlign = "center";
  context.textBaseline = "middle";
  context.fillText(caption, width / 2, height / 2);
}

export function Preview() {
  const project = useProjectStore((s) => s.project);
  const playhead = useTimelineStore((s) => s.playhead);

  const session = usePreviewStore((s) => s.session);
  const frameUrl = usePreviewStore((s) => s.frameUrl);
  const frame = usePreviewStore((s) => s.frame);
  const playing = usePreviewStore((s) => s.playing);
  const previewError = usePreviewStore((s) => s.error);
  const serverWidth = usePreviewStore((s) => s.width);
  const serverHeight = usePreviewStore((s) => s.height);
  const serverFps = usePreviewStore((s) => s.fps);
  const serverDuration = usePreviewStore((s) => s.duration);
  const zoom = usePreviewStore((s) => s.zoom);
  const setZoom = usePreviewStore((s) => s.setZoom);
  const fullscreen = usePreviewStore((s) => s.fullscreen);

  const canvasRef = useRef<HTMLCanvasElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  /** Rises on every paint so a slow fetch cannot overwrite a newer frame. */
  const paintToken = useRef(0);
  const [stage, setStage] = useState({ width: 0, height: 0 });
  /** Whether the fullscreen overlay is currently showing its controls. */
  const [controlsVisible, setControlsVisible] = useState(true);

  const canvas: CanvasConfig = useMemo(
    () => project?.canvas ?? { width: 1080, height: 1920, background: [0, 0, 0, 1] },
    [project?.canvas],
  );

  // Rust clamps the proxy to what the device can allocate, so once a session
  // exists its size wins; the table is only for sizing the placeholder.
  const proxy = useMemo(() => {
    if (serverWidth > 0 && serverHeight > 0) return { width: serverWidth, height: serverHeight };
    return proxyResolution(canvas);
  }, [serverWidth, serverHeight, canvas]);

  const fps = serverFps > 0 ? serverFps : (project?.fps ?? 30);
  const duration = serverDuration;

  // Open a session and keep it in step with the document.
  useEffect(() => {
    void preview.ensureStarted();
    return watchDocumentForPreview();
  }, []);

  // The window can leave fullscreen without going through us — F11, a tiling
  // window manager. An overlay left over an ordinary window is a stuck state.
  useEffect(() => watchWindowFullscreen(), []);

  /**
   * Fade the overlay out after a couple of seconds of stillness.
   *
   * The whole point of fullscreen is looking at the picture, so the controls
   * are guests. Any pointer movement anywhere brings them back, which is why
   * this listens on the window rather than on the stage: a viewer whose pointer
   * is parked over the corner still expects a nudge to reveal them.
   */
  useEffect(() => {
    if (!fullscreen) {
      setControlsVisible(true);
      return;
    }
    let timer = setTimeout(() => setControlsVisible(false), CONTROLS_LINGER_MS);
    const onMove = () => {
      setControlsVisible(true);
      clearTimeout(timer);
      timer = setTimeout(() => setControlsVisible(false), CONTROLS_LINGER_MS);
    };
    window.addEventListener("pointermove", onMove);
    return () => {
      window.removeEventListener("pointermove", onMove);
      clearTimeout(timer);
    };
  }, [fullscreen]);

  useEffect(() => {
    const element = stageRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() =>
      setStage({ width: element.clientWidth, height: element.clientHeight }),
    );
    observer.observe(element);
    setStage({ width: element.clientWidth, height: element.clientHeight });
    return () => observer.disconnect();
  }, []);

  /**
   * Displayed size of the canvas, letterboxed into the stage.
   *
   * Only ever CSS. The bitmap stays at the proxy's own resolution — see the
   * paint effect — so scaling up here shows every pixel Rust rendered rather
   * than resampling the frame down to the element and calling it sharp. That
   * distinction is the entire reason fullscreen exists: it has to be honest
   * about how much detail the source actually has.
   */
  const display = useMemo(() => {
    // No breathing room in fullscreen: the picture is the whole point, and a
    // 24px inset on a 4K panel is a visible black frame around the black bars.
    const margin = fullscreen ? 0 : 24;
    const availableWidth = Math.max(32, stage.width - margin);
    const availableHeight = Math.max(32, stage.height - margin);
    const fitScale = Math.min(availableWidth / canvas.width, availableHeight / canvas.height);
    // A fixed zoom is a panel affordance; fullscreen always fits.
    const scale = fullscreen || zoom === "fit" ? fitScale : zoom;
    return {
      width: Math.max(16, Math.round(canvas.width * scale)),
      height: Math.max(16, Math.round(canvas.height * scale)),
    };
  }, [stage, canvas.width, canvas.height, zoom, fullscreen]);

  // Paint the frame the pacer says is due.
  //
  // 410 means the session was superseded: drop it, a newer frame is already
  // coming. 404 means the frame is not encoded yet — one retry, then leave the
  // previous picture up rather than blanking the viewer.
  useEffect(() => {
    const element = canvasRef.current;
    if (!element) return;
    const context = element.getContext("2d");
    if (!context) return;

    if (element.width !== proxy.width || element.height !== proxy.height) {
      element.width = proxy.width;
      element.height = proxy.height;
    }

    if (!frameUrl) {
      drawPlaceholder(
        context,
        proxy.width,
        proxy.height,
        canvas.background,
        `${canvas.width} × ${canvas.height}`,
      );
      return;
    }

    const token = ++paintToken.current;
    let bitmap: ImageBitmap | null = null;

    void (async () => {
      let result = await fetchFrame(frameUrl, frame);
      if (result.status === "pending") result = await fetchFrame(frameUrl, frame);
      if (token !== paintToken.current) {
        if (result.status === "ok") result.bitmap.close();
        return;
      }
      if (result.status !== "ok") return;
      bitmap = result.bitmap;
      context.drawImage(bitmap, 0, 0, proxy.width, proxy.height);
      bitmap.close();
      bitmap = null;
    })();

    return () => {
      bitmap?.close();
    };
  }, [frameUrl, frame, proxy, canvas]);

  const step = useCallback(
    (frames: number) => {
      const delta = frameDuration(fps) * frames;
      const target = Math.min(duration, Math.max(0, playhead + delta));
      useTimelineStore.getState().setPlayhead(target);
      void preview.seek(target);
    },
    [fps, duration, playhead],
  );

  const jump = useCallback(
    (position: "start" | "end") => {
      const target = position === "start" ? 0 : duration;
      useTimelineStore.getState().setPlayhead(target);
      void preview.seek(target);
    },
    [duration],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName))
      ) {
        return;
      }
      if (event.ctrlKey || event.metaKey || event.altKey) return;

      switch (event.key) {
        case "f":
        case "F":
          event.preventDefault();
          void toggleFullscreen();
          break;
        case "Escape":
          // Only meaningful when there is a fullscreen to leave; otherwise it
          // belongs to the timeline, which uses it to clear the selection.
          if (usePreviewStore.getState().fullscreen) {
            event.preventDefault();
            void setFullscreen(false);
          }
          break;
        case " ":
          event.preventDefault();
          void preview.toggle();
          break;
        case "ArrowLeft":
          event.preventDefault();
          step(event.shiftKey ? -10 : -1);
          break;
        case "ArrowRight":
          event.preventDefault();
          step(event.shiftKey ? 10 : 1);
          break;
        case "Home":
          event.preventDefault();
          jump("start");
          break;
        case "End":
          event.preventDefault();
          jump("end");
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [step, jump]);

  const transport = (compact: boolean) => (
    <TransportControls
      playing={playing}
      playhead={playhead}
      duration={duration}
      fps={fps}
      zoom={zoom}
      fullscreen={fullscreen}
      compact={compact}
      onTogglePlay={() => void preview.toggle()}
      onStep={step}
      onJump={jump}
      onZoom={setZoom}
      onToggleFullscreen={() => void toggleFullscreen()}
    />
  );

  return (
    <section
      data-slot="preview"
      data-fullscreen={fullscreen || undefined}
      // Fullscreen takes the panel out of the flow rather than unmounting it:
      // the same canvas keeps painting the same session, and the layout the
      // user built — panel widths, timeline height — is still exactly where
      // they left it underneath.
      className={cn(
        "flex min-h-0 flex-col",
        fullscreen ? "fixed inset-0 z-50 bg-preview-letterbox" : "h-full bg-panel",
      )}
      aria-label="Player"
    >
      {fullscreen ? null : (
        <header className="flex h-8 shrink-0 items-center justify-between border-b border-border px-3">
          <h2 className="text-[12px] font-semibold tracking-tight text-panel-foreground">Player</h2>
          <span className="font-mono text-[10px] text-muted-foreground">
            {proxy.width}×{proxy.height} proxy
            {session === null ? " · connecting" : ""}
          </span>
        </header>
      )}

      {previewError ? (
        <p className="flex shrink-0 items-start gap-1.5 border-b border-border bg-destructive/15 px-3 py-1.5 text-[11px] leading-snug">
          <AlertTriangleIcon className="mt-px size-3 shrink-0 text-destructive" />
          <span className="min-w-0 flex-1">{previewError}</span>
        </p>
      ) : null}

      {/* biome-ignore lint/a11y/noStaticElementInteractions: double-click to go
          fullscreen is a pointer shorthand, not the only route — the transport
          has a button and F is bound, both of which are reachable by keyboard */}
      <div
        ref={stageRef}
        onDoubleClick={() => void toggleFullscreen()}
        className={cn(
          "flex min-h-0 flex-1 items-center justify-center",
          fullscreen ? "overflow-hidden bg-preview-letterbox" : "overflow-auto bg-background p-3",
        )}
      >
        <canvas
          ref={canvasRef}
          className={cn(fullscreen ? null : "shadow-[0_0_0_1px_var(--border)]")}
          style={{ width: display.width, height: display.height }}
        />
      </div>

      {fullscreen ? (
        <div
          data-slot="fullscreen-transport"
          className={cn(
            "absolute inset-x-0 bottom-0 flex justify-center pb-5 transition-opacity duration-300",
            controlsVisible ? "opacity-100" : "pointer-events-none opacity-0",
          )}
        >
          <div className="rounded-md border border-border bg-popover/90 shadow-2xl">
            {transport(true)}
          </div>
        </div>
      ) : (
        transport(false)
      )}
    </section>
  );
}
