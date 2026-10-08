# Troubleshooting

## The app does not start

- **The app stops at start, or the window stays black**: chukcut needs a
  Vulkan driver.
  Run `vulkaninfo --summary`. It must list your GPU. On a computer without a
  GPU, install Mesa's lavapipe (`mesa-vulkan-drivers`). It works, but it is
  slow.
- **A missing library** (`libavcodec.so.60: cannot open shared object
  file`): the binary was built on a system with another FFmpeg version.
  Build chukcut from source on this system (`scripts/install.sh`).

## Playback is slow

1. Open Settings › **Hardware**. Look at **Hardware decoding**. Your codec
   must show **works**.
2. Set Settings › **Preview resolution** to **Half** or **Quarter**.
3. Set Settings › **Proxy media** to **Automatic** or **Always**. chukcut
   then makes small copies of heavy files for the preview.
4. Use a release build. A debug build is much slower.

## Hardware decode

chukcut tries the decoders in this order: VAAPI (Intel, AMD), then NVDEC
(NVIDIA), then software. It tests each codec with a real frame before it
uses it.

- **Intel**: install the iHD driver (`intel-media-va-driver-non-free` on
  Debian and Ubuntu). `vainfo` must list your codecs.
- **AMD**: install `mesa-va-drivers`.
- **NVIDIA**: the proprietary driver gives NVDEC and NVENC. FFmpeg loads the
  NVIDIA libraries from the driver. chukcut does not bundle them.
- **AV1**: AV1 decodes on the GPU only where the GPU supports it.

To choose a path, use **Settings › Performance › Video decoding**. There, a
file that the chosen decoder cannot read still plays in software. To force a
path for a test, start chukcut with `CHUKCUT_DECODE`. It overrides the
setting, and a file the forced decoder cannot read does not play:

```bash
CHUKCUT_DECODE=software chukcut   # never use the GPU decoder
CHUKCUT_DECODE=vaapi chukcut      # VAAPI only
CHUKCUT_DECODE=cuda chukcut       # NVDEC only
CHUKCUT_DECODE=auto chukcut       # the default
```

Other switches for diagnosis:

- `CHUKCUT_PREVIEW_READBACK=1` copies each preview frame through the CPU,
  instead of the shared GPU memory. Try it when the player stays black or
  shows wrong colours.
- `CHUKCUT_PREVIEW_JPEG=software` and `CHUKCUT_PROXY_ENCODER=software` keep
  the preview encoder and the proxy encoder off the GPU.

## Colours look different in another player

chukcut writes the colour space into every export (Rec. 709 for 720p and
larger, Rec. 601 for 480p). Exports made before October 2026 had no colour
tag and a different matrix: in other players their reds and greens were a
little off. Export them again.

If a file still looks different, check the player: some players ignore the
tags of full-range files. Use limited range (the default).

## HDR clips look wrong

chukcut converts HDR clips (HLG and PQ, for example from an iPhone) to SDR.
The player and the export show the same result.

- **Highlights look flat.** chukcut assumes that the clip was mastered for
  1000 nits. Brighter highlights are clipped.
- **The thumbnail strip** uses the same conversion.
- **A clip looks grey and flat.** The file probably has no HDR tag. Check it
  with `ffprobe -show_streams FILE`: `color_transfer` must be `smpte2084`
  (PQ) or `arib-std-b67` (HLG).

## The export does not offer a GPU encoder

The export dialog offers a hardware encoder only after a test encode works.
Settings › **Hardware** › **Hardware encoding** shows the reason for each
refused encoder. A codec in FFmpeg does not mean that the driver supports
it. The software encoders (x264, x265) always work.

## AI tools

- **An AI tool says that it cannot run**, or nothing happens: the worker
  program `chukcut-ml-worker` must be next to `chukcut`, in
  `<prefix>/libexec/chukcut/` or `<prefix>/lib/chukcut/`, or on your `PATH`.
  A `chukcut` on your `PATH` that is a symbolic link finds the worker next
  to the real file.
  The install script and the tarball put it next to `chukcut` in
  `~/.local/bin`. A build from source with only `-p chukcut` does not have
  it. See [Getting started](getting-started.md#install).
  `CHUKCUT_ML_WORKER=/path` names the worker. `CHUKCUT_ML_WORKER=off` turns
  the AI tools off.
- **The AI runs on the CPU on an NVIDIA computer**: open Settings › **AI
  acceleration**. Install the bundle that the row marks as "the one for this
  machine". You do not need a CUDA installation. The bundle brings its own
  libraries. Your system's CUDA does not interfere.
- **A bundle row says "this machine's driver is too old for it"**: CUDA 13
  needs NVIDIA driver 580 or newer. CUDA 12 needs driver 525 or newer.
  Update the driver, or install the other bundle.
- **A bundle is partly installed**: the row shows **Finish** and **Remove**.
  Click **Finish**.
- **The first slow-motion, enhance or remove-object job waits for minutes**
  with "Preparing TensorRT for …": Fast mode prepares each model once for
  each range of frame sizes. The next jobs in that range start at once. To skip it,
  switch off **Fast (fp16/TensorRT)** in Settings › **AI acceleration**.
- **Intel GPUs**: the models run on the CPU. An OpenVINO build of ONNX
  Runtime can run them on the GPU: set `CHUKCUT_ORT_DYLIB` to its
  `libonnxruntime.so`. This path is not tested.
- **Folders for CUDA libraries**: `CHUKCUT_CUDA_LIB_DIRS` (folders separated
  by `:`) is searched first.
- **The command line shows more**: `chukcut-cli ml status --probe` says what
  the models run on, and why.

## Cache size

The cache can grow fast. Background mattes, slow-motion frames and remade
frames are one picture per frame. Remade 4K frames can be 1 to 3 MB each.

- Settings › **Cache limit** sets the limit. The default is 8 GB.
- Settings › **Cache** › **Clear** deletes the cache. Settings › **AI
  acceleration** has **Clear** for the baked mattes and for the remade
  frames.
- The files of the open project are kept when chukcut trims the cache.
- Downloaded AI models and caption models are not in the cache limit, and
  **Clear** does not delete them. Remove them in Settings › **AI
  acceleration**.

## Where files live

chukcut follows the XDG folders. If you set `XDG_CONFIG_HOME`,
`XDG_DATA_HOME`, `XDG_CACHE_HOME` or `XDG_STATE_HOME`, the folders move with
them.

| Folder | Contents |
|---|---|
| `~/.config/chukcut/` | `settings.json`, `shortcuts.json`, `recent.json`, `accounts.toml`, `secrets.toml` (keys, mode 0600), `export-presets.json`, `export-memory.json`, `captions.json`, `library.json`, `recent-folders.json`, the autosave working copy `autosave.chukcut` |
| `~/.local/share/chukcut/` | `luts/` (the LUT library), `grade-presets/`, `fonts/`, `templates/` (your templates), `template-media/`, `freeze-frames/`, `generated/` (cloud results), `recordings/` |
| `~/.cache/chukcut/` | `thumbnails/`, `waveforms/`, `proxies/`, `mattes/`, `flow/` (slow-motion frames), `enhance/` (remade frames), `landmarks/`, `voice/`, `audiofx/`, `compound-mix/`, `analysis/`, `library/` (downloaded stickers, music, stock) |
| `~/.cache/chukcut/ml/` | AI models and the GPU bundles. Not in the cache limit |
| `~/.cache/chukcut/whisper/` | caption models. Not in the cache limit |
| `~/.local/state/chukcut/logs/` | the log: `chukcut-YYYY-MM-DD.log`, at most 20 MB a file and 100 MB in total (see [The log file](#the-log-file)) |

A voiceover recording goes into a folder "<project> Media" next to the
project file.

## The log file

chukcut always writes a log. You do not have to turn it on.

- **Where**: `~/.local/state/chukcut/logs/` (or `$XDG_STATE_HOME/chukcut/logs/`).
  Settings › **Logs** shows the folder and opens today's file.
- **Files**: one file a day, `chukcut-2026-10-09.log`. When a file reaches
  20 MB, the log continues in `chukcut-2026-10-09.2.log`, then `.3.log`.
- **Size**: the folder never holds more than 100 MB or 30 files. chukcut
  deletes the oldest files first. A normal day of work writes some hundred
  kilobytes.
- **Level**: the file gets chukcut's INFO lines and the WARN and ERROR lines
  of all libraries. `RUST_LOG` changes only the terminal output, not the file.

Each line starts with the time in UTC, the level and the part of chukcut that
wrote it. To find something, search for these words:

| Search for | You find |
|---|---|
| `startup:` | the hardware and the setup of this run (see below) |
| ` WARN ` and ` ERROR ` | problems, fallbacks and retries |
| `over budget` | work that took too long |
| `preview playback` | how smooth playback was |
| `export progress`, `export throughput` | how fast an export ran |
| `resources` | memory, CPU and GPU load every 30 seconds |
| `[ml-worker` | the AI worker's own messages |
| `ffmpeg:` | FFmpeg's error messages |
| `clip decode route` | how each clip decodes: GPU or CPU |

### The startup block

Each start writes one block of `startup:` lines. They tell which hardware
chukcut found and which parts of it it uses:

```text
startup: app version=0.1.0 commit=18a37080fe build=release pid=41180
startup: os name="Linux Mint 22.1" kernel=6.8.0-139-generic arch=x86_64 display=x11 desktop=ubuntu:GNOME
startup: cpu model="AMD Ryzen 7 5700X3D 8-Core Processor" cores=8 threads=16 usable_threads=11 ram_mb=32018
startup: ffmpeg avcodec=60.31.102 avformat=60.16.100 avutil=58.29.100 swscale=7.5.100 swresample=4.12.100
startup: nvidia driver=610.57.04
startup: settings decode=Auto proxies=Auto preview_scale=1 preview_max_edge=0 ml_fast=true
startup: transcription engine=whisper.cpp in-process gpu=none
startup: gpu adapter="NVIDIA GeForce RTX 3060" backend=Vulkan type=DiscreteGpu vendor=0x10de driver=NVIDIA driver_info=610.57.04 dmabuf=true
startup: decode vaapi="" nvdec="h264 hevc vp9 av1" vaapi_device=none refused="h264/vaapi hevc/vaapi vp9/vaapi av1/vaapi: hardware decode is unavailable: no VAAPI device on this machine"
startup: display scale=1 window=1600x960
startup: encode hardware="h264_nvenc hevc_nvenc" refused="h264_vaapi hevc_vaapi av1_vaapi: this machine has no usable VAAPI device; av1_nvenc: cannot open the av1_nvenc encoder with bitrate: Generic error in an external library"
startup: hardware probes took ms=4421
```

- `gpu` is the graphics card that renders the preview and the export.
  `type=Cpu` means a software renderer: everything is slow.
- `decode` lists the codecs that decode on the GPU, per path (VAAPI for
  Intel and AMD, NVDEC for NVIDIA). `refused` says why a path does not work.
- `encode` lists the GPU encoders that passed a test encode.
- The AI worker writes its own line when it loads its runtime:
  `[ml-worker 41207] chukcut-ml-worker: startup: ml runtime=1.22.0 acceleration=standard providers=CUDA,CPU unavailable="…"`.

### The timing lines

Work that takes longer than its budget writes one WARN line:

```text
WARN … over budget what="decoder seek" ms=359.5 budget_ms=150.0 detail=h264_1080.mp4 to 2.188 s from a cold decoder (1 seek, Software)
```

- `what` is the kind of work, `ms` is the time it took, `budget_ms` is the
  time it may take. `detail` tells which file, frame or command it was.
- The budgets: `ui thread` and `ui tick` 50 ms (the window does not react
  for that time), `document edit` 33 ms, `decoder open` 200 ms,
  `decoder seek` 150 ms, `command` 2 s, `export frame` 1 s.
- One kind of work writes at most one line in 10 seconds. The next line then
  has `suppressed=N worst_suppressed_ms=…`: N more were too slow, and the
  slowest took that long. A line `over budget, held back by the rate limit`
  gives the count when no next line came.

During playback, the player writes one line every 10 seconds and one line
when playback stops:

```text
WARN … preview playback window seconds=10.0 fps=30.0 size=484x272 sharing="shared" shown=295 stalls=1 gap_p95_ms=43.8 gap_max_ms=189.0 rendered=303 render_mean_ms=5.0 render_p95_ms=8.5 render_max_ms=165.0 over_budget=2 budget_ms=33.3 skipped=7 late=0 discarded=4
```

- `shown` is the frames on the screen. `gap_p95_ms` and `gap_max_ms` are the
  times between two frames. At 30 fps a smooth gap is about 33 ms.
- `stalls` is the number of times one picture stayed for two frames or more.
  This is the stutter that you see.
- `render_*` is the time to make one frame. If `render_mean_ms` is more than
  `budget_ms`, the computer cannot play this project at full speed.
- `skipped` frames were not made, because the player jumped ahead to stay in
  sync with the sound.
- The line is a WARN when there was a stall, a skipped frame or a late frame.

An export writes `export progress` every 10 seconds and `export throughput`
at the end. `speed=1.00x` means one second of video in one second.

## Report a problem

Settings › **Logs** › **Show today's log** opens the log. Add it to your
report. For the AI tools, add the output of `chukcut-cli ml status --probe`.
