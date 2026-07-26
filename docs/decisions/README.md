# Decisions

One file per architectural decision that would be expensive to revisit, written
when it is made rather than reconstructed later.

Each says what was decided, why, what it costs, and what would make us change
our minds. They are not neutral surveys — a decision document that does not
recommend anything is a survey, and surveys are in `../research/`.

| # | Decision | Status |
|---|---|---|
| [0001](0001-keep-the-custom-media-stack.md) | Keep the custom media stack rather than adopting GStreamer/GES or MLT | Decided 2026-07-25 |
| [0002](0002-ffmpeg-and-hardware-encoding.md) | Ship our own LGPL-clean FFmpeg; encode on the GPU; no system dependencies | Decided 2026-07-25, not implemented |
| [0003](0003-proxy-media.md) | Proxy media: the rule for when one is worth making, all-intra H.264, and a type that keeps proxies out of the export | Decided 2026-07-26 |
| [0004](0004-the-app-shell-asks-before-it-decides.md) | The shell opens onto a start screen rather than an invented project; the unsaved guard; what settings can and cannot take effect live; the hardware and crash-recovery seams | Decided 2026-07-26 |
| [0005](0005-linked-audio-and-video.md) | An imported file with both streams becomes two linked clips; linkage lives in `Segment::extras`, not on `Segment` and not in a side table; linked edits expand in `History::apply` | Decided 2026-07-26 |

Decisions already recorded elsewhere, because they predate this directory:

- **The UI runs in a webview**, not in native Rust — `../architecture/overview.md`.
- **Preview frames leave over a custom URI scheme**, not through `invoke()` —
  `../architecture/preview-pipeline.md`.
- **All document mutation goes through invertible edit commands** —
  `../architecture/timeline-editing.md`.
