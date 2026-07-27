/**
 * The coercions, and the payload they produce.
 *
 * Rust validates the same rules and answers in prose, so none of this is about
 * safety — it is about the dialog never assembling a combination it already
 * knows will come back refused, and about `buildRequest` producing the exact
 * snake_case shape `ExportRequest` deserializes.
 */

import { describe, expect, it } from "vitest";

import type { HwEncoder } from "@/modules/export/lib/api";
import {
  applyRemembered,
  buildRequest,
  containerAccepts,
  crfRange,
  defaultFileName,
  dirOf,
  type ExportForm,
  estimateFileSize,
  estimateVideoBitrate,
  exportBlockedReason,
  formatBitrate,
  formatBytes,
  formatFps,
  formFromPreset,
  fpsToNumber,
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
import {
  CUSTOM_PRESET,
  makeProject,
  NVENC_H264,
  VAAPI_H264,
  YOUTUBE_4K,
  YOUTUBE_1080P,
} from "@/test/fixtures";

const NVENC_H265: HwEncoder = {
  ...NVENC_H264,
  id: "nvenc_h265",
  codec: "h265",
  encoder_name: "hevc_nvenc",
  label: "H.265 / HEVC (NVIDIA NVENC)",
};

function form(overrides: Partial<ExportForm> = {}): ExportForm {
  return {
    ...formFromPreset(YOUTUBE_1080P, null),
    outputPath: "/home/me/cut.mp4",
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Seeding the form
// ---------------------------------------------------------------------------

describe("the form a preset produces", () => {
  it("takes the preset's own numbers", () => {
    const seeded = formFromPreset(YOUTUBE_4K, makeProject());

    expect(seeded.width).toBe(3840);
    expect(seeded.height).toBe(2160);
    expect(seeded.fps).toBe(30);
    expect(seeded.videoCodec).toBe("h265");
    expect(seeded.quality).toEqual({ kind: "crf", value: 22 });
  });

  it("follows the project for `custom`, not the template's placeholder size", () => {
    // The template says 1080×1920; the open project says otherwise, and an
    // export that changes the aspect ratio the user framed is the worst
    // surprise there is.
    const project = makeProject({
      canvas: { width: 1440, height: 1080, background: [0, 0, 0, 1] },
    });
    project.fps = 29.97;

    const seeded = formFromPreset(CUSTOM_PRESET, project);

    expect([seeded.width, seeded.height]).toEqual([1440, 1080]);
    expect(seeded.fps).toBe(29.97);
  });

  it("rounds an odd canvas down rather than offering a resolution 4:2:0 cannot hold", () => {
    const project = makeProject({ canvas: { width: 1081, height: 607, background: [0, 0, 0, 1] } });

    const seeded = formFromPreset(CUSTOM_PRESET, project);

    expect([seeded.width, seeded.height]).toEqual([1080, 606]);
  });

  it("carries the encoder, the audio decision and the destination across a preset change", () => {
    const previous = form({ hardwareId: "nvenc_h264", includeAudio: false });

    const seeded = formFromPreset(YOUTUBE_4K, null, previous);

    // Changing preset is a change of destination, not a change of machine.
    expect(seeded.hardwareId).toBe("nvenc_h264");
    expect(seeded.includeAudio).toBe(false);
    expect(seeded.outputPath).toBe("/home/me/cut.mp4");
    expect(seeded.width).toBe(3840);
  });
});

// ---------------------------------------------------------------------------
// Container and codec
// ---------------------------------------------------------------------------

describe("what a container will carry", () => {
  it("agrees with Container::accepts", () => {
    expect(containerAccepts("mp4", "h264", "aac")).toBe(true);
    expect(containerAccepts("mp4", "av1", "aac")).toBe(true);
    // MP4 can technically hold Opus; enough players choke that Rust refuses it.
    expect(containerAccepts("mp4", "h264", "opus")).toBe(false);
    expect(containerAccepts("mp4", "vp9", "aac")).toBe(false);
    expect(containerAccepts("webm", "vp9", "opus")).toBe(true);
    expect(containerAccepts("webm", "h264", "opus")).toBe(false);
    expect(containerAccepts("webm", "vp9", "aac")).toBe(false);
    // Matroska is the escape hatch.
    expect(containerAccepts("mkv", "h265", "opus")).toBe(true);
  });
});

describe("resolving the form", () => {
  it("swaps the codecs when WebM is chosen", () => {
    const resolved = resolveSettings(form({ container: "webm" }), []);

    expect(resolved.videoCodec).toBe("vp9");
    expect(resolved.audioCodec).toBe("opus");
    expect(containerAccepts(resolved.container, resolved.videoCodec, resolved.audioCodec)).toBe(
      true,
    );
  });

  it("restores the preset's codec when the container goes back to one that carries it", () => {
    // The bug this exists for: pick WebM on the 4K preset, pick MP4 again, and
    // an implementation that overwrote the codec has silently downgraded an
    // H.265 export to H.264.
    const seeded = formFromPreset(YOUTUBE_4K, null);

    expect(resolveSettings({ ...seeded, container: "webm" }, []).videoCodec).toBe("vp9");
    expect(resolveSettings({ ...seeded, container: "mp4" }, []).videoCodec).toBe("h265");
  });

  it("lets a hardware encoder decide the codec, and bends the container around it", () => {
    // Choosing "H.265 (NVENC)" is choosing a codec as much as an encoder —
    // `resolve_settings` on the Rust side does exactly this.
    const resolved = resolveSettings(form({ container: "webm", hardwareId: "nvenc_h265" }), [
      NVENC_H265,
    ]);

    expect(resolved.videoCodec).toBe("h265");
    // WebM cannot carry HEVC, so the container has to move rather than the file
    // failing at `MediaWriter::create`.
    expect(resolved.container).toBe("mp4");
    expect(resolved.hardware?.id).toBe("nvenc_h265");
  });

  it("ignores an encoder that is listed but not usable", () => {
    const resolved = resolveSettings(form({ hardwareId: "vaapi_h264" }), [VAAPI_H264]);

    // Sending it would come back as the encoder's own note; falling back to
    // software here means the request is one Rust can accept.
    expect(resolved.hardware).toBeNull();
  });

  it("ignores an encoder id this machine does not have", () => {
    expect(resolveSettings(form({ hardwareId: "nvenc_h264" }), []).hardware).toBeNull();
  });

  it("rounds the resolution to even in both dimensions", () => {
    const resolved = resolveSettings(form({ width: 1281, height: 721 }), []);

    expect([resolved.width, resolved.height]).toEqual([1280, 720]);
  });

  it("clamps the quality into the range the resolved codec accepts", () => {
    // 60 is legal for VP9 (0..63) and rejected for H.264 (0..51).
    expect(crfRange("h264")).toEqual([0, 51]);
    expect(crfRange("vp9")).toEqual([0, 63]);

    const asVp9 = resolveSettings(
      form({ container: "webm", quality: { kind: "crf", value: 60 } }),
      [],
    );
    expect(asVp9.quality).toEqual({ kind: "crf", value: 60 });

    const asH264 = resolveSettings(
      form({ container: "mp4", quality: { kind: "crf", value: 60 } }),
      [],
    );
    expect(asH264.quality).toEqual({ kind: "crf", value: 51 });
  });
});

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

describe("the request Rust is sent", () => {
  it("is snake_case and carries every field ExportRequest declares", () => {
    const chosen = form();
    const request = buildRequest(chosen, resolveSettings(chosen, []));

    expect(request).toEqual({
      output_path: "/home/me/cut.mp4",
      preset_id: "youtube_1080p",
      overrides: {
        width: 1920,
        height: 1080,
        fps: 30,
        video_codec: "h264",
        quality: { kind: "crf", value: 20 },
        audio_codec: "aac",
        container: "mp4",
      },
      hardware: null,
      include_audio: true,
      range: null,
    });
  });

  it("carries the range as a two-element array, the shape of a Rust tuple", () => {
    const chosen = form();
    const request = buildRequest(chosen, resolveSettings(chosen, []), {
      start: 500_000,
      end: 1_500_000,
    });

    expect(request.range).toEqual([500_000, 1_500_000]);
  });

  it("sends the resolved values, not the raw ones the user typed", () => {
    const chosen = form({ container: "webm", width: 1281, hardwareId: "vaapi_h264" });
    const request = buildRequest(chosen, resolveSettings(chosen, [VAAPI_H264]));

    expect(request.overrides).toMatchObject({
      width: 1280,
      video_codec: "vp9",
      audio_codec: "opus",
      container: "webm",
    });
    // The unusable encoder never reaches Rust.
    expect(request.hardware).toBeNull();
  });

  it("names the hardware encoder by its id when one was picked", () => {
    const chosen = form({ hardwareId: "nvenc_h264" });
    const request = buildRequest(chosen, resolveSettings(chosen, [NVENC_H264]));

    expect(request.hardware).toBe("nvenc_h264");
    expect(request.overrides?.video_codec).toBe("h264");
  });

  it("passes the audio toggle through as `include_audio`", () => {
    const chosen = form({ includeAudio: false });

    expect(buildRequest(chosen, resolveSettings(chosen, [])).include_audio).toBe(false);
  });

  it("refuses to build one without a destination the user chose", () => {
    const chosen = form({ outputPath: null });

    expect(() => buildRequest(chosen, resolveSettings(chosen, []))).toThrow();
  });
});

// ---------------------------------------------------------------------------
// Names and reasons
// ---------------------------------------------------------------------------

describe("the file extension shown to the user", () => {
  it("matches what `with_extension` will do to the path", () => {
    expect(withExtension("/tmp/clip.webm", "mp4")).toBe("/tmp/clip.mp4");
    expect(withExtension("/tmp/clip", "mp4")).toBe("/tmp/clip.mp4");
    // A name with dots in it keeps them.
    expect(withExtension("/tmp/my.clip.v2.mp4", "mp4")).toBe("/tmp/my.clip.v2.mp4");
    // Case is not a difference.
    expect(withExtension("/tmp/clip.MP4", "mp4")).toBe("/tmp/clip.MP4");
    // A dotfile has no extension to replace.
    expect(withExtension("/tmp/.hidden", "mp4")).toBe("/tmp/.hidden.mp4");
    expect(withExtension("C:\\videos\\clip.mov", "mkv")).toBe("C:\\videos\\clip.mkv");
  });

  it("suggests the project name with the container's extension", () => {
    expect(defaultFileName("Holiday", "mp4")).toBe("Holiday.mp4");
    expect(defaultFileName("Holiday.chukcut", "webm")).toBe("Holiday.webm");
    expect(defaultFileName(undefined, "mkv")).toBe("Untitled.mkv");
    expect(defaultFileName("   ", "mp4")).toBe("Untitled.mp4");
    // A dot in the name is not an extension to eat.
    expect(defaultFileName("Holiday.v2", "mp4")).toBe("Holiday.v2.mp4");
  });
});

describe("why the Export button is off", () => {
  it("names the one thing the user has to do", () => {
    const project = makeProject();

    expect(exportBlockedReason(null, 0, null)).toMatch(/project/i);
    expect(exportBlockedReason(project, 0, "/tmp/a.mp4")).toMatch(/empty/i);
    expect(exportBlockedReason(project, 2_000_000, null)).toMatch(/where/i);
    expect(exportBlockedReason(project, 2_000_000, "/tmp/a.mp4")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Resolution options
// ---------------------------------------------------------------------------

describe("the resolution options", () => {
  it("keeps the aspect while scaling the long edge, for either orientation", () => {
    // Vertical: the long edge is the height.
    const vertical = resolutionOptions({ width: 1080, height: 1920 });
    expect(vertical[0]).toMatchObject({ id: "source", width: 1080, height: 1920 });
    expect(vertical.find((o) => o.id === "720")).toMatchObject({ width: 404, height: 720 });
    expect(vertical.find((o) => o.id === "2160")).toMatchObject({ width: 1214, height: 2160 });

    // Landscape: the long edge is the width, so "2160" means 2160 wide — the
    // same rule in both orientations, not the 16:9 "p" ladder.
    const landscape = resolutionOptions({ width: 1920, height: 1080 });
    expect(landscape.find((o) => o.id === "2160")).toMatchObject({ width: 2160, height: 1214 });
    expect(landscape.find((o) => o.id === "720")).toMatchObject({ width: 720, height: 404 });
  });

  it("drops the long-edge entry that duplicates the canvas", () => {
    const options = resolutionOptions({ width: 1080, height: 1920 });
    // A 1920-long-edge option would be the canvas again — and 1920 is not in
    // the list anyway; but 1080×1920's own "1080" entry is 606×1080, kept.
    const landscape = resolutionOptions({ width: 1920, height: 1080 });
    expect(landscape.filter((o) => o.width === 1920 && o.height === 1080)).toHaveLength(1);
    expect(options[0].id).toBe("source");
  });

  it("every option is even in both dimensions, whatever the canvas", () => {
    for (const option of resolutionOptions({ width: 1081, height: 607 })) {
      expect(option.width % 2).toBe(0);
      expect(option.height % 2).toBe(0);
    }
  });

  it("matches a size back to its option, or to nothing when hand-typed", () => {
    const options = resolutionOptions({ width: 1080, height: 1920 });
    expect(matchingResolution(options, 1080, 1920)?.id).toBe("source");
    expect(matchingResolution(options, 404, 720)?.id).toBe("720");
    expect(matchingResolution(options, 500, 700)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Estimates
// ---------------------------------------------------------------------------

describe("the estimated bitrate and size", () => {
  it("returns a bitrate target verbatim — that mode is exact by definition", () => {
    expect(
      estimateVideoBitrate("h264", { kind: "bitrate", value: 12_000_000 }, 1920, 1080, 30),
    ).toBe(12_000_000);
  });

  it("lands near the presets' own calibration at CRF 20, 1080p30", () => {
    const rate = estimateVideoBitrate("h264", { kind: "crf", value: 20 }, 1920, 1080, 30);
    // The YouTube preset's comment says 8–12 Mbit/s.
    expect(rate).toBeGreaterThan(8_000_000);
    expect(rate).toBeLessThan(12_000_000);
  });

  it("moves the right way with every input", () => {
    const at = (crf: number, w = 1920, h = 1080, fps = 30, codec: "h264" | "h265" = "h264") =>
      estimateVideoBitrate(codec, { kind: "crf", value: crf }, w, h, fps);

    // Lower CRF is more bits; six points is about a doubling.
    expect(at(14)).toBeGreaterThan(at(20) * 1.8);
    expect(at(26)).toBeLessThan(at(20) * 0.6);
    // More pixels and more frames are more bits.
    expect(at(20, 3840, 2160)).toBeGreaterThan(at(20) * 3);
    expect(at(20, 1920, 1080, 60)).toBeCloseTo(at(20) * 2, -4);
    // HEVC needs less than H.264 for the same look.
    expect(at(20, 1920, 1080, 30, "h265")).toBeLessThan(at(20));
  });

  it("multiplies out to bytes: duration times the summed rates over eight", () => {
    // 8 Mb/s video + 192 kb/s audio for 10 s = 81.92 Mbit = 10.24 MB.
    expect(estimateFileSize(8_000_000, 192_000, 10_000_000)).toBe(10_240_000);
    expect(estimateFileSize(8_000_000, 0, 0)).toBe(0);
    // Doubling the duration doubles the size.
    expect(estimateFileSize(8_000_000, 192_000, 20_000_000)).toBe(
      2 * estimateFileSize(8_000_000, 192_000, 10_000_000),
    );
  });

  it("captions the slider in words, on a scale shared across codecs", () => {
    expect(qualityCaption("h264", 20)).toBe("High quality");
    expect(qualityCaption("h264", 10)).toMatch(/lossless/i);
    expect(qualityCaption("h264", 40)).toMatch(/compressed/i);
    // VP9's 0..63 scale: 25 of 63 is about 20 of 51, the same band.
    expect(qualityCaption("vp9", 25)).toBe("High quality");
  });

  it("formats rates and sizes the way people read them", () => {
    expect(formatBitrate(9_940_000)).toBe("9.9 Mb/s");
    expect(formatBitrate(192_000)).toBe("192 kb/s");
    expect(formatBytes(10_240_000)).toBe("10 MB");
    expect(formatBytes(2_500_000_000)).toBe("2.50 GB");
    expect(formatBytes(999_000)).toBe("999 kB");
  });
});

// ---------------------------------------------------------------------------
// The export range, read defensively
// ---------------------------------------------------------------------------

describe("normalizing another store's exportRange", () => {
  const DURATION = 2_000_000;

  it("passes a clean range through", () => {
    expect(normalizeExportRange({ start: 100, end: 900 }, DURATION)).toEqual({
      start: 100,
      end: 900,
    });
  });

  it("clamps to the timeline and orders the ends", () => {
    expect(normalizeExportRange({ start: -50, end: 99_000_000 }, DURATION)).toEqual({
      start: 0,
      end: DURATION,
    });
    expect(normalizeExportRange({ start: 900, end: 100 }, DURATION)).toEqual({
      start: 100,
      end: 900,
    });
  });

  it("answers null for absent, malformed or empty ranges", () => {
    expect(normalizeExportRange(undefined, DURATION)).toBeNull();
    expect(normalizeExportRange(null, DURATION)).toBeNull();
    expect(normalizeExportRange("1..2", DURATION)).toBeNull();
    expect(normalizeExportRange({ start: "0", end: 100 }, DURATION)).toBeNull();
    expect(normalizeExportRange({ start: Number.NaN, end: 100 }, DURATION)).toBeNull();
    expect(normalizeExportRange({ start: 500, end: 500 }, DURATION)).toBeNull();
    // Entirely past the timeline clamps to nothing.
    expect(normalizeExportRange({ start: 3_000_000, end: 4_000_000 }, DURATION)).toBeNull();
    // And an empty timeline has no range at all.
    expect(normalizeExportRange({ start: 0, end: 100 }, 0)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

describe("path arithmetic", () => {
  it("splits and joins without inventing separators", () => {
    expect(dirOf("/home/me/cut.mp4")).toBe("/home/me");
    expect(dirOf("cut.mp4")).toBe("");
    expect(stemOf("/home/me/cut.mp4")).toBe("cut");
    expect(stemOf("/home/me/my.clip.v2.mp4")).toBe("my.clip.v2");
    expect(stemOf("/home/me/.hidden")).toBe(".hidden");
    expect(joinPath("/home/me", "cut.mp4")).toBe("/home/me/cut.mp4");
    expect(joinPath("/home/me/", "cut.mp4")).toBe("/home/me/cut.mp4");
    expect(joinPath("C:\\videos", "cut.mp4")).toBe("C:\\videos\\cut.mp4");
  });

  it("sanitizes a file name into something a file system takes", () => {
    expect(sanitizeFileName("my cut")).toBe("my cut");
    expect(sanitizeFileName("a/b\\c")).toBe("a b c");
    expect(sanitizeFileName("...sneaky")).toBe("sneaky");
    expect(sanitizeFileName("   ")).toBe("Untitled");
  });
});

// ---------------------------------------------------------------------------
// Remembered settings
// ---------------------------------------------------------------------------

describe("remembering the form", () => {
  const BASE = { width: 1080, height: 1920 };

  it("round-trips choices, and stores the resolution as a choice", () => {
    const chosen = form({ width: 404, height: 720, includeAudio: false, container: "mkv" });
    const remembered = rememberForm(chosen, BASE);

    expect(remembered.long_edge).toBe(720);
    expect(remembered.include_audio).toBe(false);
    expect(remembered.container).toBe("mkv");
    expect(rememberedPresetId(remembered)).toBe("youtube_1080p");

    const seeded = form();
    const applied = applyRemembered(seeded, remembered, BASE);
    expect([applied.width, applied.height]).toEqual([404, 720]);
    expect(applied.includeAudio).toBe(false);
    expect(applied.container).toBe("mkv");
  });

  it("remembers the canvas as null, so a new project keeps its own shape", () => {
    const chosen = form({ width: 1080, height: 1920 });
    expect(rememberForm(chosen, BASE).long_edge).toBeNull();

    // Applied to a different project's seed, the sizes stay the seed's own.
    const other = form({ width: 1920, height: 1080 });
    const applied = applyRemembered(other, rememberForm(chosen, BASE), {
      width: 1920,
      height: 1080,
    });
    expect([applied.width, applied.height]).toEqual([1920, 1080]);
  });

  it("survives a blob any historical build could have written", () => {
    const seeded = form();
    for (const garbage of [
      null,
      42,
      "remembered",
      { container: 7, quality: "loud", fps: "-", long_edge: "many", include_audio: "yes" },
      { quality: { kind: "louder", value: 99 } },
    ]) {
      const applied = applyRemembered(seeded, garbage, BASE);
      expect(applied).toEqual(seeded);
    }
    expect(rememberedPresetId(null)).toBeNull();
    expect(rememberedPresetId({ preset_id: 9 })).toBeNull();
  });
});

describe("frame rates", () => {
  it("reconstructs the decimal from the exact fraction", () => {
    expect(fpsToNumber({ num: 30, den: 1 })).toBe(30);
    expect(fpsToNumber({ num: 30_000, den: 1001 })).toBeCloseTo(29.97, 4);
    // A malformed rate must not produce Infinity in a select value.
    expect(fpsToNumber({ num: 30, den: 0 })).toBe(0);
  });

  it("writes whole rates without a decimal point", () => {
    expect(formatFps(30)).toBe("30");
    expect(formatFps(29.97)).toBe("29.97");
    expect(formatFps(23.976)).toBe("23.976");
  });
});
