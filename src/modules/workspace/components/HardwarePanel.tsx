/**
 * The panel that answers "is it actually using my graphics card".
 *
 * That question has been asked, in effect, several times, and it has never had
 * an honest place to be answered. It is worth being precise about what makes an
 * answer honest here: the engine does not read a capability list, because on
 * this class of hardware capability lists lie — `av1_vaapi` is in every modern
 * FFmpeg build and this chip cannot encode AV1; `avcodec_find_decoder` hands
 * back a decoder that cannot touch the GPU. So `export/hwaccel.rs` encodes a
 * 320×240 frame with each encoder and `media/hwdecode.rs` decodes an embedded
 * bitstream with each decoder, and only what survived that is reported as
 * usable.
 *
 * Which means this panel has three states worth distinguishing, not two:
 * working, absent, and *present but refused* — and the third one is the
 * interesting one, so it is shown with the driver's reason rather than hidden.
 */

import {
  AlertTriangleIcon,
  CheckIcon,
  CpuIcon,
  Loader2Icon,
  RotateCcwIcon,
  XIcon,
} from "lucide-react";
import type * as React from "react";
import { useEffect } from "react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { refusedEncoders, summarise, usableEncoders } from "@/modules/workspace/lib/hardware";
import { useWorkspaceStore } from "@/modules/workspace/store";
import type { HwDecodeCodecId, HwDecoder, HwEncoder } from "@/modules/workspace/types";

const DECODE_CODEC_LABELS: Record<HwDecodeCodecId, string> = {
  h264: "H.264",
  hevc: "HEVC",
  vp9: "VP9",
  av1: "AV1",
};

export function HardwarePanel() {
  const status = useWorkspaceStore((s) => s.hardwareStatus);
  const error = useWorkspaceStore((s) => s.hardwareError);
  const hardware = useWorkspaceStore((s) => s.hardware);
  const load = useWorkspaceStore((s) => s.loadHardware);

  useEffect(() => {
    // The probe is cached for the process on the Rust side, so re-asking on
    // every mount costs one IPC round trip and nothing else.
    void load();
  }, [load]);

  if (status === "idle" || status === "loading") {
    return (
      <div className="flex flex-col items-center justify-center gap-2 py-14 text-center">
        <Loader2Icon className="size-4 animate-spin text-muted-foreground" />
        <p className="text-[12px]">Trying each encoder and decoder</p>
        <p className="max-w-[300px] text-[11px] leading-relaxed text-muted-foreground">
          The only way to know is to run them, so this takes a moment the first time.
        </p>
      </div>
    );
  }

  if (status === "error") {
    return (
      <div className="rounded-md border border-border bg-destructive/10 px-3 py-3">
        <p className="flex items-start gap-1.5 text-[11px] leading-snug text-destructive-foreground">
          <AlertTriangleIcon className="mt-px size-3 shrink-0" />
          <span className="min-w-0 flex-1">{error ?? "The hardware probe did not answer."}</span>
        </p>
        <Button size="sm" variant="outline" className="mt-2.5" onClick={() => void load()}>
          <RotateCcwIcon />
          Try again
        </Button>
      </div>
    );
  }

  const usable = usableEncoders(hardware);
  const refused = refusedEncoders(hardware);
  const decodersUsable = hardware.decoders.filter((decoder) => decoder.usable);
  const decodersRefused = hardware.decoders.filter((decoder) => !decoder.usable);
  const nothingAtAll = hardware.encoders.length === 0 && hardware.decoders.length === 0;

  return (
    <div className="flex flex-col gap-4">
      {/* ------------------------------------------------------------- GPU */}
      <section className="rounded-md border border-border bg-surface/50 px-3 py-2.5">
        <div className="flex items-start gap-2.5">
          <CpuIcon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            {hardware.gpu ? (
              <>
                <p className="truncate text-[12px] font-medium">{hardware.gpu.name}</p>
                <p className="mt-0.5 truncate text-[11px] text-muted-foreground">
                  {[hardware.gpu.backend, hardware.gpu.device_type, hardware.gpu.driver]
                    .filter(Boolean)
                    .join(" · ")}
                </p>
              </>
            ) : (
              <>
                <p className="text-[12px] font-medium">Adapter not reported</p>
                <p className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
                  The compositor opens a GPU device, but nothing over the boundary says which one
                  yet.
                </p>
              </>
            )}
          </div>
        </div>
        <p className="mt-2 border-t border-border pt-2 text-[11px] leading-relaxed text-muted-foreground">
          {summarise(hardware)}.
        </p>
      </section>

      {hardware.partial ? (
        <p className="rounded-sm border border-border bg-muted/30 px-2.5 py-2 text-[11px] leading-relaxed text-muted-foreground">
          This build only reports the encoder probe. Decode support and the adapter's identity are
          measured in the engine but not yet exposed over IPC, so their absence below means "not
          asked", not "not supported".
        </p>
      ) : null}

      {nothingAtAll ? (
        <div className="rounded-md border border-dashed border-border px-4 py-8 text-center">
          <p className="text-[12px] font-medium">No hardware video path on this machine</p>
          <p className="mx-auto mt-1 max-w-[380px] text-[11px] leading-relaxed text-muted-foreground">
            Nothing here is broken: either there is no GPU that FFmpeg can reach, or its driver
            refused every encoder and decoder we tried. Export and preview run on the CPU, which is
            what they do by default anyway — the result is identical and predictable, it just takes
            longer.
          </p>
        </div>
      ) : null}

      {/* -------------------------------------------------------- Encoders */}
      {hardware.encoders.length > 0 ? (
        <Section
          title="Encoders"
          caption={
            usable.length > 0
              ? "Each of these encoded a test frame here. Export still defaults to software, because x264 at a given quality looks the same on every machine and a hardware encoder does not."
              : "Every encoder in this build was tried and refused. Exports run on the CPU."
          }
        >
          {[...usable, ...refused].map((encoder) => (
            <EncoderRow key={encoder.id} encoder={encoder} />
          ))}
        </Section>
      ) : null}

      {/* -------------------------------------------------------- Decoders */}
      {hardware.decoders.length > 0 ? (
        <Section
          title="Decoders"
          caption={
            decodersUsable.length > 0
              ? "Each of these decoded a real frame into a GPU surface. Decoding on the GPU is off by default because reading the frame back to system memory costs more than it saves; set CHUKCUT_DECODE=vaapi to measure it."
              : "Nothing decoded on the GPU here. Playback and export decode on the CPU."
          }
        >
          {[...decodersUsable, ...decodersRefused].map((decoder) => (
            <DecoderRow key={decoder.codec} decoder={decoder} />
          ))}
        </Section>
      ) : null}

      {hardware.decode_default ? (
        <p className="text-[11px] text-muted-foreground">
          Decode acceleration is currently{" "}
          <span className="font-mono text-foreground">{hardware.decode_default}</span>.
        </p>
      ) : null}
    </div>
  );
}

function Section({
  title,
  caption,
  children,
}: {
  title: string;
  caption: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2">
      <div>
        <h3 className="text-[12px] font-semibold text-panel-foreground">{title}</h3>
        <p className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">{caption}</p>
      </div>
      <ul className="flex flex-col gap-1">{children}</ul>
    </section>
  );
}

function EncoderRow({ encoder }: { encoder: HwEncoder }) {
  return (
    <Row
      usable={encoder.usable}
      title={encoder.label}
      subtitle={encoder.encoder_name}
      note={encoder.note}
    />
  );
}

function DecoderRow({ decoder }: { decoder: HwDecoder }) {
  // "In the build and claims VAAPI, but refused a real frame" is the exact
  // failure the probe exists to catch, so it gets said rather than collapsed
  // into a generic "not supported".
  const note =
    decoder.note ??
    (decoder.declares_vaapi
      ? "The decoder claims hardware support and did not produce a GPU surface for a real frame."
      : decoder.in_build
        ? "This decoder has no hardware path in this FFmpeg build."
        : "No decoder for this codec in this FFmpeg build.");

  return (
    <Row
      usable={decoder.usable}
      title={DECODE_CODEC_LABELS[decoder.codec] ?? decoder.codec}
      subtitle={decoder.decoder_name}
      note={decoder.usable ? null : note}
    />
  );
}

function Row({
  usable,
  title,
  subtitle,
  note,
}: {
  usable: boolean;
  title: string;
  subtitle: string;
  note: string | null;
}) {
  return (
    <li className="flex items-start gap-2.5 rounded-sm border border-border bg-surface/40 px-2.5 py-2">
      <span
        aria-hidden
        className={cn(
          "mt-px grid size-4 shrink-0 place-items-center rounded-full",
          usable ? "bg-primary/20 text-primary" : "bg-muted text-muted-foreground",
        )}
      >
        {usable ? <CheckIcon className="size-3" /> : <XIcon className="size-3" />}
      </span>
      <div className="min-w-0 flex-1">
        <p className="flex items-baseline gap-1.5 text-[12px]">
          <span className="font-medium">{title}</span>
          <span className="font-mono text-[10px] text-muted-foreground">{subtitle}</span>
          <span className="sr-only">{usable ? "usable" : "refused"}</span>
        </p>
        {note ? (
          <p className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">{note}</p>
        ) : null}
      </div>
    </li>
  );
}
