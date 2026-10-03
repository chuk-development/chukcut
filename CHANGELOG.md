# Changelog

All notable changes to chukcut. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the version
numbers follow [Semantic Versioning](https://semver.org/). Before 1.0, a minor
version can change the project file format; the app migrates old projects
when it opens them.

## [Unreleased]

## [0.1.0] - 2026-10-03

The first version. It covers the work from the first commit (2026-07-25) to
the native-UI build-out of October 2026. [`docs/STATUS.md`](docs/STATUS.md)
records what each feature was verified against, and what is still rough.

### Engine

- Document model with invertible edit commands: every edit, including a
  multi-clip edit, undoes as one step. Times are exact `i64` microseconds.
- Media import of any format FFmpeg reads. The canvas and frame rate come from
  the first clip. Missing media is a state, not a deletion: an offline clip
  draws a placeholder and the export names it.
- GPU compositor on wgpu and Vulkan: transform, crop, opacity, rotation,
  keyframes, blend in linear light. The preview and the export use the same
  shaders and give the same pixels.
- Hardware decode: VAAPI on Intel and AMD with zero-copy DMA-BUF import into
  the compositor; NVDEC on NVIDIA through NV12 textures. H.264, HEVC, VP9 and
  AV1. Software decode as the fallback.
- Audio playback through cpal (ALSA), with the audio device as the clock.
- Hardware preview JPEG on VAAPI, drawn straight into a VA surface.
- Proxy media generation (decision 0003).
- One GPU device and one VAAPI device per process, shared by the preview,
  the export and the tests.
- Crash recovery, autosave and a clean close.
- A benchmark suite, `chukcut-bench`, that generates its own media and refuses
  to report on a busy machine.

### Native app

- The whole UI moved from a webview to GPUI, Zed's GPU UI toolkit, in one
  process with the engine (decision 0011).
- CapCut-style layout: title bar, asset panel, player, inspector and timeline.
- Our own design language: graphite surfaces, an aqua accent, our own icon
  set and component kit (`docs/design/language.md`). Panels give way on
  narrow windows.
- Start screen with recent projects, autosave restore, an unsaved-changes
  guard, settings (proxies, cache, hardware) and a shortcuts sheet.

### Editing

- Magnetic timeline: razor at the pointer, ripple trim and delete, track
  management, markers, in and out marks on the ruler.
- Multi-select by Ctrl-click, Shift-click and rubber band; cut, copy, paste
  and duplicate. A paste never overlaps.
- Linked audio and video, detach and relink audio, audio fades with handles,
  waveforms and thumbnails on clips.
- Keyframes on clips, speed, crop, transform and opacity in the inspector.
- Titles with stroke, shadow and box, the same in preview and export.

### Colour

- The full Adjust tab: exposure, temperature, tint, saturation, vibrance,
  contrast, highlights, shadows, whites, blacks, sharpen, clarity, fade,
  vignette and grain.
- HSL, curves and colour wheels (shadows, midtones, highlights, offset).
- `.cube` LUTs, 1D and 3D, with a library; one-click filters.

### Effects and transitions

- 17 GPU effects for short-form video: glow, shake, light sweep, RGB split,
  glitch, blur, zoom blur, vignette, film grain, halation, bloom and others.
- Effects on a clip, or as an effect clip on its own lane that applies to
  everything beneath it.
- Picture in picture and split-screen layouts with rounded corners.
- Transitions on the timeline: basic transitions, 120 transitions from the
  gl-transitions library ported to WGSL, and 5 seamless transitions.

### Motion and tracking

- In, Out and Combo animation presets, stored as parameters relative to the
  clip, with an easing library (decision 0012).
- A text animator by letter, word or line.
- Punch-in zoom, and automatic zoom on jump cuts.
- Motion tracking: a Rust KLT tracker, a box selected on the player, track
  smoothing, re-tracking, and a follow link that keeps an overlay on the
  object. A track can be baked to keyframes (decision 0014).

### Captions and speech

- Automatic captions, offline with whisper.cpp (optional CUDA build) or
  through any OpenAI-compatible transcription server (decision 0013).
- Word and sentence captions, a caption lane, styles, karaoke highlight and
  emoji.
- SRT and VTT import and export.
- Caption translation with DeepL or an OpenAI-compatible chat model.
- Text to speech with ElevenLabs or an OpenAI-compatible server; word timing
  becomes captions. Sound effects and music with ElevenLabs.

### Audio cleanup

- Silence detection and cutting with a review list, and filler-word cutting.
- Voice cleanup with RNNoise (decision 0015).
- A loudness target (EBU R128) on export.

### Export

- H.264 and HEVC with AAC. Hardware encode with NVENC and VAAPI; each
  encoder is trial-encoded before the dialog offers it. Software x264 and
  x265 as the fallback.
- Range export, presets, estimated size, and frame snapshots as PNG.
- A licence summary for stock media, and a credits file beside the video
  when something needs credit.

### Integrations

- Accounts for ElevenLabs, fal.ai, Pexels, Pixabay, Freesound, DeepL and
  OpenAI-compatible servers. Keys stay in `secrets.toml` with mode 0600.
- Stock search (Pexels, Pixabay, Freesound), and fal.ai background removal,
  upscaling and frame interpolation with the price shown first.

### Packaging and project

- Released under GPL-3.0-or-later (decision 0010).
- A release profile with thin LTO, `scripts/install.sh` for a user-local
  install without sudo, a desktop entry, a `.chukcut` MIME type, AppStream
  metadata and an app icon. A release tarball recipe in `packaging/`.
- CI runs formatting, clippy, the engine and app tests, a release build and
  packaging checks.

### Known gaps

- The effect runtime for third-party packages cannot open packages that
  store assets in the binary encoding.
- The text animator composites on the CPU, which is slow in export.
- Colour "Save as preset", auto adjust, colour match and the Mask sub-tab are
  placeholders.
- Tracking does not re-find an object after it leaves the frame.

[Unreleased]: https://github.com/chuk-development/chukcut/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/chuk-development/chukcut/releases/tag/v0.1.0
