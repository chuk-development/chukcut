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

Decisions already recorded elsewhere, because they predate this directory:

- **The UI runs in a webview**, not in native Rust — `../architecture/overview.md`.
- **Preview frames leave over a custom URI scheme**, not through `invoke()` —
  `../architecture/preview-pipeline.md`.
- **All document mutation goes through invertible edit commands** —
  `../architecture/timeline-editing.md`.
