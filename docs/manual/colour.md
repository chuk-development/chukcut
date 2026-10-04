# Colour

Select a video, photo, sticker or compound clip. Open the inspector's
**Adjust** tab. It has five sub-tabs: **Basic**, **HSL**, **Curves**,
**Colour wheels** and **Mask**. The preview and the export use the same
shader, so the export shows the same colours as the player.

Every change is one undo step. Below every sub-tab are two buttons:

- **Save as preset** saves the clip's grade under a name. The preset then
  appears in the asset panel under **Filters › My presets**.
- **Apply to all** gives the same grade to the other clips.

Both are off while the clip has no grade.

## Basic

### Auto

- **Intensity**: **Subtle**, **Medium** or **Full**. It sets how far the
  tools below change the picture.
- **Auto adjust** measures the clip and sets exposure, temperature, tint and
  the other basic controls. The status line says what it changed, for
  example "Auto adjust: exposure +0.3, temperature -0.1, tint +0.0".
- **Match to**: choose another clip on the timeline. **Match colour** then
  makes this clip look like that one. With **Frame at playhead**, chukcut
  reads only the frame at the playhead, not the whole clip. The status line
  shows the colour difference before and after.

Auto adjust and colour match use no AI model. They measure eight frames and
write the result into the normal controls below. You can change each value
after that. A clip takes a few seconds.

### LUT

- **Name**: **None**, a LUT from the library, or **Import LUT…** to add a
  `.cube` file (1D or 3D) to the library.
- **Intensity**: how strong the LUT is, in %.

The library is in `~/.local/share/chukcut/luts/`. The asset panel's
**Filters** tab shows chukcut's own looks.

### Adjust

- **Colour**: **Temperature**, **Tint**, **Saturation**, **Vibrance**.
- **Light**: **Exposure**, **Brightness**, **Contrast**, **Highlights**,
  **Shadows**, **Whites**, **Blacks**.
- **Effects**: **Sharpen**, **Clarity**, **Grain**, **Fade**, **Vignette**,
  **Midpoint**, **Feather**.

The checkbox in a section header turns the section off and keeps its
values. The arrow resets the section.

## HSL

Click one of eight colour dots: **Red**, **Orange**, **Yellow**, **Green**,
**Aqua**, **Blue**, **Purple**, **Magenta**. Then change **Hue**,
**Saturation** and **Luminance** of that colour range only.

## Curves

Four dots select the curve: master, red, green, blue. Click on the graph to
add a point. Drag a point to move it. Right-click a point to remove it.

## Colour wheels

**Shadows**, **Midtones**, **Highlights** and **Offset**. Drag in a wheel to
push that range towards a colour. Each wheel has a **Luminance** value.

## Mask: grade only the subject or the background

**Apply to**: **Whole clip**, **Subject** or **Background**.

With **Subject**, the grade changes only the subject. With **Background**,
it changes everything except the subject. The clip is not cut out.

The subject comes from the clip's matte. You set it in **Video › Remove
background**: people (Robust Video Matting), the main object (BiRefNet lite)
or an object that you selected with a click. If the clip has no matte yet,
**Subject** makes a people matte. While the matte is made, the caption shows
"Making the matte… N of M frames". Frames without a matte show the grade on
the whole clip. See [AI tools](ai-tools.md).

Example: a red ball in a grey world.

1. Select the ball: **Video › Remove background › Auto remove**, **Keep:
   Select**, **Select on player**, then click the ball.
2. In **Adjust › Mask**, set **Apply to: Background**.
3. In **Adjust › Basic**, set **Saturation** to 0.
4. Go back to **Video › Remove background** and clear the **Auto remove**
   checkbox. The clip is not cut any more. The grade keeps the matte,
   because it uses it.

## Filters and presets in the asset panel

The asset panel's **Filters** tab shows chukcut's own LUT looks. A click
grades the selected clip. **My presets** holds the grades that you saved with
**Save as preset**.

## Masks, chroma key and blend modes

These are in the **Video** tab. See
[Effects and transitions](effects-and-transitions.md#masks).
