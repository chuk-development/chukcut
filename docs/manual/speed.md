# Speed and slow motion

Select a video, audio or compound clip and open the **Speed** tab. It has
the sub-tabs **Standard**, **Curve** and **Speed effects**.

## Standard: one speed for the whole clip

- **Speed**: drag the slider (0.1x to 100x) or type a value. The clip gets
  shorter or longer on the timeline.
- **Duration**: shows the original length. Type a new length to set the
  speed from it.
- **Change audio pitch**: off, the sound keeps its pitch at any speed. On,
  faster sound is higher and slower sound is lower.
- **Reset** (at the bottom) goes back to 1x.

If the clip has a speed curve, a speed set here replaces the curve.

## Curve: speed ramps

Pick a ramp: **Montage**, **Hero**, **Bullet**, **Jump cut**, **Flash in**,
**Flash out**, or **Custom** to draw your own. **None** turns the curve off.

The graph goes from 0.1x to 10x. Click to add a point. Drag a point to move
it. Right-click a point to remove it. **Duration** shows the length before
and after. **Point** shows the speed of the selected point. The timeline
shows the ramp's name on the clip.

The sound follows the curve. chukcut keeps its pitch unless **Change audio
pitch** is on.

## Speed effects: a ramp and smooth frames in one click

A ramp from the **Curve** tab alone looks jerky in its slow part, because a
slowed clip holds each source frame. **Speed › Speed effects** (video clips)
puts a ramp and the frame smoothing that suits it on the clip, as one undo
step:

| Effect | Ramp | Smoothing |
|---|---|---|
| **Smooth montage** | Montage | Blend |
| **Hero moment** | Hero (down to 0.25x) | Optical flow (AI) |
| **Bullet time** | Bullet (down to 0.2x) | Optical flow (AI) |
| **Smooth jump** | Jump cut | Blend |
| **Flash in** | Flash in | Blend |
| **Flash out** | Flash out | Blend |

**Off** removes the ramp and the smoothing. The effects with optical flow
start the bake at once; see "How optical flow works" below. A line under the
tiles says which effect the clip has. After you pick one, you can change the
ramp in **Curve** and the smoothing in **Standard**. The tile then no longer
shows as selected. The ramp goes on the linked audio clip too, like a ramp
from **Curve**.

## Frame blending: smooth slow motion

At 0.25x, each source frame stays on the screen for four frames. The motion
looks jerky. **Speed › Standard › Frame blending** (video clips only) has
three modes:

| Mode | What it does | Cost |
|---|---|---|
| **None** | Each frame stays until the next source frame is due. | none |
| **Blend** | Each new frame mixes the two source frames around it. Smooth, but a fast object shows a double image. | small, live in the preview |
| **Optical flow (AI)** | A model (RIFE) makes real frames between the source frames. A moving object is in the right place, with no double image. | a bake; see below |

**Smooth slow-mo** is one click: it sets the clip to 0.5x (when it is not
slowed already) and turns on **Optical flow (AI)**. It is one undo step.

### How optical flow works

- The model downloads once (RIFE, MIT licence, 22 MB).
- chukcut bakes the new frames in the background. A strip under the
  inspector shows "Making slow-motion frames… N of M" with **Stop**.
- The preview shows **Blend** until a frame is baked, and the baked frame
  after that.
- Below the modes, a line says "All N in-between frames are baked", or
  "M of N in-between frames are baked" with **Finish missing frames**.
- After an edit (a trim, a new speed), chukcut bakes the missing frames
  again on its own.
- An export bakes the missing frames first. If it cannot, it stops and
  says why.

### Cost

Measured on an RTX 3060 with the CUDA bundle:

| Frame size | GPU, per new frame | CPU (4 threads), per new frame |
|---|---|---|
| 640×360 | 18 ms | 0.25 s |
| 1280×720 | 70 ms | 1.2 s |
| 1920×1080 | 173 ms | much slower |

A 3 s 720p clip at 0.25x needs 267 new frames: 25 s on the GPU. On the CPU,
the same clip takes minutes. The strip then says "Optical flow runs on the
CPU here: N frames, about M min" and tells you that a GPU bundle makes it 10
to 30 times faster.

The frames are JPEG files in `~/.cache/chukcut/flow/`. They count towards
the cache limit. A 4K clip gets its new frames at 1080p.

## Motion blur

Motion blur is an effect, not a speed setting. Add it from the asset panel's
**Effects › Motion**. See [Effects and transitions](effects-and-transitions.md).

## Freeze frame

Select a video clip, put the playhead on a frame, and use **Freeze frame**
in the clip's right-click menu. chukcut splits the clip and puts a 3 s
still of that frame between the two parts.
