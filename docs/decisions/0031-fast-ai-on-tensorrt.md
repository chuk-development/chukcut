# 0031 — "Fast" AI runs measured models on TensorRT, built from the fp32 files; no fp16 files

Date: 2026-10-04. Status: accepted (agent/mlspeed).

## What was decided

**One switch, per-model plans.** Settings › AI acceleration has "Fast
(fp16/TensorRT)" (`Settings::ml_fast`, on by default; `chukcut-cli ml
acceleration fast|standard`). The ML worker starts with `--acceleration
fast|standard`. In Fast mode a model with a *fast plan*
(`chukcut_ml_worker::accel::fast_plan`) runs on ONNX Runtime's TensorRT
provider, at the precision its plan names, with the CUDA provider behind it
for any node TensorRT does not take. Every other model, and every model in
Standard mode, runs as before (CUDA, OpenVINO or the CPU, fp32). The plans
are measurements, not guesses (RTX 3060, `docs/STATUS.md`, "Faster AI on
NVIDIA"):

| Model | Plan | Speed-up | Against fp32 on CUDA |
|---|---|---|---|
| RIFE v4 | TensorRT fp32 (TF32) | 1.3–1.9x | 66–81 dB |
| Real-ESRGAN x4v3 | TensorRT fp16 | 2.3x | 57.7 dB, max 6 |
| LaMa | TensorRT fp16 | 2.1x | identical outside the hole; 38.6 dB inside it |
| BiRefNet lite | TensorRT fp32 | 2.65x | IoU 0.9998 |
| RVM, MobileSAM, VitTrack, YuNet, face mesh, HTDemucs | none | — | — |

RIFE is in fp32 on purpose: in fp16 it was 2.5x faster, but its warping
grid's coordinates do not fit fp16 to a pixel at 1080p (3 % of values off
by more than 8, 35.5 dB). RVM has no plan because TensorRT's parser refuses
its Resize (the scale is an input); it is 15 ms a frame on CUDA anyway.

**TensorRT is an add-on, pinned like the CUDA bundles.** `ml install
tensorrt` (or Install in the settings) fetches two packs for the CUDA major
of the installed bundle (`registry::TENSORRT`): ONNX Runtime's TensorRT
provider (`libonnxruntime_providers_tensorrt.so`, 0.9 MB, kept from the same
pinned GPU archive the bundle's ONNX Runtime came from — the bundle used to
drop it) and TensorRT 10.16.1.11 itself from NVIDIA's `tensorrt-cu13-libs` /
`tensorrt-cu12-libs` wheel on pypi.nvidia.com, pinned by SHA-256, of which
only the Linux libraries are kept (libnvinfer, its plugins, the ONNX parser,
the per-architecture builder resources). ORT 1.28.3's provider links
`libnvinfer.so.10`; TensorRT 11 would not load. The provider is symlinked
beside `libonnxruntime.so`, where ONNX Runtime opens providers from. The
worker preloads TensorRT only in Fast mode.

**Engines are cached per model, GPU, driver, TensorRT version, precision
and input shape**, under `~/.cache/chukcut/ml/tensorrt/<model>-<version>/
<gpu>-<driver>-trt<version>-<precision>/<shape>/`, with a `build.json`
saying how long the build took. A session is built for exactly one set of
input shapes (profile min = opt = max), so a clip's frames reuse one engine
and another size gets its own directory instead of a rebuild. A new driver
or GPU is a new directory: a stale engine is never loaded.

**Building is announced, not hidden.** Before a build the worker sends a
progress message with `hold_secs` (protocol 7); the engine extends the
request's deadline by it and `ml::worker::building()` exposes the stage and
its elapsed time, which the slow-motion and enhance progress lines show
("Preparing TensorRT for … (first time only …): 1:12 so far"). `ml status`
and the settings page list each model's provider, precision and engine
build times.

**Falling back is automatic.** A TensorRT session that fails to build, or a
run that fails on it, moves that model to CUDA for the rest of the worker's
life (`Runtime::demote`) and the job goes on; a missing add-on, a worker in
Standard mode or no working CUDA provider means TensorRT is simply not
offered (the probe says why).

**No fp16 model files.** Of the five heavy models only BiRefNet (MIT,
`model_fp16.onnx` in the same repository) and RVM (GPL, a release asset)
have public fp16 exports with a usable licence. RVM's was slower than fp32
on CUDA (16.6 against 14.7 ms) and less accurate (IoU 0.97); BiRefNet's was
9 % faster where TensorRT fp32 is 2.65x. Converting at install time
(onnxconverter-common's algorithm, in a throwaway venv) produced RIFE and
LaMa files ONNX Runtime refuses (Cast nodes typed wrong), hung for over ten
minutes on a second RIFE attempt, and gained 18 % on Real-ESRGAN, where
TensorRT's own fp16 builder gains 2.3x from the fp32 file the app already
has. So nothing is converted and nothing second is downloaded.

**HTDemucs is built without constant folding** (`accel::tuning`). Its
export computes shapes through ~700 ScatterND/Expand/Range chains on a
fixed input; ONNX Runtime's constant folding turned them into gigabytes of
constants while creating the session: the worker peaked at 6.8 GB on CUDA
and 8.5 GB on the CPU. Without folding: 1.5 GB and 2.5 GB, the same speed
(270 ms a segment on CUDA), output within 8e-6. Smaller segments were not
an option: the export's input length is fixed.

**Copies and conversions.** RIFE's two frames are converted to the
network's input once per request instead of once per phase, and handed to
ONNX Runtime as a view; RGBA↔planar conversions run on four threads with a
rounding that needs no libm call (`pixels.rs`), which took RIFE's 1080p
pre- and post-processing from ~50 to ~25 ms; Real-ESRGAN writes one tile's
4x picture on a second thread while the network makes the next.

## Why

- TensorRT fuses and picks kernels per GPU and builds fp16 from fp32
  weights; on the 3060 it was 1.3–2.7x where cuDNN's NHWC layout or the
  CUDA provider's other knobs gained nothing (NHWC was 5–15 % slower).
- Per-model plans keep "without hurting quality" checkable: each plan has a
  PSNR or IoU in STATUS, and `chukcut-cli ml bench --compare` measures it
  again on any machine.
- An add-on, not part of the bundle: TensorRT is 3.7–4.3 GB to download,
  NVIDIA-only and optional; the CUDA bundle stays 1.3–1.9 GB.

## What it costs

- **Disk and download**: 3.7 GB (CUDA 13) or 4.3 GB (CUDA 12) to download,
  ~2.6 GB on disk, plus the ORT GPU archive again (241 / 424 MB) for the
  0.9 MB provider. Engines: 12 MB (RIFE) to a few hundred MB per model and
  size.
- **First use**: an engine build per model and size: Real-ESRGAN 25–60 s,
  RIFE 1–3 min, LaMa 1–2 min, BiRefNet ~5.5 min with 5 GB of memory. Shown,
  cached, but real.
- **One engine per input size**: a project with clips of many sizes builds
  many engines (each cached).
- **Two sessions per model** in Fast mode (CUDA and TensorRT): more GPU
  memory, so a fallback costs nothing.

## What would change our minds

- An fp16 export with a usable licence for RIFE that keeps the warp in fp32,
  or a RIFE graph with normalised coordinates: fp16 would then be worth its
  2.5x.
- TensorRT for RTX (smaller, JIT, consumer GPUs) in ONNX Runtime's Linux
  builds: it would replace the 3.7 GB add-on.
- A dynamic-shape profile that costs no speed: one engine per model instead
  of one per size.
