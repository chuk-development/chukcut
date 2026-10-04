# The showcase project and a tour of the app

This page shows chukcut with one project that uses most of its features. A
script builds the project from generated media, with `chukcut-cli` only. Then
the page goes through the app area by area: what each area does and how to
open it.

Everything here was seen working on 2026-10-04 (release build, RTX 3060 for
the CLI and the ML worker, the app on a private Xvfb display with lavapipe).
The showcase was extended the same day with the newer features: a compound
clip, a second timeline made from a template, an animated sticker with
motion blur, frame blending, optical-flow slow motion and select object with
a background-only grade. QA pass 3 (agent/qa3) added a crop, a speed
effect (Bullet time), auto adjust, colour match, remove object and enhance
quality on a small clip, and isolate voice on the voiceover. The last
section lists what did not work.

## Make the showcase

```bash
memguard-allow 16G cargo build --release -p chukcut -p chukcut-cli -p chukcut-ml-worker -j 3
scripts/demo.sh --export          # media + project + export, about 3 min
./target/release/chukcut _scratch/demo/showcase.chukcut
```

`scripts/demo.sh` writes everything to `_scratch/demo/` (ignored by git):

| Path | What |
|---|---|
| `media/` | the generated media (below) |
| `showcase.chukcut` | the project |
| `showcase.mp4`, `showcase.srt` | the export and its caption sidecar (with `--export`) |
| `xdg/` | the CLI's own `HOME` and XDG folders, so the script does not change your settings |

Options: `--media` makes the media again, `--export` also exports,
`--no-ml` leaves out the ML steps. `CHUKCUT_CLI=/path/to/chukcut-cli` uses
another binary. The script needs `ffmpeg` (with the `flite` filter for the
voice), `python3`, `jq`, `bc` and a Vulkan device (the template section is
rendered).

**ML steps.** Optical-flow slow motion and Bullet time (RIFE), select
object (MobileSAM and VitTrack), remove object (LaMa), enhance quality
(Real-ESRGAN) and isolate voice (HTDemucs) run only when the models and a
GPU runtime are already installed (`chukcut-cli ml install gpu`, `ml
install rife`, …): the script links your ML folder (`~/.cache/chukcut/ml`,
or `$CHUKCUT_DEMO_ML_CACHE`) into its own cache and downloads nothing.
Otherwise the comet gets frame blending instead of optical flow, the
pattern gets Smooth montage instead of Bullet time, the other ML steps are
left out, and the script prints a `note:` line for each. The mattes, flow
frames, remade frames and the isolated voice are baked into `xdg/cache/`;
the app run with your own settings bakes them again into your cache in the
background: "Preparing 451 frames and 1 voice" in the title bar, about
1.5 min on an RTX 3060. With Fast mode and the TensorRT add-on, the first
run also prepares each model for its frame size once (Real-ESRGAN at
270×480 31 s, RIFE at 1080×1920 84 s); later runs reuse it.

The project opens in the app with your normal settings. Only the "Showcase"
export preset is in the script's own config folder; the app shows it under
"My presets" only when it runs with the same `XDG_CONFIG_HOME`.

### The media

All media is made by FFmpeg at 1080x1920, 30 fps. Nothing is downloaded.

| File | Made from | Used for |
|---|---|---|
| `01-gradients.mp4` | `gradients` source | the opening shot, with glow |
| `02-fractal.mp4` | `mandelbrot` zoom | grade, LUT, curve, grain, punch-in zoom |
| `03-pattern.mp4` | `testsrc2` with a hue sweep | the speed ramp |
| `04-ball.mp4` | a red ball on a figure-of-eight path over a dimmed `testsrc2` | motion tracking, select object |
| `05-greenscreen.mp4` | a rotating test card on green | chroma key and a mask |
| `06-life.mp4` | Conway's `life` | picture in picture |
| `07-comet.mp4` | a bright disc that crosses `gradients` at about 17 px a frame, 3 s | optical-flow slow motion, colour match |
| `08-lowres.mp4` | a `mandelbrot` zoom at 270×480, CRF 38, with a white box as a "watermark", 2 s | remove object, enhance quality 4x |
| `pulse.gif` | `geq`: a pulsing cyan ring, transparent background, 2 s loop | animated sticker, motion blur |
| `voice.wav` | `flite` speech synthesis, -16 LUFS | voiceover |
| `music.wav` | `aevalsrc`: a kick at 120 BPM over a chord, 30 s | music bed |
| `sticker.png` | `geq`: a smiley with a transparent background | sticker |
| `teal-orange.cube` | a 17-point LUT the script writes | the LUT |
| `voice.srt` | written by the script, times measured with `silencedetect` | captions |

### What the project contains

The result is two timelines. **Timeline 01** is 27.7 s on 8 lanes
(`chukcut-cli info _scratch/demo/showcase.chukcut`):

| Time | Lane | What | CLI command |
|---|---|---|---|
| 0 – 4 s | Video 1 | gradients with **auto adjust** (60 %) and a **glow** effect | `trim --ripple`, `auto-adjust --amount 0.6`, `effect add glow --clip` |
| 4 – 8.5 s | Video 1 | fractal: **grade** (exposure, contrast, saturation, temperature, vignette), the **LUT** at 80 %, an S **curve**, **film grain**, a **punch-in zoom** | `grade --set … --lut`, `curve`, `effect add film_grain`, `zoom` |
| 8.5 – 13.7 s | Video 1 | 3 s of the pattern with the **Bullet time speed effect**: the bullet ramp and **optical flow (AI)** in the slow middle, one undo step (without RIFE: Smooth montage, the montage ramp with frame blending) | `speed-effect --effect bullet` |
| 13.7 – 19.7 s | Video 1 | the ball clip: **select object** on the ball (MobileSAM + VitTrack), the grade limited to the **background** (saturation 0, darker), the matte uncut, so the red ball is the only colour | `select-object --point`, `apply-to --grade background`, `grade`, `remove-background --off` |
| 19.7 – 25.7 s | Video 1 | the comet at half speed with **optical flow (AI)**: RIFE makes the 89 frames in between; **colour match** gives it the fractal's teal-and-orange look | `smooth-slow-mo --speed 0.5`, `colour-match --to` |
| 25.7 – 27.7 s | Video 1 | the small, blocky clip: the white box **removed** (LaMa, a box in every frame), then **enhanced 4x** from 270×480 to 1080×1920 (Real-ESRGAN) | `remove-object --box`, `enhance-quality --scale 4` |
| cuts | Video 1 | **transitions**: `gl:crosswarp`, `seamless:zoom_in`, `dissolve` | `transition add --kind` |
| 4.5 – 8 s | Video 3 | **picture in picture** top left (round corners, border, shadow), **cropped** to a square from the middle, with **blend mode** Screen | `layout pip`, `crop`, `blend screen` |
| 6 – 9 s | Video 2 | **sticker** (the PNG), In animation *pop*, Combo *wobble*, rotation **keyframes** | `place`, `set`, `animate`, `keyframe` |
| 9 – 13.2 s | Video 2 | green screen with **chroma key** and a rounded **rectangle mask** that opens from 20 % to 90 % (mask keyframes) | `chroma-key`, `mask --add rectangle --at` |
| 0.2 – 4 s | Video 2 | **compound clip** "Intro titles" (double-click to open it) holding the two titles below | `compound create --name` |
| 0.2 – 2.1 s | inside it | **title template** *pop-headline* "chukcut" | `title template` |
| 2.1 – 4 s | inside it | styled title, **text animator** word by word (*fade_up*), Out animation *fade* | `title add`, `animate-text`, `animate --slot out` |
| 20.2 – 23.2 s | Video 2 | **animated sticker** (the GIF) that crosses the frame and back (position keyframes) with **motion blur** (shutter 300°, 12 samples) | `sticker --file`, `keyframe --property x`, `effect add motion_blur` |
| 13.7 – 19.7 s | Text 2 | "tracked" label that **follows the ball** (KLT tracker, 180 frames) | `track --overlay` |
| 13.5 – 14.1 s | Effects 1 | **shake** effect clip over the cut into the ball | `effect add shake --at` |
| 0.7 – 9.9 s | Captions | five **captions** from the SRT, *karaoke* style, the spoken word in yellow | `captions import --preset karaoke`, `captions style` |
| 0.5 – 10.5 s | Audio 1 | voiceover: **normalised** to -14 LUFS, **EQ**, **isolate voice** (HTDemucs keeps the speech, drops the room tone) | `normalize`, `audio-effect add eq3`, `isolate-voice --keep voice` |
| 0 – 27.7 s | Audio 2 | music: **ducked** 10 dB under the voice (6 volume keyframes), **reverb** | `duck`, `audio-effect add reverb` |
| ruler | | six **markers**: Intro, Grade + LUT, Speed ramp, Tracking, Slow motion, Remove + enhance | `marker add` |
| export | | own **export preset** "Showcase" (TikTok, CRF 19) | `preset save`, `export --preset user_showcase --sidecar srt` |

**Template cut**, the second timeline (the tab next to Timeline 01), is 6 s:
the **Quick Cuts template** filled with the six demo clips and put into the
showcase as a timeline of its own (`template apply --into … --as timeline
--name "Template cut"`, one undo step), with a label over it on a text lane
of its own (`lane-add --kind text`, `title add --track`).
Its clips, titles and transitions stay editable, and its six slots are
listed by Templates › This project and `template slots`.

The export renders Timeline 01: 831 frames (27.7 s at 30 fps), H.264
1080x1920 and AAC, 29.7 MB, 22–46 s with the software encoder on this
machine (the slower runs with other builds running). The whole script with
`--media --export` took 3 min 11 s on the RTX 3060, a run without `--media`
and with every frame already baked 55 s
(Fast mode, TensorRT engines mostly prepared already), of which select
object took 17 s (180 frames), isolate voice 13 s and enhance quality 31 s
to prepare TensorRT for 270×480 the first time. Frames from the export
match `render-frame` (mean difference 0.9–1.7 code values, the H.264
encode; 3.8 over the film grain at 6 s) and the app's player.

## A tour of the app

### Start screen

Starts when the app opens without a file. **New project** with a canvas
(9:16, 16:9, 1:1, 4:5), a name and a frame rate; a click on a canvas or a
frame rate is kept even when the first clip has another shape. **Open
project…**, the **Templates** grid (eleven built-ins: a click asks for a clip
per slot and makes the project) and the **Projects** grid (recent projects
with a thumbnail, age, length and path). **Shortcuts** and **Settings** at
the bottom left.

### Title bar

The app name, **Menu** (New project, Open, Save, Save as, Import media,
Export, Settings, Keyboard shortcuts, Quit, each with its shortcut), the save
state ("Saved"), the project name with its canvas and frame rate, and
**Export** at the right. While an opened project's AI frames are made, a
chip says so: "Preparing 451 frames and 1 voice · 49 % Stop". While an
export queue runs, the chip shows it ("Queue 1 of 1 · MP4 · aq · 8 %").

### Asset panel (top left)

Tabs across the top; each has categories on the left and a search field.

- **Media**: Import (Ctrl+I; the system portal, or the built-in file browser
  when there is no portal), Project media (the imported files with
  thumbnails, an "Added" badge and the length), AI tools.
- **Audio**: Import, Project audio, Music library, Sound library, Text to
  speech, Sound effects, Music. The library and cloud parts need a network or
  an account.
- **Text**: Basic, Outline, Box, Glow, Retro and Templates. A click adds the
  style at the playhead; a drag puts it on the timeline.
- **Stickers**: Smileys, People, Animals, Food, Travel, Activities, Objects,
  Symbols, Flags, Icons, in three looks (Fluent 3D, Flat, Noto), and
  **Animated** (Noto Animated Emoji, Lottie). The images are downloaded on
  first use, so the tiles stay empty for a few seconds. A GIF, WebP or Lottie
  file you import is an animated sticker too (the showcase's `pulse.gif`).
- **Captions**: Auto captions (a cloud account or whisper.cpp on this
  machine; word or sentence captions, lines, length, auto emoji), Captions,
  Style, Import & export (SRT, VTT), Translate.
- **Effects**: Blur, Light, Motion (shake, **motion blur**), Retro, Distort,
  Film, Layout. A click puts the effect on the selected clip; a drag onto the
  timeline makes an effect clip.
- **Transitions**: Basic (dissolve, dip to colour, wipe, slide, zoom, blur),
  Seamless, Library (the gl-transitions). A click puts the transition on the
  cut after the selected clip.
- **Filters**: our own LUT looks (Cinematic, Film, …). A click grades the
  selected clip.
- **Stock**: Pexels, Pixabay and Freesound search, after an account with
  your own key is added in Settings → Accounts.
- **Templates**: the built-in templates by category, My templates, and
  **This project** (its slots, and Save as template for the selected clips).

### Player (top centre)

The frame at the playhead, the time code and the length, previous frame,
play (Space), next frame, the preview quality (**Full** or lower), the
canvas aspect ratio, zoom to fit and full screen. **⋯ → Save frame as
image…** writes the frame as a PNG. When a follower clip is selected, the
player draws the track's path.

On Xvfb with lavapipe the first frame takes 15 to 30 s to appear after the
project opens; every later frame takes a few seconds. On a real GPU this is
not the case.

### Timeline (bottom)

Toolbar: select/blade tool, undo, redo, split, delete left, delete right,
delete, marker, record voiceover (microphone), main-track magnet, snapping,
zoom to fit, zoom out, the zoom slider, zoom in.

Above the lanes, the **timeline tabs** (Timeline 01, Template cut; **+**
adds one; right-click renames, duplicates or deletes). Inside a compound
clip the tabs become breadcrumbs (Timeline 01 › Intro titles); a click on
Timeline 01 closes it. **Alt+G** makes a compound clip of the selected
clips, **Alt+Shift+G** puts its clips back, a double-click opens it.

Lanes from top to bottom: titles, effect clips, captions, the overlay video
lanes, the main lane (with **Cover**), audio lanes. Each lane has lock, show
and mute buttons. Clips show thumbnails or waveforms, a dot for keyframes, a
speed-ramp label ("Bullet · …") and a ⋈ button on each transition. Markers are
coloured flags on the ruler.

The wheel scrolls the time; Ctrl+wheel zooms; **Shift+wheel scrolls the
lanes up and down** (needed to reach the music lane in the showcase). Keys:
`?` (Menu → Keyboard shortcuts) lists them all; S or Ctrl+B splits, Delete
removes, Q/W delete left/right of the playhead, M adds a marker, Ctrl+C/X/V/D
for the clipboard.

### Inspector (top right)

Shows **Details** of the project (name, path, colour space, media, proxy
policy, timeline name, ratio, resolution, frame rate, length; **Change**
edits them) when nothing is selected. With a clip selected, the tabs follow
the clip's kind:

- **Video clip**: *Video* (Basic: transform, blend mode and opacity,
  stabilise, scene detection, auto reframe; Remove background: **Auto
  remove** with Keep People (RVM), Objects (BiRefNet, GPU only) or **Select**
  (click the object on the player; MobileSAM + VitTrack), Cut out instead,
  show matte, and chroma key with colour picker, intensity, softness, spill,
  edge shrink; Crop: ratio presets, a box with handles on the player,
  rotate and flip; Mask: linear, mirror, circle, rectangle, star, heart,
  each with add/subtract/intersect, invert and keyframable values; Retouch:
  four looks and five sliders on the faces; Enhance: **Remove object**
  (select or paint on the player) and **Enhance quality** Off / 2x / 4x),
  *Audio* when the file has sound, *Speed* (Standard with **Frame blending**:
  None, Blend, Optical flow (AI), and **Smooth slow-mo**; Curve; Speed
  effects; change audio pitch), *Animation* (In, Out, Combo, Zoom), *Adjust*
  (Basic: **Auto adjust**, **Match colour** to another clip, LUT and adjust
  controls; HSL; Curves; Colour wheels; Mask with
  **Apply to** whole clip, subject or background; Save as preset, Apply to
  all), *Effects* (the clip's effect stack with each parameter, reset and
  keyframe buttons, and Apply to whole clip, subject or background).
- **Compound clip**: Video (Basic, Mask), Audio, Speed, Animation, Adjust,
  Effects, like a video clip without the ML tools.
- **Sticker**: Video (with *Play once* for an animated sticker), Animation,
  Adjust, Tracking, Effects.
- **Title or caption**: *Text* (words, font, size, bold/italic/underline,
  alignment, letter and line spacing, fill, outline, shadow, box), *Video*,
  *Animation*, *Tracking* (a follower: follow mode, smoothing, re-track from
  here, bake to keyframes, stop following, remove track), *Effects*.
- **Audio clip**: *Basic* (volume, fades, normalise loudness with the measured
  before → after, reduce noise, remove silences with Review pauses and Filler
  words, **Isolate voice** with Keep voice or background and a strength,
  equalizer, …), *Voice changer*, *Speed*.

### Export (the Export button, or Ctrl+E)

A dialog with the cover frame at the left. Preset (Social, YouTube, Master,
Audio only, GIF, Custom, and **My presets**; **+** saves the current
settings), name, folder, Video / Audio only / GIF, resolution, bitrate,
codec (H.264, HEVC, AV1, ProRes), format, frame rate, the encoder that will
be used (here "GPU · H.264 (NVIDIA NVENC)"), colour space, audio settings, and
the length and a size estimate at the bottom. **Export** runs now; **Add to
queue** puts it in a queue that keeps running when the dialog is closed.

### Settings (Menu → Settings, Ctrl+,)

New projects (canvas, frame rate), preview and playback (preview resolution,
largest preview edge, audio scrubbing), proxies and cache (proxy policy,
proxies on disk, cache size and limit), **AI acceleration** (what models run
on, the GPU bundles to install or remove, each model with its size and
licence, **Fast (fp16/TensorRT)** with the TensorRT add-on and what each
heavy model runs on, the baked mattes and remade frames), **Performance**
(video decoding and AI runtime pickers), **Keyboard shortcuts** (Edit shortcuts…: search,
change, add, remove, reset, three presets, conflicts), the cloud accounts,
the hardware report and the log folder.

### CLI and MCP

`chukcut-cli` does every edit in this page from a shell (`docs/cli.md`).
`chukcut-cli mcp` serves the same operations to an MCP client: 154 tools,
including `view_frame`, which returns the frame as a PNG
(`claude mcp add chukcut -- /path/to/chukcut-cli mcp`).

## What does not work, or is missing

Found while building the showcase. The open ones are also in `docs/QA.md`.

- **Fixed (first pass):** Shift+wheel did not scroll the timeline's lanes on
  X11, so lanes below the panel (the music lane of the showcase) could not be
  reached. The CLI gaps found then (no sticker command, no `--track` on
  `title add`, no lane command, no `--duration` on `title template`, the
  glow and contrast notes in `docs/cli.md`) were closed by agent/upkeep.
- **Second pass (agent/qa2):** a template could not be applied *into* an
  open project. Fixed on agent/polish3; since QA pass 3 the "Template cut"
  timeline is the template itself, not its render.
- **Third pass (agent/qa3):** fixed on the way, see `docs/QA.md` "QA pass
  3": misspelt arguments in a batch file were ignored, short clips had their
  faces analysed again on every request, the crop ratio stayed lit after an
  undo.
- **Second pass:** a GIF must keep a transparent colour in its palette
  (`palettegen=reserve_transparent=1`), or ffmpeg writes it opaque and the
  sticker is a box. Not a chukcut bug; the script's recipe does it right.
- Not tried: auto captions (no transcription key; whisper.cpp would download a
  model), cloud TTS and stock (no accounts), voiceover recording (no input on
  Xvfb), playback with sound (Space is not pressed on this machine).
