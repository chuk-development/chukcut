import { AlertTriangleIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { frameDuration } from "@/lib/time";
import { TransportControls } from "@/modules/preview/components/TransportControls";
import { fetchFrame, proxyResolution } from "@/modules/preview/lib/api";
import { preview, watchDocumentForPreview } from "@/modules/preview/lib/session";
import { usePreviewStore } from "@/modules/preview/store";
import { useProjectStore } from "@/modules/project/store";
import type { CanvasConfig } from "@/modules/project/types";
import { useTimelineStore } from "@/modules/timeline/store";

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

  const canvasRef = useRef<HTMLCanvasElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  /** Rises on every paint so a slow fetch cannot overwrite a newer frame. */
  const paintToken = useRef(0);
  const [stage, setStage] = useState({ width: 0, height: 0 });

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

  /** Displayed size of the canvas, letterboxed into the stage. */
  const display = useMemo(() => {
    const margin = 24;
    const availableWidth = Math.max(32, stage.width - margin);
    const availableHeight = Math.max(32, stage.height - margin);
    const fitScale = Math.min(availableWidth / canvas.width, availableHeight / canvas.height);
    const scale = zoom === "fit" ? fitScale : zoom;
    return {
      width: Math.max(16, Math.round(canvas.width * scale)),
      height: Math.max(16, Math.round(canvas.height * scale)),
    };
  }, [stage, canvas.width, canvas.height, zoom]);

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

  return (
    <section
      data-slot="preview"
      className="flex h-full min-h-0 flex-col bg-panel"
      aria-label="Player"
    >
      <header className="flex h-8 shrink-0 items-center justify-between border-b border-border px-3">
        <h2 className="text-[12px] font-semibold tracking-tight text-panel-foreground">Player</h2>
        <span className="font-mono text-[10px] text-muted-foreground">
          {proxy.width}×{proxy.height} proxy
          {session === null ? " · connecting" : ""}
        </span>
      </header>

      {previewError ? (
        <p className="flex shrink-0 items-start gap-1.5 border-b border-border bg-destructive/15 px-3 py-1.5 text-[11px] leading-snug">
          <AlertTriangleIcon className="mt-px size-3 shrink-0 text-destructive" />
          <span className="min-w-0 flex-1">{previewError}</span>
        </p>
      ) : null}

      <div
        ref={stageRef}
        className="flex min-h-0 flex-1 items-center justify-center overflow-auto bg-background p-3"
      >
        <canvas
          ref={canvasRef}
          className="shadow-[0_0_0_1px_var(--border)]"
          style={{ width: display.width, height: display.height }}
        />
      </div>

      <TransportControls
        playing={playing}
        playhead={playhead}
        duration={duration}
        fps={fps}
        zoom={zoom}
        onTogglePlay={() => void preview.toggle()}
        onStep={step}
        onJump={jump}
        onZoom={setZoom}
      />
    </section>
  );
}
