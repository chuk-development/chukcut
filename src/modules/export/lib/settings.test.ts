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
  buildRequest,
  containerAccepts,
  crfRange,
  defaultFileName,
  type ExportForm,
  exportBlockedReason,
  formatFps,
  formFromPreset,
  fpsToNumber,
  resolveSettings,
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
    });
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
