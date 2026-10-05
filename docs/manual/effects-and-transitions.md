# Effects and transitions

## Effects

Open the asset panel's **Effects** tab. The effects run on the GPU, in the
preview and in the export.

| Category | Effects |
|---|---|
| **Blur** | Reduce noise, Blur, Zoom blur |
| **Light** | Glow, Light sweep |
| **Motion** | Shake, Motion blur |
| **Retro** | RGB split, Glitch, VHS, Pixelate |
| **Distort** | Mirror, Kaleidoscope |
| **Film** | Grain, Halation, Bloom, Gate weave, Letterbox |
| **Layout** | Picture in picture, Side by side, Top and bottom, Three rows, Three columns, Grid, Frame |
| **Face** | Retouch |

There are two ways to use an effect:

- **On a clip**: select a clip, then click the effect. The effect changes
  only that clip.
- **As an effect clip**: drag the effect onto the timeline (or click it with
  no clip selected; it lands at the playhead). An effect clip sits on an
  effect lane and changes everything below it, for as long as it lasts. Trim
  and move it like any clip.

### Change an effect

Select the clip and open the inspector's **Effects** tab. Each effect in the
list has:

- an eye (**Turn off** / **Turn on**), up and down arrows for the order, a
  reset arrow, and a bin (**Remove effect**);
- its parameters, each with a slider, a reset and a keyframe button.

**Apply to** at the top (video clips only): **Whole clip**, **Subject** or
**Background**. With **Subject** or **Background**, the effects change only
that part of the picture, for example a blur on the background only. The
subject comes from the clip's matte. See
[AI tools](ai-tools.md#grade-or-effects-on-the-subject-or-the-background).

### Motion blur

**Effects › Motion › Motion blur** smears a clip along its own motion: its
keyframes, animations and tracking. Set the **Shutter angle** (0 to 360°)
and the number of **Samples**. A clip that does not move stays sharp. The blur does
not see motion inside the video picture itself.

### Layouts

**Effects › Layout**:

- Select one clip and click **Picture in picture**: the clip becomes a
  small window with round corners, a border and a shadow.
- Select several clips and click **Side by side**, **Top and bottom**,
  **Three rows**, **Three columns** or **Grid** for a split screen.

## Animations

Select a clip and open **Animation**.

- **In**: plays from the clip's first frame. **Out**: plays into its last
  frame. Both follow the clip when you trim it. Presets: **Fade**, **Slide
  left/right/up/down**, **Zoom in**, **Zoom out**, **Pop**, **Bounce**,
  **Spin**, **Blur**, **Wipe right/left/up/down**, **Swing**, **Shake**,
  **Rise**, **Flip**, **Whip**.
- **Combo**: loops for the whole clip. Presets: **Pulse**, **Heartbeat**,
  **Wobble**, **Rock**, **Float**, **Jitter**, **Rotate**, **Flicker**.
- Each has **Duration** (on Combo: **Speed (one loop)**), **Strength** and
  **Easing**.
- **Zoom** (not on titles):
  - **Punch-in zoom** zooms in over the clip: **Zoom**, **Push-in time**,
    **Easing**, and **Pivot**. Drag the crosshair on the player to put the
    pivot on a face.
  - **Auto zoom**: when the clip is one of several jump cuts from one take,
    **Auto zoom jump cuts** punches in every second one. It is one undo
    step.

Titles have **Text** in place of **Zoom**. See
[Text and captions](text-and-captions.md#animate-a-title).

## Transitions

Open the asset panel's **Transitions** tab.

- **Basic**: **Cross dissolve**, **Dip to colour**, **Wipe**, **Slide**,
  **Zoom**, **Blur**.
- **Seamless**: **Zoom in through**, **Zoom out through**, **Spin**, **Whip
  pan**, **Push**.
- **Library**: about 120 transitions from the gl-transitions collection.
  Each tooltip names its author and licence.

Select a clip, then click a transition. It goes on the cut between that
clip and the next one. On the timeline, the transition is a badge on the
cut. Drag its edge to change its length.

## Masks

**Video › Mask** (video, photo and compound clips).

1. Click a shape under **Add mask**: **Linear**, **Mirror**, **Circle**,
   **Rectangle**, **Star** or **Heart**.
2. Drag its handles on the player.
3. Set **Position X**, **Position Y**, **Width**, **Height**, **Rotate**,
   **Feather** and, for a rectangle, **Round corners**. Each value can have
   keyframes.

A clip can have several masks. Each mask has **Add**, **Subtract** or
**Intersect**, and **Invert**. The eye turns a mask off. The arrows change
the order. The bin removes it.

## Chroma key

**Video › Remove background › Chroma key** (a green screen).

1. Tick **Chroma key**.
2. Click the eyedropper, then click the background colour on the player.
3. Set **Intensity**, **Softness**, **Spill removal** and **Edge shrink**.
4. **Show matte** shows what stays (white) and what goes (black).

## Blend modes

**Video › Basic › Blend**: **Mode** and **Opacity**. The modes: **Normal**,
**Multiply**, **Screen**, **Overlay**, **Soft light**, **Hard light**,
**Darken**, **Lighten**, **Colour dodge**, **Colour burn**, **Difference**,
**Exclusion**, **Add**, **Subtract**.

## Stickers

Open the asset panel's **Stickers** tab.

- **Smileys**, **People**, **Animals**, **Food**, **Travel**,
  **Activities**, **Objects**, **Symbols**, **Flags**: emoji in three looks,
  **3D**, **Flat** and **Noto**.
- **Icons**: search for a word, for example "heart" or "arrow".
- **Animated**: Noto animated emoji by Google (CC BY 4.0).

Click a sticker to add it at the playhead. The images download on first use,
so the tiles can stay empty for a few seconds.

A Lottie `.json`, an animated GIF or an animated WebP that you import is an
animated sticker too. By default it loops. **Video › Basic › Sticker › Play
once** plays it one time and then holds the last frame.

A sticker is an image clip: it has **Video**, **Animation**, **Adjust**,
**Tracking** and **Effects**. It can follow an object. See
[Tracking](tracking.md).

Limits: chukcut draws no text layers and no image layers inside a Lottie
file. A GIF must have a transparent colour in its palette, or it shows as a
box.

## Crop, rotate and flip

Select a video or image clip and open **Video › Crop**. While this tab is
open, the player shows the whole picture of the clip. A box marks the part
that stays, and the rest is dark.

- **Crop**: the ratio buttons **Free**, **Original**, **9:16**, **16:9**,
  **1:1**, **4:5**, **4:3**, **3:4** and **2.35:1**. A ratio sets the largest
  box of that shape around the centre of the current box. **Original**
  removes the crop. A line says how many pixels the crop keeps.
- **On the player**: drag inside the box to move it. Drag a corner or an
  edge to resize it. With a ratio other than **Free**, the box keeps its
  shape. The box never leaves the picture. The crop is written when you let
  go: one undo step.
- **Rotate and flip**: **Rotate 90° left**, **Rotate 90° right**, **Flip
  horizontally**, **Flip vertically**, and **Rotate** for any angle. A flip
  button is lit while the clip is flipped. The rotation takes keyframes like
  the one in **Basic › Transform**.
- **Keyframe**: the diamond adds a crop keyframe at the playhead. The
  keyframe holds the crop that the player shows, so nothing moves yet.
  Click the diamond again on a keyframe to delete it. The arrows move the
  playhead to the previous or the next crop keyframe.
- **Animate a crop**: add a keyframe, move the playhead, then drag the box
  or click a ratio. When the crop has keyframes, each change sets the
  keyframe at the playhead. If there is no keyframe there, chukcut adds one.
  The crop moves between the keyframes. Right-click the diamond to set the
  easing. The **Keyframe easing** graph shows under **Crop** and in
  **Basic**, with a **Crop** chip.
- The reset arrow of **Crop** removes the crop and all its keyframes, in one
  undo step. The reset arrow of **Rotate and flip** sets the rotation to 0
  and removes the flips.

When you leave the tab, the player shows the cropped clip again. The cropped
part fills the clip's frame: a 9:16 crop of a 16:9 clip becomes a 9:16 clip
on the canvas. When the crop moves, the clip's shape on the canvas changes
with it. A crop keyframe stays with the clip when you move, trim or split
the clip, like a transform keyframe. A freeze frame keeps the crop of its
instant. `chukcut-cli crop` sets the same crop from a script, and `crop
--at` sets a crop keyframe.

## Video › Basic

The other sections of **Video › Basic**:

- **Transform**: **Scale** (with **Uniform scale**), **Position** (**X**,
  **Y**), **Rotate**, and buttons to align the clip to an edge or the
  centre.
- **Keyframe easing**: see [Timeline editing](timeline.md#keyframes).
- **Stabilise**, **Scene detection**, **Auto reframe**: see
  [Timeline editing](timeline.md#analysis-tools-no-ai-model).
- **Reduce image noise** (video and image clips): tick it to smooth sensor
  noise. **Strength** sets how much. **Keep detail** keeps fine texture: a
  high value averages only pixels whose colour is very close. Edges stay
  sharp at every setting. **Mode**: **Spatial** looks at one frame;
  **Temporal** also uses the frames before and after (see below). It is the
  **Reduce noise** effect, so it also shows in the **Effects** tab, and both
  sliders take keyframes. Clearing the tick removes the effect. It runs on
  the GPU, in the preview and in the export.
- **Enhance quality**: a button that opens **Video › Enhance**, where the AI
  upscaling is (see [AI tools](ai-tools.md#enhance-quality)).
- **Optical flow** (video clips): a button that opens **Speed › Standard**,
  where **Optical flow (AI)** is (see [Speed](speed.md)).

### Reduce noise

**Effects › Blur › Reduce noise** is an edge-preserving filter: it averages
each pixel with its neighbours, but only with those of a similar colour.
Grain and sensor noise go; an edge, which is a large difference, stays.
Above a **Strength** of 60 it runs twice, which smooths more without
blotches. The radius grows with the frame, so the preview and a 4K export
look the same. It does not use AI.

**Mode › Temporal** also compares each pixel with the same pixel in the
frame before and the frame after. Sensor noise changes from frame to frame,
but a still picture does not, so where the picture holds still the three
frames are averaged. This removes more noise than the spatial filter and
keeps more detail. Where something moves, the frames differ much more than
noise does, and only the current frame is used, so nothing leaves a trail.
The spatial filter then runs gently on the result. **Strength** and **Keep
detail** set both parts. Temporal mode works on video clips. On a clip that
uses frame blending, motion blur or a blur animation, only the spatial part
runs. The first frame of a file has no frame before it and uses the frame
after only.
