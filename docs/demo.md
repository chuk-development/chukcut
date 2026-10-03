# The showcase project and a tour of the app

This page shows chukcut with one project that uses most of its features. A
script builds the project from generated media, with `chukcut-cli` only. Then
the page goes through the app area by area: what each area does and how to
open it.

Everything here was seen working on 2026-10-04 (release build, RTX 3060 for
the CLI, the app on a private Xvfb display with lavapipe). The last section
lists what did not work.

## Make the showcase

```bash
memguard-allow 16G cargo build --release -p chukcut -p chukcut-cli -j 6
scripts/demo.sh --export          # media + project + export, about 1.5 min
./target/release/chukcut _scratch/demo/showcase.chukcut
```

`scripts/demo.sh` writes everything to `_scratch/demo/` (ignored by git):

| Path | What |
|---|---|
| `media/` | the generated media (below) |
| `showcase.chukcut` | the project |
| `showcase.mp4`, `showcase.srt` | the export and its caption sidecar (with `--export`) |
| `xdg/` | the CLI's own `HOME` and XDG folders, so the script does not change your settings |

Options: `--media` makes the media again, `--export` also exports.
`CHUKCUT_CLI=/path/to/chukcut-cli` uses another binary. The script needs
`ffmpeg` (with the `flite` filter for the voice), `python3`, `jq` and `bc`.

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
| `04-ball.mp4` | a red ball on a figure-of-eight path over a dimmed `testsrc2` | motion tracking |
| `05-greenscreen.mp4` | a rotating test card on green | chroma key and a mask |
| `06-life.mp4` | Conway's `life` | picture in picture |
| `voice.wav` | `flite` speech synthesis, -16 LUFS | voiceover |
| `music.wav` | `aevalsrc`: a kick at 120 BPM over a chord | music bed |
| `sticker.png` | `geq`: a smiley with a transparent background | sticker |
| `teal-orange.cube` | a 17-point LUT the script writes | the LUT |
| `voice.srt` | written by the script, times measured with `silencedetect` | captions |

### What the project contains

The result is 19.7 s on 8 lanes (`chukcut-cli info _scratch/demo/showcase.chukcut`):

| Time | Lane | What | CLI command |
|---|---|---|---|
| 0 – 4 s | Video 1 | gradients with a **glow** effect | `trim --ripple`, `effect add glow --clip` |
| 4 – 8.5 s | Video 1 | fractal: **grade** (exposure, contrast, saturation, temperature, vignette), the **LUT** at 80 %, an S **curve**, **film grain**, a **punch-in zoom** | `grade --set … --lut`, `curve`, `effect add film_grain`, `zoom` |
| 8.5 – 13.7 s | Video 1 | 3 s of the pattern on the **bullet speed ramp** | `speed-curve --preset bullet` |
| 13.7 – 19.7 s | Video 1 | the ball clip | |
| cuts | Video 1 | **transitions**: `gl:crosswarp`, `seamless:zoom_in`, `dissolve` | `transition add --kind` |
| 4.5 – 8 s | Video 3 | **picture in picture** top left (round corners, border, shadow) with **blend mode** Screen | `layout pip`, `blend screen` |
| 6 – 9 s | Video 2 | **sticker** (the PNG), In animation *pop*, Combo *wobble*, rotation **keyframes** | `place`, `set`, `animate`, `keyframe` |
| 9 – 13.2 s | Video 2 | green screen with **chroma key** and a rounded **rectangle mask** that opens from 20 % to 90 % (mask keyframes) | `chroma-key`, `mask --add rectangle --at` |
| 0.2 – 2.1 s | Text 2 | **title template** *pop-headline* "chukcut" | `title template` |
| 2.1 – 4 s | Text 2 | styled title, **text animator** word by word (*fade_up*), Out animation *fade* | `title add`, `animate-text`, `animate --slot out` |
| 13.7 – 19.7 s | Text 2 | "tracked" label that **follows the ball** (KLT tracker, 180 frames) | `track --overlay` |
| 13.5 – 14.1 s | Effects 1 | **shake** effect clip over the cut into the ball | `effect add shake --at` |
| 0.7 – 9.9 s | Captions | five **captions** from the SRT, *karaoke* style, the spoken word in yellow | `captions import --preset karaoke`, `captions style` |
| 0.5 – 10.5 s | Audio 1 | voiceover: **normalised** to -14 LUFS, **EQ** | `normalize`, `audio-effect add eq3` |
| 0 – 19.7 s | Audio 2 | music: **ducked** 10 dB under the voice (6 volume keyframes), **reverb** | `duck`, `audio-effect add reverb` |
| ruler | | four **markers**: Intro, Grade + LUT, Speed ramp, Tracking | `marker add` |
| export | | own **export preset** "Showcase" (TikTok, CRF 19) | `preset save`, `export --preset user_showcase --sidecar srt` |

The export: 591 frames (19.7 s at 30 fps), H.264 1080x1920 and AAC,
-14.3 LUFS integrated, 25.9 MB. On this machine the software encoder took
58 s, and `--hardware auto` (NVENC) 18 s. Frames from the export match
`render-frame` and the app's player.

## A tour of the app

### Start screen

Starts when the app opens without a file. **New project** with a canvas
(9:16, 16:9, 1:1, 4:5), a name and a frame rate; a click on a canvas or a
frame rate is kept even when the first clip has another shape. **Open
project…** and the **Projects** grid (recent projects with a thumbnail, age,
length and path). **Shortcuts** and **Settings** at the bottom left.

### Title bar

The app name, **Menu** (New project, Open, Save, Save as, Import media,
Export, Settings, Keyboard shortcuts, Quit, each with its shortcut), the save
state ("Saved"), the project name with its canvas and frame rate, and
**Export** at the right.

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
  Symbols, Flags, Icons, in three looks (Fluent 3D, Flat, Noto). The images
  are downloaded on first use, so the tiles stay empty for a few seconds.
- **Captions**: Auto captions (a cloud account or whisper.cpp on this
  machine; word or sentence captions, lines, length, auto emoji), Captions,
  Style, Import & export (SRT, VTT), Translate.
- **Effects**: Blur, Light, Motion, Retro, Distort, Film, Layout. A click puts
  the effect on the selected clip; a drag onto the timeline makes an effect
  clip.
- **Transitions**: Basic (dissolve, dip to colour, wipe, slide, zoom, blur),
  Seamless, Library (the gl-transitions). A click puts the transition on the
  cut after the selected clip.
- **Filters**: our own LUT looks (Cinematic, Film, …). A click grades the
  selected clip.
- **Stock**: Pexels, Pixabay and Freesound search, after an account with
  your own key is added in Settings → Accounts.

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
  stabilise, scene detection, auto reframe; Remove background: chroma key with
  colour picker, intensity, softness, spill, edge shrink, show matte; Mask:
  linear, mirror, circle, rectangle, star, heart, each with add/subtract/
  intersect, invert and keyframable values; Retouch), *Audio* when the file
  has sound, *Speed* (Standard, Curve, Speed effects; change audio pitch),
  *Animation* (In, Out, Combo, Zoom), *Adjust* (Basic: LUT and adjust
  controls; HSL; Curves; Colour wheels; Mask; Save as preset, Apply to all),
  *Effects* (the clip's effect stack with each parameter, reset and keyframe
  buttons).
- **Title or caption**: *Text* (words, font, size, bold/italic/underline,
  alignment, letter and line spacing, fill, outline, shadow, box), *Video*,
  *Animation*, *Tracking* (a follower: follow mode, smoothing, re-track from
  here, bake to keyframes, stop following, remove track), *Effects*.
- **Audio clip**: *Basic* (volume, fades, normalise loudness with the measured
  before → after, reduce noise, remove silences with Review pauses and Filler
  words, equalizer, …), *Voice changer*, *Speed*.

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
proxies on disk, cache size and limit), and below that the cloud accounts
and the hardware report.

### CLI and MCP

`chukcut-cli` does every edit in this page from a shell (`docs/cli.md`).
`chukcut-cli mcp` serves the same operations to an MCP client: 89 tools,
including `view_frame`, which returns the frame as a PNG
(`claude mcp add chukcut -- /path/to/chukcut-cli mcp`).

## What does not work, or is missing

Found while building the showcase. The open ones are also in `docs/QA.md`.

- **Fixed here:** Shift+wheel did not scroll the timeline's lanes on X11, so
  lanes below the panel (the music lane of the showcase) could not be reached.
- The CLI has no sticker command; the engine's sticker library
  (`library_sticker_*`) is only in the app. The showcase uses its own PNG.
- `title add` has no `--track`, and no command adds a lane, so two titles
  cannot overlap in time; `title template` has no `--duration`.
- `docs/cli.md` shows `effect add … glow --set intensity=0.8`; the range is
  0 to 100. The grade's `contrast` rests at 1 (like `saturation`), which the
  control list does not say.
- Not tried: auto captions (no transcription key; whisper.cpp would download a
  model), cloud TTS and stock (no accounts), voiceover recording (no input on
  Xvfb), playback with sound (Space is not pressed on this machine).
