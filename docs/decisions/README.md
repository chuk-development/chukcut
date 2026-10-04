# Decisions

One file per architectural decision that would be expensive to revisit, written
when it is made rather than reconstructed later.

Each says what was decided, why, what it costs, and what would make us change
our minds. They are not neutral surveys — a decision document that does not
recommend anything is a survey, and surveys are in `../research/`.

| # | Decision | Status |
|---|---|---|
| [0001](0001-keep-the-custom-media-stack.md) | Keep the custom media stack rather than adopting GStreamer/GES or MLT | Decided 2026-07-25 |
| [0002](0002-ffmpeg-and-hardware-encoding.md) | Ship our own LGPL-clean FFmpeg; encode on the GPU; no system dependencies | Decided 2026-07-25. Hardware paths built; the LGPL-clean requirement is **superseded by [0010](0010-open-source-under-gpl.md)**, the bundling is not |
| [0003](0003-proxy-media.md) | Proxy media: the rule for when one is worth making, all-intra H.264, and a type that keeps proxies out of the export | Decided 2026-07-26 |
| [0004](0004-the-app-shell-asks-before-it-decides.md) | The shell opens onto a start screen rather than an invented project; the unsaved guard; what settings can and cannot take effect live; the hardware and crash-recovery seams | Decided 2026-07-26 |
| [0005](0005-linked-audio-and-video.md) | An imported file with both streams becomes two linked clips; linkage lives in `Segment::extras`, not on `Segment` and not in a side table; linked edits expand in `History::apply` | Decided 2026-07-26 |
| [0006](0006-in-app-menu-bar.md) | The window loses its decorations and we draw the title strip; the menu bar is React and takes the theme; the item table and its gates stay in Rust and cross as a resolved bar | Decided 2026-07-27 |
| [0007](0007-colour-as-materials.md) | Colour adjustments are pool materials, and edits are reference swaps | Decided 2026-07-27 |
| [0008](0008-one-undo-stack-two-command-kinds.md) | One undo stack, two command kinds | Decided 2026-07-27 |
| [0009](0009-missing-media-is-a-state-not-a-deletion.md) | Missing media is a state, not a deletion: removing an import is an undoable edit that leaves clips offline | Decided 2026-07-28 |
| [0010](0010-open-source-under-gpl.md) | Open source under GPL-3.0, not sold; which settles the FFmpeg linking question 0002 was built around, and does not settle codec patents | Decided 2026-08-05 |
| [0011](0011-native-ui-on-gpui.md) | The UI is native, on GPUI; the webview is gone; the engine is its own crate with no UI dependency; Linux only, NVIDIA and Intel first | Decided 2026-10-02. **Supersedes the webview split, 0004's React parts and 0006** |
| [0012](0012-animation-as-parameters.md) | Animations are parameters relative to the clip (In/Out/Combo, text animator, punch-in zoom), not baked keyframes, so they survive trims and splits | Decided 2026-10-03 |
| [0013](0013-local-transcription-with-whisper-cpp.md) | Offline captions run whisper.cpp through whisper-rs (feature `local-whisper`, CUDA opt-in), not candle: word timestamps, CPU speed, quantised models | Decided 2026-10-03 |
| [0014](0014-motion-tracks-are-materials-followed-by-link.md) | Motion tracks are materials; an overlay follows one through a link the compositor resolves via the tracked clip's source time, so trims, slips and speed changes keep it on the object | Decided 2026-10-03 |
| [0015](0015-voice-cleanup-engine.md) | Voice cleanup runs RNNoise (nnnoiseless, BSD-3) into a cached WAV both mixers read; loudness via ebur128 with a true-peak limiter on export | Decided 2026-10-03 |
| [0016](0016-effects-on-clips-and-effect-clips.md) | Built-in effects live on clips (in `extras`) and on effect clips (an effect lane, applying to everything beneath); both render as compositor layers; keyframes in source time | Decided 2026-10-03 |
| [0017](0017-cloud-results-carry-provenance.md) | Cloud results (TTS, stock, generated media) carry provenance and licence in an asset.json copied into the project's origins; export summarises licences and writes credits | Decided 2026-10-03 |

Decisions already recorded elsewhere, because they predate this directory:

- ~~The UI runs in a webview~~ — reversed by [0011](0011-native-ui-on-gpui.md).
- **Preview frames leave over a custom URI scheme**, not through `invoke()` —
  `../architecture/preview-pipeline.md`.
- **All document mutation goes through invertible edit commands** —
  `../architecture/timeline-editing.md`.
| [0020](0020-masks-keys-and-blend-modes.md) | Masks, chroma key and blend mode are one pool material per clip; masks and key are alpha in the quad shader, blend modes a layer pass over the frame so far | Decided 2026-10-03 |
| [0019](0019-analysis-without-a-model-results-in-the-document.md) | Scene detection, stabilisation, beats and reframe run on our own Rust code without a model; results live in the document as tagged extras in source time | Decided 2026-10-03 |
| [0021](0021-audio-time-stretch.md) | Speed changes, speed curves and audio effects are rendered per clip (Signalsmith Stretch for time and pitch, our own DSP for the rest); the export renders inline, the preview plays a cached 48 kHz render of the same spec | Decided 2026-10-03 |
| [0018](0018-speed-curves-anchored-to-source-time.md) | Speed curves are pool materials anchored to source time; one TimeMap maps timeline↔source everywhere | Decided 2026-10-03 |
| [0027](0027-preview-frames-shared-with-gpui.md) | Preview frames reach GPUI as exported GPU memory (an opaque-fd buffer), imported and drawn by a vendored, patched GPUI; the readback stays as the automatic fallback | Decided 2026-10-04 |
| [0025](0025-ml-worker-process.md) | Machine learning runs in a separate worker process on ONNX Runtime loaded at run time from pinned runtime packs; CUDA, then OpenVINO, then CPU; model pixels (mattes) are cache, edit data is document | Decided 2026-10-04 |
| [0022](0022-project-templates.md) | Project templates are projects whose picture clips carry a slot marker in `extras`, stored as a directory (manifest around an ordinary project, plus copied media); built-ins are built in code from our own parts; a fill trims, slows or crops media to the slot | Decided 2026-10-04 |
| [0024](0024-sequences-and-compound-clips.md) | Several timelines per project and compound clips: sequences are materials, the active one is swapped into `Project::tracks`, navigation is an undoable edit; compound clips render nested and mix flattened | Decided 2026-10-04 |
| [0026](0026-frame-blending-motion-blur-and-animated-stickers.md) | Frame blending and motion blur are weighted draws of one clip summed in a float layer and resolved into an ordinary clip layer; animated stickers (Lottie via velato + vello on the shared device, GIF/WebP via `image`) are image clips that loop or play once | Decided 2026-10-04 |
| [0028](0028-optical-flow-frames-and-matte-driven-masks.md) | "Optical flow (AI)" frames are made by RIFE in the ML worker, baked into the cache per source-frame pair and phase, and drawn in place of the blend; a clip's matte limits its own grade and effects to the subject or the rest | Decided 2026-10-04 |
| [0029](0029-remove-object-and-enhance-quality-are-remade-frames.md) | "Remove object" (LaMa on a selected, painted or boxed mask, helped by background memory, a clean plate and smoothing) and "Enhance quality" (Real-ESRGAN 2x/4x) remake a clip's frames into the cache, drawn in place of the decoded ones | Decided 2026-10-04 |
| [0030](0030-colour-ai-faces-and-voice-isolation.md) | Auto adjust and colour match fit ordinary grade controls on the shader's CPU twin; face landmarks are a cached track, retouch an effect placed from them, a followed face a motion track; voice isolation (HTDemucs) is a rendered cache before the denoise | Decided 2026-10-04 |
| [0031](0031-fast-ai-on-tensorrt.md) | "Fast" AI runs measured models (RIFE, Real-ESRGAN, LaMa, BiRefNet) on TensorRT from the fp32 files at a per-model precision, engines cached per model, GPU, driver, precision and shape, CUDA as fallback; TensorRT is a pinned add-on; no fp16 files; HTDemucs without constant folding | Decided 2026-10-04 |
| [0032](0032-body-landmarks-and-follow-body.md) | Body landmarks are RTMPose keypoints of people YOLOX finds, a cached track per media file; a followed body part is a motion track; auto reframe holds a body where no face shows; a zipped model unpacks only its pinned member | Decided 2026-10-04 |
| [0033](0033-release-builds.md) | Release builds come from a manual workflow: a .deb on the system FFmpeg and AppImages with FFmpeg bundled (system libva, libdrm, GPU loaders and C++ runtime), both tested in the job; macOS and Windows jobs are experimental and blocked by Linux-only preview and export code in the engine | Decided 2026-10-04 |
