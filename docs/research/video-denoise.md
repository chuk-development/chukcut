# Video denoise: what is built, and an ML denoiser (2026-10-05)

## Built

Reduce noise has two modes (`fx::temporal`, decision 0026 amendment):

- **Spatial**: a 5×5 bilateral pass in √-linear units, on the GPU.
- **Temporal**: the frames before and after are drawn like the clip and
  averaged in where their 3×3 means are close to this frame's
  (motion-adaptive), then a gentler spatial pass. Preview and export run
  the same passes.

Measured on generated footage (testsrc2 with ffmpeg temporal noise
`alls=28`, 320×240, strength 70, keep detail 40;
`tests/temporal_denoise.rs`): the fine detail left (mean distance of a pixel
from its 3×3 mean, green, 0–255) is 11.1 without the effect, 7.1 with the
spatial pass and 5.0 in Temporal mode. The same on the RTX 3060 and on
lavapipe. An export of the temporal project differs from its preview by
1.51 per 8×8 block mean; an export without the effect differs from its own
preview by 1.46, so the temporal pass adds no measurable difference.

Not built: motion compensation. A moving area gets the spatial pass only.
The optical flow of decision 0028 (RIFE) could warp the neighbours first;
that is a bake, not a live pass.

## An ML denoiser through the worker: not built, not cheap

Candidates with GPL-compatible licences (checked in the repositories'
LICENSE files on 2026-10-05):

| Model | Kind | Licence | ONNX |
|---|---|---|---|
| FastDVDnet (m-tassano/fastdvdnet) | video, 5 frames, needs a noise-level map | MIT | none published; export from the `.pth` with PyTorch |
| NAFNet (megvii-research/NAFNet), SIDD weights | single image | MIT, with Apache-2.0 BasicSR parts | none published by the authors |

Why it is not cheap:

- No official ONNX file. We would export one ourselves with PyTorch,
  check it against the reference, and host it (the worker downloads models
  by URL, `ml-worker/src/registry.rs`), with its licence beside it.
- FastDVDnet reads two frames before and two after. The provider's cache
  holds two frames, so it cannot run live; it would be a bake with a frame
  cache, like Enhance quality (`modules::enhance`), with the bake queue,
  cache limits and the prepare chip.
- It needs a noise-level estimate per clip (a sigma map), which is one more
  analysis step.
- NAFNet is image-only: it would not use the frames over time, which is
  what the Temporal mode already does on the GPU without a model.

The work is about the size of the Enhance quality bake. Do it only when
users ask for stronger denoise than the Temporal mode gives on real
low-light footage.
