# 0026 — Frame blending and motion blur average draws; animated stickers are image clips drawn by vello

Date: 2026-10-04. Status: accepted.

## Decision

**Frame blending and motion blur are one mechanism: several weighted draws
of one clip, summed.** The compositor draws a clip that blends frames, or
whose motion blur sees it move within the shutter, as `Draw::Accumulated`:
every source frame (two, weighted by the fractional source position) at
every placement along the shutter (each `1/n`) goes through the ordinary
quad shader's premultiplied fragment, additively, into an `Rgba16Float`
layer. A resolve pass turns the premultiplied mean into an ordinary
straight-alpha clip layer, which then takes the paths every clip layer
takes: its effects, its blend mode, the "over" pass. `render::accumulate`.

- **Frame blending** is a per-clip setting (None / Frame blend) stored as an
  `extras` block (`{"frame_blend": {"mode": "blend"}}`), like `audiofx`'s.
  `speed::blend::blend_for` turns the clip's source time into the two
  neighbouring frames' middles and the weight. It works at any speed and on
  any curve, because it only asks `TimeMap` where in the file the clip is.
- **Motion blur** is an effect in the catalogue (`motion_blur`: shutter angle,
  samples), so the Effects tab, keyframes, undo and the CLI treat it like
  any effect, but it is never a pass of the effect chain
  (`FxInstance::at_rest` is always true for it). The compositor samples the
  clip's whole placement — keyframes, In/Out/Combo animation, a followed
  track, a stabilisation window — at instants spread over a shutter centred
  on the frame, and draws the one decoded frame at each.
- **The provider keeps the last two frames per video** (`CachedTexture::previous`),
  so the earlier of a blend's two frames is a cache hit and the decoder never
  seeks backwards during playback or export. The preview's decode-ahead asks
  for both.

**Animated stickers are image materials.** A Lottie `.json`, or a GIF or WebP
with more than one frame, is an `ImageMaterial`; `modules::animated` loads it
(cached by path, size and mtime) and the provider draws the frame at the
clip's time, looped or held on its last frame (a per-clip `extras` block,
`sticker_playback`). Lottie is parsed by velato and rasterised by vello on
`gpu::render_context()`'s device; GIF and WebP are decoded by the `image`
crate.

## Why

- **No new `EditCommand`, no new `Segment` field, no new `MaterialKind`.**
  Every setting is an `extras` block or a catalogue effect, and every edit is
  the remove + insert of one segment, so undo is exact and no struct literal
  in another branch breaks. Old files open unchanged; a project without these
  features saves byte-identical.
- **Averaging in float, after the quad shader, keeps every clip feature.** The
  grade, LUT, masks, chroma key, crop and YUV conversion all run per draw in
  the shader they already run in. Blending the decoded frames first would
  have needed a second conversion path for NV12, imported VA surfaces and
  RGBA alike. Summing in 8 bits would round each of eight 1/8 addends and
  step the result by up to four code values.
- **A still clip with motion blur costs nothing**: placements that do not
  differ are drawn once, byte-identical to no motion blur (tested).
- **vello on our device, not dotlottie-rs.** dotlottie-rs builds ThorVG from
  C++ and renders on the CPU; vello is Rust, renders on the GPU we already
  hold, and Noto's animated emoji (the corpus that matters) use no feature
  velato lacks. velato 0.12 is used without its `vello` feature because it
  pins vello 0.10 (wgpu 29); our eight-line `RenderSink` targets vello 0.11,
  which is on our wgpu 30, so there is one wgpu in the build.
- **GIF and WebP through `image`, not FFmpeg.** FFmpeg 6.1, the system
  library, cannot decode an animated WebP ("image data not found"); one
  decoder for both keeps the frame timing rules (a 0–10 ms GIF delay is
  100 ms, as browsers do) in one place. A GIF imported by the user is now a
  looping sticker with transparency, not a video clip.
- **Image materials, not a new material kind**, because a sticker already is
  an image clip on an overlay lane: it moves, keyframes, follows a track and
  takes masks and effects with no new code anywhere.

## What it costs

- An accumulated clip costs `frames × placements` draws plus two full-frame
  passes; motion blur at 8 samples on a blending clip is 16 draws.
- Not applied inside a transition window, nor while a blur *animation* runs
  on the clip (that owns the clip's layer).
- Motion blur samples placement only: the picture inside a video is one
  decoded frame, so motion *inside* the footage is not blurred (frame
  blending is the tool for that).
- The VAAPI path now holds two decoder surfaces per material instead of one
  between decodes, inside `EXTRA_HW_FRAMES`' headroom.
- velato does not draw Lottie text layers, embedded images (ignored) or some
  effects. GIF/WebP frames are kept decoded in memory, refused above 384 MB.
- Optical flow ("Optical-flow-light") is not built.

## What would change our minds

- A pipeline that needs the source frames blended *before* the quad (an
  optical-flow warp): then the blend becomes a pre-pass on the decoded
  frames, and frame blending would move with it.
- velato falling behind the Lottie files people bring: dotlottie-rs as a
  second renderer for the files velato cannot draw.
