# Settings

Open the settings with **Menu › Settings…**, with `Ctrl+,`, or with
**Settings** on the start screen. The settings are one page of sections.
Each change is saved at once. There is no OK button. The settings belong to
this computer, not to a project, and they are not part of undo.

## New projects

- **Canvas**: the shape that the start screen selects first, for example
  **9:16 · 1080×1920**.
- **Frame rate**: **24 fps** to **60 fps**.

## Preview and playback

- **Preview resolution**: **Full**, **Half** or **Quarter**, relative to the
  size of the player. A lower value plays heavy timelines more smoothly. The
  export always uses the full resolution.
- **Largest preview edge**: **Automatic**, or a limit from **2160 px** to
  **540 px**. **Automatic** renders at the size of the player, and never
  above the canvas.
- **Audio scrubbing**: plays a short sound while you drag or step the
  playhead.

## Proxies and cache

- **Proxy media**: **Automatic**, **Always** or **Off**. A proxy is a small
  copy of a heavy file. The preview plays the proxy. The export always uses
  the original. **Automatic** makes proxies only for footage that this
  computer cannot decode in time, for example 4K HEVC.
- **Proxies on disk**: the size of the proxies, and **Clear**.
- **Cache**: the size of the cache, and **Clear**. The cache holds
  thumbnails, waveforms, proxies, preview frames, background mattes and
  remade frames. chukcut makes all of it again when it needs it. **Clear**
  does not delete downloaded AI models or caption models.
- **Cache limit**: **2 GB**, **4 GB**, **8 GB** (the default), **16 GB**,
  **32 GB** or **No limit**. Above the limit, chukcut deletes the files that
  were not used for the longest time. The files of the open project stay.

## AI acceleration

This section shows where the AI models run, and lets you install the GPU
libraries. See also [AI tools](ai-tools.md).

- **Models run on**: for example "CUDA 13 on the GPU with chukcut's CUDA
  libraries", or "Nothing yet; ONNX Runtime downloads on first use". A line
  below it gives advice, for example which bundle to install for your
  NVIDIA GPU.
- **Graphics**: your GPUs, and the NVIDIA driver version.
- **NVIDIA GPU (CUDA 13)** and **NVIDIA GPU (CUDA 12)**: the GPU bundles.
  These rows show only when chukcut finds an NVIDIA GPU, or when a bundle is
  installed. Each bundle contains ONNX Runtime with CUDA, the CUDA runtime,
  cuBLAS, cuRAND, NVRTC and cuDNN 9 from NVIDIA. CUDA 13 needs driver 580 or
  newer (about 1.3 GB). CUDA 12 needs driver 525 or newer (about 1.9 GB). The
  row says which one is right for this computer.
  - **Install (size)** downloads the bundle. A progress bar and **Cancel**
    show while it installs.
  - **Remove** deletes an installed bundle.
  - **Finish** completes a bundle that is only partly installed.
- One row for each model, with its licence and its size. A model that runs
  only on a GPU says **needs a GPU**. **Remove** deletes a downloaded model.
  A model that is not there says **Downloads on first use**.
- **Baked mattes**: the cut-outs of Remove background and Select object, one
  picture per frame. **Clear** deletes them. Clips that use them make them
  again.
- **Remade frames**: the frames of Remove object and Enhance quality.
  **Clear** deletes them.

To choose the runtime by hand, use **Settings › Performance › AI
runtime**.

## Performance

- **Video decoding**: **Automatic**, **VAAPI (Intel, AMD)**, **NVDEC
  (NVIDIA)** or **Software**. **Automatic** uses VAAPI when the GPU takes its
  frames without a copy, then NVDEC, then the CPU. A file that the chosen
  decoder cannot read plays in software. The change applies to the clips
  that open after it; restart chukcut to apply it to all of them. When
  `CHUKCUT_DECODE` is set, the row says so, and the variable wins.
- **AI runtime**: **Automatic**, or one ONNX Runtime pack: **ONNX Runtime
  (CPU)**, **ONNX Runtime for NVIDIA (CUDA 12)** or **(CUDA 13)**. A pack
  that is not installed shows "(not installed)", and chukcut then uses
  **Automatic**. **Automatic** uses the GPU bundle that your driver can run,
  else the CPU. A change stops the AI worker; the next AI tool starts it
  again with the new runtime. When `CHUKCUT_ML_RUNTIME` is set, the row says
  so, and the variable wins.

## Keyboard shortcuts

The **Shortcuts** row shows the preset, the number of your changes and the
number of conflicts. **Edit shortcuts…** opens the shortcut editor. See
[Keyboard shortcuts](shortcuts.md).

## Accounts

Accounts are optional. They connect chukcut to online services with your own
key.

- **Add account** offers **OpenAI-compatible**, **ElevenLabs**, **fal.ai**,
  **Pexels**, **Pixabay**, **Freesound** and **DeepL**.
- The form has **Get a key** (opens the provider's key page), **Name**, and
  for an OpenAI-compatible server **Base URL (up to /v1)** and **Default
  model**, then the API key. A local server can work without a key.
- **Save and test** saves the account and makes a test request. Each account
  row has **Test**, **Edit** and **Delete**.
- The keys stay on this computer, in `~/.config/chukcut/secrets.toml`. Only
  your user can read that file.
- An environment variable `CHUKCUT_KEY_<ACCOUNT_ID>` overrides the key in the
  file.

What each account is for:

| Account | Used by |
|---|---|
| OpenAI-compatible | auto captions on a server, caption translation, text to speech |
| ElevenLabs | text to speech, sound effects, music |
| fal.ai | background removal and upscaling in the cloud, with the price before the job |
| Pexels, Pixabay | stock video and photos |
| Freesound | stock sounds |
| DeepL | caption translation |

## Hardware

A report. The switches are in **Performance**:

- **Graphics card** and **Render backend**.
- **Zero-copy decode**: **Yes** when decoded frames go to the GPU without a
  copy (VAAPI).
- **Hardware decoding** and **Hardware encoding**: each codec with its
  backend, and **works**, the reason it was refused, or **not in this
  build**.

To choose a decode path, use **Performance › Video decoding**.
`CHUKCUT_DECODE` overrides it (see [Troubleshooting](troubleshooting.md)).

## Logs

- **Log folder**: the path of the logs.
- **Open folder** opens it. **Show today's log** opens today's file.
