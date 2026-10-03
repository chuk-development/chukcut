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

Decisions already recorded elsewhere, because they predate this directory:

- ~~The UI runs in a webview~~ — reversed by [0011](0011-native-ui-on-gpui.md).
- **Preview frames leave over a custom URI scheme**, not through `invoke()` —
  `../architecture/preview-pipeline.md`.
- **All document mutation goes through invertible edit commands** —
  `../architecture/timeline-editing.md`.
