# 0012 — Local transcription with whisper.cpp (whisper-rs)

Date: 2026-10-03. Status: accepted.

## Decision

Offline auto captions run **whisper.cpp through the `whisper-rs` crate**
(0.16, whisper.cpp built from source by `whisper-rs-sys`). It sits behind the
engine feature `local-whisper`, which the app turns on;
`local-whisper-cuda` (app: `--features cuda`) adds whisper.cpp's CUDA
backend. Models are whisper.cpp's GGML files from
`huggingface.co/ggerganov/whisper.cpp`, downloaded on first use into
`~/.cache/chukcut/whisper/` and checked against SHA-256 values pinned in
`modules/speech/models.rs`.

## Why not candle

candle's Whisper is pure Rust and would avoid a C++ build, but for this job it
loses on every axis that matters to a user:

- **Word timestamps.** Word captions and karaoke need a time per word.
  whisper.cpp produces them (token timestamps, `max_len = 1` +
  `split_on_word`). candle's example decoder gives segment times only; word
  timing would be our own DTW over cross-attention, which is a research
  project, not a feature.
- **CPU speed.** whisper.cpp's GGML kernels are hand-tuned (AVX2/AVX-512,
  OpenMP) and run quantised models (`q5_0`). Measured here: 11 s of speech
  with `tiny` in **3.7 s in a debug build**, on a machine at load 30. candle
  on CPU is several times slower for the same model.
- **Model size and memory.** The quantised `large-v3-turbo` is 548 MB on disk;
  the safetensors candle loads are 1.6 GB.
- **Maturity.** whisper.cpp is the reference local Whisper, with long-audio
  handling, language detection and non-speech suppression already right.

## What it costs

- A **CMake build of whisper.cpp** the first time the app is built (about two
  minutes) and `clang`/`libclang` for bindgen. Both are on the build machines.
  The engine's own tests do not enable the feature, so `cargo test -p
  chukcut-engine` does not pay for it.
- **CUDA is opt-in** because building ggml-cuda needs `nvcc` and takes long.
  Without it the CPU path is fast enough for `tiny`/`base`/`small`.
- **No Vulkan backend**, on purpose. whisper.cpp's Vulkan backend would open
  its own Vulkan instance next to wgpu's, which `CLAUDE.md` forbids
  ("concurrent Vulkan instances crash drivers"). CUDA is a separate API and is
  already in use for NVDEC.
- **A bug to remember.** whisper-rs 0.16's `set_abort_callback_safe` casts its
  user data to the wrong type, so whisper.cpp read garbage and aborted the
  encoder at once (`error -6`). `speech/local.rs` installs the raw callback with
  a pointer to an `AtomicBool` instead. Re-check this when bumping whisper-rs.

## What would change our minds

- whisper-rs stops tracking whisper.cpp, or the C++ build becomes a problem on
  a target we ship to: move to candle and accept segment-level timing plus our
  own alignment.
- A Rust speech model with native word timing and comparable CPU speed appears.
