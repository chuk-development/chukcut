# AI tools

chukcut's AI tools run on your computer. No frame and no sound goes to a
server. (The optional cloud accounts are a separate thing; see
[Settings](settings.md#accounts).)

## How the AI tools work

- **A separate program runs the models.** It is `chukcut-ml-worker`. The
  editor starts it when a tool needs it. If a model fails or uses too much
  memory, the editor keeps running. The worker must be next to `chukcut` or
  on your `PATH`. See [Getting started](getting-started.md#install).
- **Models download once,** on first use, into `~/.cache/chukcut/ml/`. Each
  download is checked against its SHA-256. Settings › **AI acceleration**
  lists each model with its size and licence, and has **Remove**.
- **GPU or CPU.** On an NVIDIA GPU, install the CUDA bundle in Settings ›
  **AI acceleration** (see [Settings](settings.md#ai-acceleration)). Without
  it, the models run on the CPU. The CPU is 10 to 50 times slower. A tool
  that takes long on the CPU tells you how long before it starts and while
  it runs.
- **Fast mode (TensorRT).** On an NVIDIA GPU, the **TensorRT** add-on in
  Settings › **AI acceleration** makes slow motion, Enhance quality, Remove
  object and the Objects background 1.5 to 2.6 times faster. **Fast
  (fp16/TensorRT)** is on by default and has an effect only when the
  add-on is installed. The first job of a model at a new range of frame
  sizes prepares the model once: 30 s to 6 min. The progress line says
  "Preparing TensorRT for … (first time only …)". The next jobs in that
  range start at once. Slow motion has four ranges (up to 1280×720 and up
  to 1920×1080, each wide and tall); Enhance quality has one.
- **Results are a cache, settings are the project.** The project file keeps
  what you chose (the model, your clicks, the scale). The pictures that the
  models make (mattes, new frames) go into `~/.cache/chukcut/`. If the cache
  is cleared, chukcut makes them again.
- **Baking.** A tool works through the clip frame by frame in the
  background. A strip under the inspector shows the progress, with **Stop**.
  You can keep on editing. The preview uses each frame as soon as it is
  there.
- **Missing frames bake on their own.** After a trim, a speed change or an
  undo, chukcut bakes the frames that are missing. When you open a project,
  the title bar chip **Preparing…** does the same.
- **The export waits.** An export bakes all missing frames first. If it
  cannot, it stops and says why.

All costs on this page were measured on an RTX 3060 with the CUDA 13 bundle,
and on the CPU of the same computer. Your numbers will be different.

## Overview

| Tool | Where | Model | Clips |
|---|---|---|---|
| [Remove background: People](#remove-background) | Video › Remove background | Robust Video Matting | video |
| [Remove background: Objects](#remove-background) | Video › Remove background | BiRefNet lite | video |
| [Select object](#select-object) | Video › Remove background › Keep: Select | MobileSAM + VitTrack | video |
| [Grade or effects on subject or background](#grade-or-effects-on-the-subject-or-the-background) | Adjust › Mask, Effects tab | the clip's matte | video |
| [Remove object](#remove-object) | Video › Enhance | LaMa (+ MobileSAM, VitTrack) | video |
| [Enhance quality](#enhance-quality) | Video › Enhance | Real-ESRGAN x4v3 | video |
| [Optical flow, Smooth slow-mo](#optical-flow-slow-motion) | Speed › Standard | RIFE v4 | video |
| [Retouch](#retouch) | Video › Retouch | YuNet + MediaPipe face mesh | video |
| [Follow face](#follow-face) | Tracking tab | YuNet + MediaPipe face mesh | title, sticker, photo |
| [Follow body part](#follow-body-part) | Tracking tab | YOLOX + RTMPose | title, sticker, photo |
| [Fast motion (AI) tracker](#ai-tracker) | Tracking tab | VitTrack | title, sticker, photo |
| [Isolate voice](#isolate-voice) | Audio › Isolate voice | HTDemucs | clips with sound |
| [Auto captions](#auto-captions) | asset panel › Captions | Whisper | the timeline |

## Remove background

**Where:** select a video clip, open **Video › Remove background**, and tick
**Auto remove**. **Keep** chooses what stays:

- **People**: people stay, the rest is cut out. Model: Robust Video Matting
  (GPL-3.0, 15 MB). It uses the frames before, so the edges are steady.
- **Objects**: the main object stays: a product, a pet, a car. Model:
  BiRefNet lite (MIT, 224 MB). **GPU only**: it needs an NVIDIA GPU with
  the CUDA bundle. It looks at one frame at a time, so the edges can
  shimmer.
- **Select**: you click the object. See [Select object](#select-object).

Other controls:

- **Cut out instead**: removes the subject and keeps the rest.
- **Show matte**: shows the matte in black and white.
- "N of M frames have a matte", with **Finish missing frames**.

A frame without a matte yet is shown whole.

**Cost:**

| Model | GPU, per frame | CPU |
|---|---|---|
| Robust Video Matting | 13 ms (540×960); 240 frames in 6.4 s | works, slower |
| BiRefNet lite | 416 ms (960×540), 157 ms in Fast mode; 240 frames in 107 s | refused (12 to 25 s and up to 11 GB per frame) |

**Cache:** one greyscale picture per frame in `~/.cache/chukcut/mattes/`.
It counts towards the cache limit. Settings › **AI acceleration** › **Baked
mattes** › **Clear** deletes the mattes.

The same tab has **Chroma key** for green screens. It uses no model. See
[Effects and transitions](effects-and-transitions.md#chroma-key).

## Select object

**Where:** **Video › Remove background › Auto remove**, **Keep: Select**.

1. Click **Select on player**. The button changes to **Done**.
2. Click the object on the player. A dot marks each click.
3. Alt-click or right-click a part to leave it out.
4. Click **Done**.

chukcut finds the object in the clicked frame (MobileSAM), follows it
forwards and backwards through the clip (VitTrack), and makes a matte for
every frame. **Clear clicks** removes your clicks. Your clicks are saved in
the project.

**Models:** MobileSAM (Apache-2.0, 45 MB) and VitTrack (Apache-2.0, 0.7 MB).

**Cost:** about 70 ms per frame on the GPU (120 frames of 1080×1920 in
13 s). About 0.7 s per frame on the CPU, so a 10 s clip takes minutes.

**Limits:** while the object is fully hidden, or out of the frame, the
selection is lost and that part of the matte is empty. A click on an object
that touches an area of the same colour can select both. The click works on
the frame under the playhead.

**Cache:** as Remove background.

## Grade or effects on the subject or the background

**Where:** **Adjust › Mask › Apply to** for the grade, and **Apply to** at
the top of the **Effects** tab for the effects. Choose **Whole clip**,
**Subject** or **Background**.

This uses the clip's matte (People, Objects or a selected object) to limit
the grade or the effects. The clip is not cut. If the clip has no matte,
**Subject** or **Background** makes a people matte. Examples: blur only the
background, or keep the colour only on one object.

To keep the matte without the cut, set **Apply to** first, then clear the
**Auto remove** checkbox. The matte stays because the grade or the effects
use it.

The limit also holds inside a transition, while an In or Out animation
with a blur runs, and on a clip with frame blending or motion blur: there
each blended frame uses its own matte.

One clip has one matte. The grade and the effects use the same one.

## Remove object

**Where:** **Video › Enhance**, tick **Remove object**. Choose how to mark
the object:

- **Select on player**: click the object. Alt-click or right-click a part to
  keep. chukcut follows the object over the whole clip (MobileSAM and
  VitTrack, as in Select object).
- **Paint on player**: paint over what to remove, for example a logo or a
  sign. **Brush**: **S**, **M** or **L**. A stroke covers the same place in
  every frame.

The line below says what is removed and gives an estimate in the form
"N frames: about … on a GPU, … on the CPU".

How chukcut fills the hole, cheapest first:

1. The background that earlier frames showed. This works in a still shot
   and also when the camera pans or tilts: chukcut follows the camera, so
   the background that slides past a logo is used.
2. For a selected object, the background seen anywhere in the still part of
   the clip.
3. LaMa paints the rest. While the shot is still or the camera pans, the
   paint is mixed with the previous frame's, so it stays steady.

**Model:** LaMa (Apache-2.0, 208 MB).

**Cost:** LaMa takes 160 ms per frame on the GPU (77 ms in Fast mode) and
2 s on the CPU. A 3 s 1080p clip with a static logo took 27 s on the GPU. A
3 s 720p clip with a moving object on a still background took 19 s.

**Limits:** a painted area never shows what is behind it in a still shot,
so LaMa invents the fill there; the fill is steady. On a pan the background
slides out from behind a static logo, so most of the hole gets the real
background after a few frames (on a 3 px a frame pan, the shimmer in the
hole went down by 85 %). A zoom, a rotation or a shaky camera is not
followed: there the fill is made frame by frame and can shimmer.

**Cache:** "remade frames", JPEG files in `~/.cache/chukcut/enhance/`. They
count towards the cache limit. Settings › **AI acceleration** › **Remade
frames** › **Clear** deletes them.

## Enhance quality

**Where:** **Video › Enhance › Enhance quality**: **Off**, **2x** or **4x**.

The clip is made larger and cleaner: less blur, less noise, fewer
compression blocks. The line below says the new size, for example "Made at
1280×720", and gives an estimate.

**Model:** Real-ESRGAN general x4v3 (BSD-3-Clause, 5 MB).

**Limits:** the result is at most 3840 px on its long side. A source larger
than 1920 px is refused.

**Cost per frame:**

| Source | GPU | GPU, Fast mode | CPU (4 threads) |
|---|---|---|---|
| 640×360 | 0.10 s | 0.04 s | 2 s |
| 1280×720 | 0.5 s | 0.2 s | much slower |
| 1920×1080 | 1.05 s | 0.42 s | about 16 s |

A 3 s 640×360 clip made at 2x took 6 s in Fast mode.

**Disk:** 0.1 to 0.4 MB per frame on simple footage, 1 to 3 MB per frame at
4K on real footage.

**Cache:** remade frames, as Remove object. Remove object and Enhance
quality can be on together.

## Optical flow (slow motion)

**Where:** **Speed › Standard › Frame blending › Optical flow (AI)**, the
**Smooth slow-mo** button, or the **Hero moment** and **Bullet time** tiles
in **Speed › Speed effects**.

RIFE makes new frames between the real ones, so slow motion is smooth and
has no double image. On a clip with Remove object or Enhance quality, the
new frames are made from the remade frames (the remade frames are made
first), so the removed object stays away in the slow motion too.

**Model:** RIFE v4 (MIT, 22 MB).

**Cost:** 64 ms per new frame at 1280×720 and 145 ms at 1080p on the GPU
(38 ms and 96 ms in Fast mode); 0.25 s and 1.2 s (640×360, 1280×720) on
the CPU. Details on the [Speed](speed.md#cost) page.

**Cache:** JPEG frames in `~/.cache/chukcut/flow/`, in the cache limit.

## Retouch

**Where:** **Video › Retouch**, tick **Retouch**. **Look**: **Natural**,
**Soft**, **Bright** or **Sculpt**. Sliders: **Strength**, **Smooth skin**,
**Brighten eyes**, **Whiten teeth**, **Slim face**.

The first time, chukcut finds the faces in every frame: "Finding the faces…
N %. The clip shows unretouched until then." Retouch is also an effect
(**Effects › Face** in the asset panel).

**Models:** YuNet (MIT, 0.2 MB) finds faces. The MediaPipe face mesh
(Apache-2.0, 4.9 MB) puts 478 points on each face.

**Cost:** about 6 s for 300 frames of 1080×1920, on the GPU or the CPU. The
retouch itself costs almost nothing in the export.

**Cache:** the face points, in `~/.cache/chukcut/landmarks/`.

## Follow face

**Where:** the **Tracking** tab of a title, sticker or photo: **Pin to**,
then **Follow face**. See [Tracking](tracking.md#follow-a-face).

**Models and cost:** as Retouch.

## Follow body part

**Where:** the **Tracking** tab of a title, sticker or photo: **Body part**,
**Person**, then **Follow body part**. See
[Tracking](tracking.md#follow-a-body-part).

**Models:** YOLOX-tiny (Apache-2.0, 20 MB) finds the people. RTMPose-m
(Apache-2.0, 54 MB) puts 17 points on each person: nose, eyes, ears,
shoulders, elbows, wrists, hips, knees, ankles. Both come as zip files;
chukcut checks the zip and takes only the model out of it.

**Cost:** 7 ms for each person in a frame on the GPU, 22 ms on the CPU. A
clip is ready in about the time it takes to decode it.

**Cache:** the body points, in `~/.cache/chukcut/landmarks/`.

## AI tracker

**Where:** the **Tracking** tab: **Tracker › Fast motion (AI)**. See
[Tracking](tracking.md).

**Model:** VitTrack (Apache-2.0, 0.7 MB). 3.5 ms per frame on the GPU; 120
frames in 2 s. On the CPU it is slower, but it works.

## Isolate voice

**Where:** select a clip with sound. Open **Audio** (or **Basic** for an
audio clip), and tick **Isolate voice**.

- **Keep**: **Voice** keeps the speech and removes music and noise.
  **Background** keeps the music and removes the voice.
- **Strength**: **Light**, **Medium** or **Full**.

The clip plays as it was until the new sound is ready: "Separating the
voice… N %". The export uses the same sound.

**Model:** HTDemucs, fine-tuned for vocals (MIT, 316 MB).

**Cost:** 60 s of sound took 11 to 17 s on the GPU and 36 to 40 s on the
CPU, with about 9 s of that to load the model. The worker uses about 1.5 GB
of memory (2 GB on the CPU) while it runs.

**Cache:** a sound file in `~/.cache/chukcut/voice/`.

## Auto captions

**Where:** the asset panel's **Captions** tab. See
[Text and captions](text-and-captions.md#auto-captions).

**Model:** Whisper through whisper.cpp (MIT). Five sizes, from **Tiny
(75 MB, fastest)** to **Large v3 turbo (548 MB, best)**. It runs on the CPU.
A build with `scripts/install.sh --cuda` runs it on an NVIDIA GPU.

**Cache:** the models are in `~/.cache/chukcut/whisper/`. They are not in
the cache limit.

## Tools that use no model

These run on the CPU, in seconds. They need no download.

- **Auto adjust**, **Match colour**: [Colour](colour.md#auto)
- **Stabilise**, **Scene detection**, **Auto reframe**: **Video › Basic**.
  Auto reframe uses the face detector when the AI tools are installed,
  and the person detector on frames without a face.
- **Beats**, **Auto-cut to beat**, **Snap cuts to beats**: **Audio › Beats**
- **Reduce noise** (RNNoise), **Normalize loudness**, **Remove silences**:
  [Audio](audio.md)

## Licences

Every model is free for any use, also commercial use. The table with links is
in the [README](../../README.md#ai-models-and-their-licences).
