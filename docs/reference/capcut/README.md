# CapCut desktop — UI reference

What CapCut's desktop editor (Windows, 2026-10) looks like and does, written
down as the spec for chukcut's UI. The screenshots this was taken from live
next to this file on the developer's machine only (`*.png`, gitignored): they
show ByteDance artwork and must never be committed. Recapture them from the
`win10` libvirt VM with `_scratch/vm/vm.sh` and `_scratch/vm/inspector.sh`
(QMP input events and `virsh screenshot`; nothing touches the host desktop).

We copy the *layout and the workflow*, not the look: our own icons, colours
and wording.

## Layout (1920×1080)

```
┌ title bar: logo · Menü · autosave state ·  project name  · layout · Pro · Share · [Export] ┐
├ asset panel (≈650 px) ─────────┬ player (≈650 px) ───────┬ inspector (≈600 px) ─────────┤
│ tab row: Media Audio Text      │ "Player – Timeline 01"   │ nothing selected: project    │
│ Sticker Effects Transitions    │ black canvas             │   details (name, path, colour│
│ Captions Filters Adjust        │ time 00:02:13:29 / total │   space, proxy, ratio, res,  │
│ Templates                      │ play · quality · zoom ·  │   fps) + "Change"            │
│ left: category list            │ ratio · fullscreen       │ clip selected: tabs, below   │
│ right: search + tile grid      │                          │                              │
├ timeline toolbar ──────────────┴──────────────────────────┴──────────────────────────────┤
├ timeline: track headers (lock, eye, mute, …) · ruler · main track with "Cover" button ·   │
│ video clips with title + thumbnail strip · audio clips with title + waveform            │
└───────────────────────────────────────────────────────────────────────────────────────────┘
```

Panels are dark grey (`#1e1e1e`–`#2a2a2a`) on near-black, accent cyan
(`#22d3ee`-ish) for the active tab, selection and primary buttons.

## Asset panel tabs

| Tab | Left column | Content |
|---|---|---|
| Media | Import, Your, Generate, Storage, Library (Featured, Christmas, Greenscreen, Background, Intro/Outro, Transitions, Landscape, Atmosphere, Life) | drop zone "Import — drag videos, photos and audio here"; tile grid with duration badge and download arrow |
| Audio | Import, Your, Music, Sound effects, Copyright | search "songs or artists", list |
| Text, Sticker, Effects, Transitions, Filters, Adjust, Templates | saved / featured / categories | tile grid with preview thumbnail and name |

Effects and transitions are tiles with a looping preview on hover and a "+"
to add; categories down the left.

## Timeline toolbar, left to right

| Button | Shortcut | Notes |
|---|---|---|
| New timeline | | |
| Select / split tool | (dropdown) | arrow = select, blade = split on click |
| Undo | Ctrl+Z | |
| Redo | Ctrl+Shift+Z | |
| Split | Ctrl+B | at the playhead, selected clip or all |
| Delete left | Q | trims the selected clip up to the playhead |
| Delete right | W | trims from the playhead to the clip end |
| Delete | Backspace / Delete | |
| Marker | M | |
| Crop | | |
| Freeze / Reverse / Mirror / Rotate | (dropdown) | |
| Transcript, AI extend, AI edit, split scenes, remove background, auto adjust, optimise graphics | | AI — not planned |

Right side:

| Button | Shortcut |
|---|---|
| Voiceover (record) | |
| Main track magnet | P |
| Auto snapping | N |
| Linkage (A/V link) | ` |
| Preview axis | S |
| Zoom to fit | Shift+Z |
| Zoom out / slider / zoom in | Ctrl+− / Ctrl++ |

## Timeline

- Track headers: track type icon, lock, eye (video only), mute, "…" menu.
  The main video track has a **Cover** button in front of it.
- Video clips: name in a strip at the top, thumbnail filmstrip below,
  selected = white outline. Audio clips: name, waveform filling the clip,
  blue. Clips on the main track are magnetic (no gaps).
- Ruler with `mm:ss` labels every 30 s at the default zoom; the playhead is
  a white line with a handle on the ruler.

## Inspector (clip selected)

Video clip — top tabs **Video · Speed · Animation · Adjust · AI stylise**:

- **Video → Basic**: Transform (Scale slider + %, uniform scale toggle,
  Position X/Y, Rotation, alignment buttons), Blend (mode, opacity),
  Stabilise, Quality, noise reduction, optical flow, … Every property has a
  reset and a **keyframe diamond** at the right.
  Sub-tabs: Basic · Remove background · Mask · Retouch.
- **Video → Mask**: "Add mask" + shape tiles (linear, mirror, circle,
  rectangle, star, heart, text, brush …).
- **Speed**: Standard (speed slider 0.1×–100× with ticks, duration in s,
  "change audio pitch" toggle) · Curve (presets) · Speed effects. "Reset".
- **Animation**: In · Out · Combo, each a tile grid with categories.
- **Adjust**: Basic (auto adjust, colour match, colour correction, LUT with
  intensity, protect skin tones, **Adjust**: temperature, tint, saturation,
  exposure, contrast, highlights, shadows, whites, blacks, brilliance,
  sharpen, clarity, grain, fade, vignette — each slider + number + keyframe)
  · HSL · Curves · Colour wheels · Mask. Footer: "Save as preset",
  "Apply to all".

Audio clip — tabs **Basic · Voice changer · Speed**: volume (dB slider),
fade in, fade out (s), loudness normalise, enhance voice, noise reduction,
isolate voice.

Nothing selected — **Details**: name, path, colour space (Rec.709 SDR),
imported media (keep in place), arrange layers, proxy, timeline name, aspect
ratio, resolution, frame rate, "Change".

## Export dialog

Left: cover preview ("Edit cover"). Right, scrolling:

- Export timeline (name), Name, Export to (path + folder button)
- **Video** (checkbox): remove watermark (Pro), Resolution 480p · 720p ·
  1080p · 2K · 4K, Bitrate Lower · Recommended · Higher · Custom, Codec H.264
  · HEVC · AV1 · RLE (alpha), Format mp4 · mov, Frame rate 24 · 25 · 29.97 ·
  30 · 50 · 59.94 · 60, colour space (read-only)
- **Audio** (checkbox): format
- **Export GIF** (checkbox): resolution
- **Captions** (Pro)
- Copyright check toggle
- Footer: duration and estimated size ("3m 42s | about 220 MB"), Export,
  Cancel.

Pro assets block the export with a list of what is Pro — irrelevant to us.
