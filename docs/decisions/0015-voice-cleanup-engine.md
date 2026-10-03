# 0015 — Voice cleanup: RNNoise in Rust, rendered to a cache file

Status: accepted (2026-10-03, silence agent)

## Decision

"Reduce noise" on a clip runs **RNNoise through the `nnnoiseless` crate**
(a pure-Rust port with Xiph's model compiled in), renders the whole file once
into a 48 kHz WAV under `cache_root()/voice/`, and both mixers read that file
instead of the original. Strength is a dry/wet mix baked into the render,
quantised to 5 %. Loudness is measured with the `ebur128` crate.

Code: `crates/engine/src/modules/voice/` (cleanup block, render, commands),
`crates/engine/src/modules/loudness/` (R128, limiter, export target).

## Options compared

| Option | Licence | Size / build cost | Quality on speech | Verdict |
|---|---|---|---|---|
| **`nnnoiseless` (RNNoise)** | BSD-3-Clause, model included (Xiph/Mozilla, BSD) | 88 KB of weights, a few small crates, no C | Good on steady noise (hum, fan, hiss, room); weaker on music and babble | **chosen** |
| DeepFilterNet 3 (`deep_filter`) | MIT / Apache-2.0 | pulls `tract` (ONNX runtime in Rust): a large compile and several MB of model | Best of the three; also handles non-stationary noise | later, behind a feature |
| FFmpeg `arnndn` | the filter is LGPL; the models in GregorR/rnnoise-models have no clear licence file | needs a model file shipped next to the binary | same network as RNNoise | rejected: licence of the model files, and a file to ship |
| FFmpeg `afftdn` | LGPL, in every FFmpeg | nothing extra | spectral subtraction; "musical noise" at strong settings | rejected as default; fine for a "light" mode later |

RNNoise wins because it is **the only option with a clean licence, nothing to
download, nothing to build in C, and no new runtime**, and it is good at the
noise a phone or webcam recording actually has. It also yields a per-frame
voice probability for free, which `modules/silence` uses for its "voice"
detection mode — so Silero VAD (an ONNX model through `ort`) was not needed
either.

## Why rendered, not live

RNNoise is recurrent: an output frame depends on every frame before it. Run
inside the preview mixer, a seek would start the network cold and sound
different from playing through, and the preview and the export would differ —
the one thing this feature must never do. A render front to back is the same
samples every time, in preview and export, by construction.

The render drops RNNoise's one-frame (480 sample) output delay so source time
`t` of the cache is source time `t` of the original; a unit test proves the
alignment by correlation. A missing cache is re-rendered by the export before
it mixes (`denoise::ensure_rendered`), using the engine id the document
recorded.

## What it costs

- Disk: 16-bit stereo 48 kHz, about 11.5 MB per minute of source, per
  strength used. Cleared with the rest of the cache.
- Time: a full decode plus RNNoise per change of strength. RNNoise is far
  faster than real time on one core [est]; the decode dominates for video
  files.
- A strength change re-renders; the slider therefore offers three steps
  (light, medium, strong) rather than a continuous drag.

## What would change our mind

- Users with music or crowd noise under speech: add DeepFilterNet behind a
  cargo feature as a second engine id; the document already records which
  engine rendered a clip.
- If cache size becomes a complaint: render FLAC instead of WAV.
