# 0036 — Whisper runs on CUDA in a helper process, built whenever nvcc is there

Date: 2026-10-09. Status: accepted. Amends 0013.

## What was decided

Local transcription runs on the NVIDIA GPU when one is usable, and on the CPU
everywhere else, from one build that starts on every machine.

- **A helper process.** `chukcut-whisper-cuda` (`crates/whisper`, binary
  built only with its `cuda` feature) is whisper.cpp with ggml's CUDA
  backend. It links `libcudart.so.12`, `libcublas.so.12`,
  `libcublasLt.so.12` and `libcuda.so.1`. The editor never links them.
- **One code path.** `chukcut_whisper::run::transcribe` is the whisper.cpp
  call (word timestamps, the abort-callback workaround from 0013). The
  helper calls it with the GPU on; the engine calls the same function in its
  own process with the GPU off (feature `local-whisper`, as before).
- **The engine decides per transcription** (`modules/speech/local.rs`,
  `helper.rs`). When the helper is beside the binaries (same places as the
  ML worker: beside the executable, one up, `libexec/chukcut/`,
  `lib/chukcut/`, or `CHUKCUT_WHISPER_HELPER`) and an NVIDIA driver is
  loaded (`/sys/module/nvidia/version`), it starts the helper. The helper
  says hello with the device ggml found. Any failure falls back to the CPU in
  the editor's process and is logged: no binary, no driver, the loader cannot
  find the CUDA libraries, no GPU in the hello, a protocol mismatch, no hello
  within 20 s, an error reply (out of memory on the card), a crash. Cancel
  kills the helper. The `speech_*` commands are unchanged; the progress
  label names the device ("Transcribing on the GPU (NVIDIA GeForce RTX
  3060, CUDA)"), and `speech_device` / `speech_device_known` report it.
- **CUDA libraries from chukcut's bundle.** When the dynamic loader cannot
  find the CUDA libraries, the engine starts the helper again with the
  `lib/` directories of the NVIDIA library packs the ML settings installed
  (decision 0025, amended) on `LD_LIBRARY_PATH`. The CUDA 12 bundle has
  `libcudart.so.12` and `libcublas.so.12`, so a machine with the bundle and
  without the toolkit transcribes on the GPU too.
- **Protocol.** One transcription per process: hello line, one request line
  plus raw f32 samples on stdin, progress lines, one done or error line. JSON
  lines on stdout (`chukcut_whisper::protocol`, version 1). `--probe` says
  hello and exits; that is how the captions panel learns the device without
  loading a model.
- **Built by the normal build.** `crates/engine/build.rs`, with the
  `local-whisper` feature (the app and the CLI turn it on), on Linux, when
  `nvcc` is found (`CUDACXX`, `PATH`, `/usr/local/cuda/bin`), runs `cargo
  build --release -p chukcut-whisper --features cuda` into
  `<target>/whisper-cuda/` and copies the binary into the profile directory
  beside `chukcut`. So `cargo run -p chukcut` transcribes on the GPU on a
  machine with `nvcc`, with no flag. It shares the outer build's jobserver,
  skips under clippy, and a failed build is a warning, not a failed build
  (`CHUKCUT_WHISPER_CUDA=1` makes it an error, `=0` skips it). CI and the
  release builders have no `nvcc` and produce no helper; packaging ships it
  when it exists, as an optional file.

## Why

- **The build must start without CUDA.** whisper-rs-sys's `cuda` feature
  links cudart and cuBLAS into the binary. A CUDA editor would not start on a
  machine without the toolkit's libraries; that is why CUDA was opt-in before.
  In a separate process, a missing library costs one failed `exec`.
- **Crashes stay out of the editor**, as with the ML worker (0025): an
  out-of-memory on a 4 GB card or a driver fault ends the helper, and the
  CPU path still produces the captions.
- **Why not ggml's dynamic backends** (`GGML_BACKEND_DL`: `libggml-cuda.so`
  loaded at run time). It needs ggml built as shared libraries, and
  whisper-rs-sys 0.15 builds and links whisper.cpp statically
  (`BUILD_SHARED_LIBS=OFF`, `static=ggml`). A CUDA module loaded into a
  process whose ggml is static would bring its own copy of `ggml-base`, two
  registries and two allocators. Making it work means forking whisper-rs-sys,
  and the module would still run inside the editor.
- **Why not a mode of the ML worker.** The worker must start without CUDA to
  run ONNX models on the CPU, and it gets CUDA through `dlopen`. whisper.cpp
  CUDA links CUDA at start-up, the opposite.
- **Why not Vulkan.** whisper.cpp's Vulkan backend would open a Vulkan
  instance next to wgpu's in the editor; `CLAUDE.md` forbids that. In a
  helper it would be contained, but it would be a second GPU path to
  maintain while CUDA already runs here. CUDA it is.
- **Why a nested cargo in a build script.** A cargo feature cannot depend on
  whether `nvcc` is installed, and the owner wants the GPU without a flag.
  The nested build has its own target directory, so it never waits for the
  outer build's lock, and the outer jobserver keeps the total job count.

## Measured (RTX 3060, driver 610.57, CUDA toolkit 12.0, 2026-10-09)

The same synthetic speech (FFmpeg's `flite` voice), through
`examples/captions.rs`; the GPU column includes starting the helper and
loading the model, the CPU ran 8 threads on a machine at load 15–29.

| Audio | Model | CPU (in the editor) | CUDA (helper) |
|---|---|---|---|
| 35 s | Base | 11.8 s | 1.2–2.1 s |
| 35 s | Large v3 turbo | 49.1 s | 1.8–3.6 s |
| 5 min 16 s | Base | 44.9 s | 3.7–4.0 s |
| 5 min 16 s | Large v3 turbo | 516.6 s | 5.9–6.4 s |

The words are the same on both (115 and 1035), the times within 20 ms.
`docs/STATUS.md`, "Whisper on the GPU", has the details.

## What it costs

- **A first build of about 16 minutes** on a machine with `nvcc` (ggml's
  CUDA kernels, measured at a load of ~27; ggml compiles them for the
  build machine's GPU only, `sm_86` here). Every worktree pays it once,
  because its target directory is new. `CHUKCUT_WHISPER_CUDA=0` skips it.
- **122 MB** of helper binary beside the editor.
- **Start-up per transcription:** the helper starts CUDA (0.2–0.3 s
  usually, 2–5 s on a busy machine or a cold driver) and loads the model
  into GPU memory. It is inside the GPU numbers above.
- **A packaged helper is for the packager's GPU and CPU** unless the
  packager sets `GGML_NATIVE=OFF` and `CMAKE_CUDA_ARCHITECTURES`
  (`packaging/README.md`).
- **The progress bar restarts** when the helper fails mid-way and the CPU
  takes over.

## What would change our minds

- whisper-rs-sys supporting shared ggml with `GGML_BACKEND_DL`: one build
  with a CUDA module beside it, loaded only when present, and no nested
  cargo. It would still run in the editor's process, which is the weaker
  half of the trade.
- A Whisper model on ONNX Runtime with word timestamps as good as
  whisper.cpp's: it would move into the ML worker, which already has CUDA,
  TensorRT and the bundles, and this helper would go.
- Packages built on a builder with `nvcc`: then the release workflow builds
  the helper with portable settings and every package carries it.
