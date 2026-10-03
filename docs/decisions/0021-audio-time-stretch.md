# 0021 — Audio time stretch and effects: Signalsmith Stretch, rendered per clip

Status: accepted (2026-10-03, audio-tools agent)

## Decision

A clip's sound is **rendered per clip** by `modules/audiofx`: source → time
map (constant speed or speed curve) → effect stack. The time stretch and the
pitch shift use **Signalsmith Stretch** (MIT, header-only C++) through the
`signalsmith-stretch` crate (MIT, `cc` + `bindgen`). Every other effect
(equalisers, compressor, Freeverb, echo, telephone, megaphone, robot) is our
own Rust.

The export renders each such clip inline at its own rate. The preview plays a
48 kHz render of the same spec from `cache_root()/audiofx/<hash>.wav` (32-bit
float), made by a background thread; until it lands, a constant-speed clip
plays as before (resampled, dry) and a curved clip is silent. A test pins the
preview's samples to the export's (`tests/audio_tools.rs`, worst difference
below 1e-5).

Code: `crates/engine/src/modules/audiofx/` (`stretch.rs` is the driver that
follows a curve, `render.rs` the per-clip render, `cache.rs` the preview
side).

## Options compared

| Option | Licence | Build | Quality | Curves | Verdict |
|---|---|---|---|---|---|
| **Signalsmith Stretch** via `signalsmith-stretch` 0.1.3 | MIT (library and binding) | one C++14 file through `cc`, bindings through `bindgen` (libclang, already needed by whisper-rs) | phase vocoder with multi-band phase locking; clean on speech and music from 0.5x to 2x, usable to 10x; formant control for the voice presets | yes: every `process` call takes its own input/output ratio | **chosen** |
| Rubber Band | GPL-2+ or commercial | large C++ build, FFTW or KissFFT | the reference for quality | yes | rejected: a much larger build for a small gain; GPL would be acceptable (the app is GPL-3) but not worth it |
| Our own WSOLA | ours | none | good on speech, smears music and transients | yes | rejected: Signalsmith is better at everything WSOLA is good at, and pitch shift comes with it |
| Our own phase vocoder | ours | `realfft` | phasey without phase locking; weeks to get to Signalsmith's level | yes | rejected |
| FFmpeg `atempo` | LGPL | in FFmpeg | WSOLA, 0.5x–100x per instance | no: one rate per filter graph | rejected: cannot follow a curve, and runs outside our mixer |
| `timestretch` crate | MIT | pure Rust | aimed at EDM loops, young | partly | rejected: young, single maintainer, unproven on speech |

## Why rendered per clip, not live in the mixer

A phase vocoder, a reverb and a compressor all have state. In the preview
mixer — which computes each 10 ms block from the playhead so that a seek and a
continuation are the same thing — they would start cold at every seek, and the
preview would sound different from the export. Rendering a clip front to back
is the same samples every time. `voice::denoise` made the same argument for
RNNoise (decision 0015) and the shape is deliberately the same.

## Alignment, measured

Signalsmith documents an input and an output latency (2880 frames each at
48 kHz with the default preset). Which way round they combine was **measured**
against a click: output stream index `m` shows the input fed up to `Lo` output
frames earlier, less `Li`. The driver feeds `map(m) + Li` by stream index `m`
and drops the first `Lo` frames. A click lands within 1 ms of where the map
puts it from 0.5x to 2x; at 4x it arrives about 9 ms early (a 120-sample burst
against a 5760-sample analysis window). The driver pre-rolls with real source
from before the clip's in point.

## What it costs

- **Time**: on one core of the owner's desktop, 60 s of output takes 1.1 s at
  0.5x and 0.9 s at 2x (53–66x real time); the pitch shift is the same cost;
  compressor, reverb and robot run at 600–700x, the EQ and echo faster still.
  An effect change on a one-minute clip is heard in the preview about a second
  later.
- **Disk**: 32-bit float stereo at 48 kHz, 23 MB per minute of rendered clip,
  per distinct setting. Cleared with the rest of the cache. Float, because an
  equaliser boost must not clip on the way through the cache.
- **Memory**: a render holds its source span and its output in RAM (about
  46 MB per minute of clip); an hour-long clip with an effect needs ~3 GB
  while it renders.
- **Export**: the export renders every processed clip again rather than
  reading the cache, so an export at 44.1 kHz renders at 44.1 kHz instead of
  resampling a 48 kHz render, and tests with synthetic sources never touch the
  user's cache.
- **Latency on edits**: a trim of a processed clip changes its spec and
  re-renders it; until then the preview plays the old behaviour.

## What would change our minds

- Users hear artefacts on extreme ramps (beyond 4x): try the binding's
  `preset_cheaper` vs `preset_default` at high rates, or Rubber Band's R3
  engine behind a feature.
- Exports of long talks with effects become slow: let the export read the
  48 kHz cache when the export rate is 48 kHz (the spec hash already makes
  that safe).
- A per-clip render is too slow for live slider drags: render a short window
  around the playhead first, then the rest.
