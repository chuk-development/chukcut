# 0025 — Machine learning runs in a separate worker process on ONNX Runtime, loaded at run time

Date: 2026-10-04. Status: accepted (ML worker agent).

## What was decided

Every learned model runs in `chukcut-ml-worker` (`crates/ml-worker`), a
separate binary that the engine starts, talks to and restarts
(`modules/ml/worker.rs`). The editor process never loads ONNX Runtime.

- **Runtime.** `ort` 2.0.0-rc.13 with `load-dynamic`: the worker opens
  `libonnxruntime.so` with `dlopen` when the first request needs it. The
  library comes from a **runtime pack**, Microsoft's own release archive,
  pinned by URL and SHA-256 in `registry.rs` and unpacked while it downloads
  into `~/.cache/chukcut/ml/runtime/` (only the `.so` files are kept; the
  TensorRT provider is skipped). The CPU pack (9 MB) is fetched on first use;
  the CUDA 12 and CUDA 13 packs (424 / 241 MB) are installed on request
  (`chukcut-cli ml install runtime:cuda12`), and so is cuDNN 9 for CUDA 12
  (`runtime:cudnn9-cu12`, NVIDIA's own PyPI wheel, 766 MB, unpacked to
  1.2 GB), which is the library an NVIDIA machine most often lacks.
  `chukcut-cli ml status` names the GPU vendors present and what to install
  for them. `CHUKCUT_ORT_DYLIB` names any other build, e.g. Intel's
  OpenVINO-enabled one.
- **Providers.** Probed once per worker: CUDA, then OpenVINO, then the CPU. A
  provider counts only if registering it on a session builder succeeds,
  which is the step that opens its libraries. A model's session is built on
  the first provider that accepts it, so a missing cuDNN ends in a CPU
  session and a log line, never in a crash. CUDA and cuDNN libraries are
  preloaded (by file-name prefix, so CUDA 12's `.so.12` and CUDA 13's
  `.so.13` both work) from `CHUKCUT_CUDA_LIB_DIRS`, the cuDNN pack and the
  runtime pack's own `lib/`, so a user does not have to change
  `LD_LIBRARY_PATH`.
- **Protocol.** Length-prefixed frames on stdin/stdout: a JSON header (a
  request or a reply, tagged) and a raw payload (an RGBA8 frame in, a matte's
  alpha bytes out). Every request has an id; replies are zero or more
  `progress`, then one `done` or `error`. A reader thread in the worker takes
  `cancel` as soon as it arrives. `hello` reports `PROTOCOL_VERSION` (now 2)
  and the engine refuses a worker with another.
- **Supervision.** One worker per editor. Requests carry a deadline; a missed
  deadline kills the worker. A dead worker is restarted on the next request,
  at most three times a minute. Callers that keep state in the worker (a
  track's template, a matte's recurrent state) start it again on the new one.
- **Models** are listed in the registry with a pinned URL, SHA-256, size,
  licence and `commercial_ok`. They download on first use into
  `~/.cache/chukcut/ml/models/<id>/<version>/`. A model whose licence forbids
  commercial use is refused unless asked for by name. Today: YuNet (MIT,
  faces for auto reframe), VitTrack (Apache-2.0, tracker T2), RVM
  MobileNetV3 (GPL-3.0, Remove background).
- **Results.** Edit-defining results (tracks, reframe paths) go into the
  document through `EditCommand`s. Pixels (mattes) go into the cache, keyed
  by media content, model and version; the document records only the model
  and version. An export bakes missing mattes with the recorded version
  first, and fails in words when it cannot.
- **Never on the UI thread.** Every call into the worker blocks its caller,
  so every caller is a job thread (tracking, bake, analysis) or the export
  thread; the app only polls job status from its tick.

## Why

- **Crashes stay out of the editor.** A native crash, an out-of-memory on the
  GPU or a wedged driver in the runtime kills the worker and fails one job.
  The user's unsaved project is in the other process.
- **The GPU rule.** The editor must not open a second Vulkan device
  (`CLAUDE.md`). ONNX Runtime's CUDA and OpenVINO providers do not use
  Vulkan, but a later WebGPU or whisper.cpp-Vulkan backend would, and in
  another process that is contained.
- **No gigabyte of CUDA in the editor.** The editor binary and its
  dependencies stay as they were; a machine without a runtime pack simply has
  no ML, and every feature that uses ML has a model-free fallback (auto
  reframe to saliency, tracking to KLT) or says what is missing.
- **ONNX Runtime over candle or burn:** one format for every model, and
  providers for both first-class vendors (CUDA for NVIDIA, OpenVINO for
  Intel). `docs/research/ml-features.md` §5.2 has the comparison.
- **Load at run time, not link.** Linking ONNX Runtime would make the editor
  depend on a 9 MB-to-1 GB shared library at start-up, and the CUDA build
  would have to be the one linked. Loading lets the same worker binary run
  the CPU pack, a CUDA pack or a user's OpenVINO build.
- **Frames over a pipe** rather than shared memory: an analysis job sends a
  640 px frame (1.6 MB) per request, which a pipe moves in well under a
  millisecond; the model takes 2–100 ms. Shared memory pays off only for
  real-time inference in the preview, which nothing does yet.

## Measured (RTX 3060, `chukcut-cli ml bench`, 2026-10-04)

| Model | Frame | CPU (4 threads, loaded machine) | CUDA 13 |
|---|---|---|---|
| RVM MobileNetV3 | 540×960 | 99 ms | 15.6 ms |
| RVM MobileNetV3 | 1080×1920 | — | 36.5 ms |
| VitTrack | 640×360 | 4.6 ms | 2.8 ms |
| YuNet | 640×360 | — | 2.3 ms |

A 3 s 720×1280 talking head bakes its 90 mattes in 16.9 s on the CPU (under
a load of ~45 from other builds) and 2.6 s on CUDA. CUDA's mattes differ
from the CPU's by 1.3/255 on average.

## What it costs

- **Every frame is decoded in the engine and copied into the worker.** For
  analysis at 640–960 px this is noise next to the model.
- **State lives in the worker.** A worker restart mid-job costs a track one
  frame of lower accuracy and a matte a few frames of settling.
- **The packs are big and the CUDA pack is not self-sufficient.** It needs
  the CUDA 12/13 runtime from the system (or `CHUKCUT_CUDA_LIB_DIRS`), and
  a new enough one: Ubuntu's CUDA 12.0 `libcudart.so.12` lacks
  `cudaLibraryGetKernel`, which ORT 1.28's CUDA 12 provider needs, so on
  the owner's machine only the CUDA 13 pack with CUDA 13 libraries from a
  Python venv lit the GPU. Without them the probe reports why and the worker
  runs on the CPU. Shipping the CUDA runtime itself as a pack (NVIDIA's
  `nvidia-cuda-runtime` wheels) is the obvious next step.
- **OpenVINO is not in Microsoft's Linux builds.** Intel GPUs get OpenVINO
  only through `CHUKCUT_ORT_DYLIB` today.
- **One request at a time.** A bake and a track share one worker and take
  turns per frame. Fine for one user; a second worker per GPU is the next
  step if two background jobs become common.

## What would change our minds

- Real-time ML in the preview (live background removal while scrubbing):
  frames would have to cross by memfd or DMA-BUF, or the model would move
  into the engine on the shared wgpu device (burn's wgpu backend, if its
  ONNX import covers the model).
- ONNX Runtime's WebGPU provider becoming stable: one provider for every
  vendor, and the runtime packs would shrink to one.
- A model that `ort` cannot run but candle can (a transformer with custom
  ops): it would get a second worker binary on the same protocol, not a
  path inside the editor.
