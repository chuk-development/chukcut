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
| `~/.local/state/chukcut/logs/` | `chukcut-YYYY-MM-DD.log`, the newest 7 days |

A voiceover recording goes into a folder "<project> Media" next to the
project file.

## Report a problem

Settings › **Logs** › **Show today's log** opens the log. Add it to your
report. For the AI tools, add the output of `chukcut-cli ml status --probe`.
