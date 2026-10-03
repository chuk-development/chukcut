# What DaVinci Resolve users add on, and what chukcut should build in

Research date: 2026-10-03. Resolve versions in scope: 20, 21.0 and 21.1
(21.1 shipped on 2026-09-08).

The question: which plugins and scripts do Resolve users install because
Resolve lacks something? And which of those capabilities should chukcut build in
natively, for CapCut's audience of short-form creators?

---

## 1. Summary

Resolve is a free, very capable editor. Its users still buy and download add-ons.
The add-ons fall into five groups, and each group points at a gap:

1. **Animation without keyframes.** On Resolve's Edit page, anything that moves
   needs keyframes or a trip into Fusion, which is a node-graph compositor. A
   whole cottage industry sells "drag it on, pick slide/scale/fade, done" tools:
   Magic Animate, Neo TextMotion, MotionVFX, speed-ramp and shake packs. This is
   the biggest group in the three videos the owner sent. CapCut has this built
   in. **chukcut must have it built in.**
2. **Short-form automation.** Captions with word-by-word animation, silence and
   filler-word cutting, and automatic zoom-ins. These come from AutoSubs, Snap
   Captions, FireCut, AutoCut and Recut. Resolve's own transcription is
   Studio-only (paid). Since 21.1, Python scripting is Studio-only too, which
   breaks free scripts such as AutoSubs on the free version.
3. **Looks.** Film emulation (Dehancer, FilmConvert, FilmBox, Cineprint), lens
   emulation (LensNode) and better glows. Resolve's own film-look tools
   (Film Look Creator, halation) are Studio-only.
4. **Repair.** Video noise (Neat Video), audio cleanup (iZotope RX, CrumplePop)
   and flicker (Flicker Free). Resolve's temporal video denoise is Studio-only.
5. **Pro VFX and workflow.** Planar tracking (Mocha Pro), large effect suites
   (Sapphire, Continuum), the Reactor package manager, and bin and folder tools
   (SuperBins, PostHaste). Most of these matter less to short-form creators.

Two more facts matter for a **Linux** editor:

- The free Resolve on Linux does not decode or encode H.264, H.265 or AAC. Phone
  footage (H.264/HEVC + AAC) must be converted before import. chukcut uses
  FFmpeg, so it does not have this gap. This is a strong reason for Linux
  creators to choose chukcut.
- Resolve 21.1 moved Python scripting to Studio. chukcut's planned CLI and MCP
  server (STATUS "Open work" item 4) gives free automation that Resolve now
  charges for.

The recommended built-in list (section 8) has 14 features. The top five:
animated word-by-word captions, silence and filler-word cutting, keyframe-free
animation presets with easing presets, a per-letter/word/line text animator,
and automatic punch-in zooms.

---

## 2. Video editing primer (for the owner)

One line each. These terms appear in the rest of this document.

- **NLE** — non-linear editor: a timeline editor like Resolve, Premiere, CapCut
  or chukcut.
- **Keyframe** — a stored value at a time (for example, scale 1.0 at 0 s and
  1.2 at 1 s). The editor interpolates between keyframes to make motion.
- **Easing** — the shape of the motion between two keyframes. Linear is robotic.
  Ease-out slows into the end. Bounce and elastic overshoot and settle.
- **Animation preset (in / out / combo)** — a stored motion that you apply with
  one click, without keyframes. CapCut's "Animation" tab is this.
- **Jump cut** — a cut that removes a pause inside one continuous shot. The
  talking head "jumps". This is the basic rhythm of short-form video.
- **Punch-in (zoom cut)** — a sudden or eased zoom on the same shot, used to add
  energy or to hide a jump cut.
- **J-cut** — the audio of the next clip starts before its picture (you hear the
  next scene first).
- **L-cut** — the audio of the current clip continues after the picture has cut
  to the next clip.
- **Ripple edit** — trim a clip and move everything after it, so no gap opens.
- **Roll edit** — move a cut point between two clips. One clip gets longer and
  the other gets shorter, and the total length stays the same.
- **Slip edit** — keep a clip's position and length, but change which part of
  the source it shows.
- **Slide edit** — move a clip left or right between its neighbours. The
  neighbours get longer or shorter to fill the space.
- **Nested sequence (compound clip)** — a timeline put inside another timeline
  as one clip, so you can treat a group of clips as one.
- **Adjustment layer (adjustment clip)** — an empty clip on an upper track. Its
  effects apply to everything below it for its duration.
- **Speed ramp** — speed that changes inside one clip (fast, then slow-motion,
  then fast). Short-form "velocity edits" are built on this.
- **Motion blur** — the smear that real cameras record on fast motion. Added
  artificially to make zooms, shakes and speed ramps feel real.
- **LUT** — a lookup table that maps input colours to output colours. Used for
  camera conversions and for "looks".
- **Grading** — creative colour work: contrast, colour balance, looks.
- **Film emulation** — making digital footage look like it was shot on film
  stock: film colour response, grain, halation, bloom and gate weave.
- **Grain** — the random texture of film. Digital grain is a noise overlay that
  matches the film's grain size and response.
- **Halation** — a red-orange glow around bright highlights on film. Light goes
  through the film, reflects off the back, and exposes the red layer again.
- **Bloom** — a soft glow that spreads from bright areas.
- **Gate weave** — the small frame-to-frame wobble of film in a projector or
  camera gate.
- **Denoise (spatial / temporal)** — removing sensor noise. Spatial looks at one
  frame. Temporal compares frames, which keeps more detail but costs more.
- **Point tracking** — follow one small feature to get position (and with two
  points, scale and rotation).
- **Planar tracking** — follow a whole flat surface (a phone screen, a sign, a
  wall) to get its full perspective. You use it to replace a screen or to stick
  a graphic onto a surface.
- **Rotoscoping / masking** — cutting an object out frame by frame, by hand or
  with AI (Resolve's Magic Mask).
- **Fusion** — Resolve's built-in node-based compositor and motion-graphics
  page. Powerful, but slow to learn.
- **Text+** — Resolve's title tool that Fusion powers. Editable on the Edit page,
  animated mostly in Fusion.
- **OFX (OpenFX)** — the open plugin standard that Resolve uses for third-party
  effects.
- **DCTL** — DaVinci Color Transform Language: small GPU colour programs that
  you load into Resolve's colour page.
- **LUFS** — loudness units. Platforms normalise to a target (about −14 LUFS).
  A video that is too quiet sounds weak next to other videos.
- **B-roll** — cutaway footage that plays over the main speaker.

---

## 3. The three videos the owner sent

The `gscrape yt video` command fetched the metadata and the English auto
transcripts. Raw JSON is in `_scratch/` (not committed).

### 3.1 "I Made 4 FREE Plugins for DaVinci Resolve" — NeoEdit

<https://www.youtube.com/watch?v=uThK-1RypxQ> · 10.8k views · 10:48 ·
published 2026-10-01. Chapters: Intro 0:00, Installing 0:57, Neo TextMotion Lite
1:25, Neo Glow Lite 5:22, Neo Zoom Lite 7:27, Neo Light Sweep Lite 9:09.

A free "Starter Pack" of four lite plugins. Each one is the hook for a paid pack
(Animated Text, Motion Essentials, Effects Core) at <https://neoeditfx.com>.
The creator's main pitch: "if you want to animate a title right now in DaVinci
Resolve, you have to go into Fusion. You have to keyframe it manually and do all
the easing by hand for every single title."

- **Neo TextMotion Lite** — a title with built-in in/out animation and no
  keyframes. Controls: animate by letter, word, line or all; order forward or
  reverse; in, out or both; duration; an "exponential power" ease strength;
  slide/fade/scale switches; direction in and direction out, with "mirror exit"
  off; separate move distances for in and out. The paid version adds a curve
  editor, glow/shine/outline on the text, counters and per-character styling.
- **Neo Glow Lite** — a multi-layer glow, compared to Resolve's single-layer
  "Soft Glow". It selects by channel (luminance, red, green or blue) and
  threshold, then has spread, tint colour, tint strength and blend.
- **Neo Zoom Lite** — a zoom-in/zoom-out for talking-head clips and reels. It
  comes as an effect or as a ready "zoom layer" (an adjustment clip). Controls:
  end zoom (for example 1.2), a pivot that you drag to eye level, in and out
  durations, offset, easing curve, and swap start/end.
- **Neo Light Sweep Lite** — a moving band of light across logos or text.
  Intensity, width, soft edge, angle, placement (you keyframe it to sweep),
  blend and edge glow.

All four run on the Edit page. That is the selling point: no Fusion.

### 3.2 "The best DaVinci Resolve plugins in 2026" — Kirk Mihelakos

<https://www.youtube.com/watch?v=D19i63s1NCQ> · 29.3k views · 9:12 ·
published 2026-05-28. A narrative and commercial filmmaker's daily tools.

- **LensNode** (Node Mill / Video Village; $49 lite, $99 per year, $249
  permanent) — emulates vintage lenses on modern footage: Petzval swirl,
  vignette, colour fringe, sensor size, distortion, bloom, coma, bokeh. It has
  profiles for old Nikon, Zeiss, Cooke and Canon lenses. He combines it with
  Magic Mask and depth maps to put the look only on parts of the image.
- **DCTL Tetra** (free, pay-what-you-want) — a hue-shift tool based on
  tetrahedral interpolation. You push one colour towards another (cyan to green,
  orange car to red) across the whole image.
- **Soundly** (free tier and paid) — a sound-effects library app. You search,
  select a region, and drag it into Resolve.
- **Audiio with "Hans 2"** (sponsor) — a music library with an assistant. It
  finds tracks from a text prompt or from an image.
- **SuperBins** — watches a project folder on disk and mirrors its folder
  structure into Resolve bins. New files appear in Resolve automatically.
- **Post Haste** (Digital Rebellion, free) — creates a project folder tree from
  a template (footage, audio, SFX, VFX and so on).
- **FilmBox** (Video Village, about $1,000 perpetual) — high-end film
  emulation, "probably the best film emulation platform ever", plus a cheaper
  "FilmBox Looks".
- His own **film-emulation titles** (text with grain, chromatic aberration and
  texture, so titles do not look like "clean text") and a free keyboard layout.

### 3.3 "Free Plugins That Fix What DaVinci Resolve Can't" — VortexEdixx

<https://www.youtube.com/watch?v=NA26_GET3Wg> · 71.8k views · 10:34 ·
published 2026-09-20. The most-watched of the three, aimed at short-form
editors.

- **Alok's Cam Align** — split-screen layouts for two clips (vertical V2 and
  horizontal V2). Per-camera X/Y, size and angle; split width, height and
  roundness; border width and colour; drop shadow; background colour or
  transparent.
- **Hope's Wave Warp** — a procedural wave distortion (sine, unisphere and so
  on) with speed. Stacked, it makes animated abstract backgrounds.
- **PaperClip Emoji** — a panel inside Resolve with almost every emoji, static
  or animated. One click downloads it into the media pool.
- **Rev Rectangle** (Reveace) — a rectangle mask with per-corner radius, a
  chamfer (cut corners), and a "tail" that makes chat-bubble shapes.
- **Hope's Liquid Glass** — Apple-style "liquid glass": the shape refracts the
  background, with refraction strength, spread, softness and shadow.
- **Rev EaseSpline** (paid, sponsor) — one-click easing presets (bounce,
  elastic, bezier) instead of editing every curve in the spline editor.
- **LM Proximity Nodes** — five nodes that react to the distance of a pointer
  (size, blur, Y offset and so on). Used for UI and app-promo animations where a
  cursor moves over icons.

### 3.4 What the three videos have in common

All three sell **convenience over power**. Resolve can already do every one of
these things with Fusion nodes, keyframes and expressions. People pay or
download because they want a slider and a preset instead of a node graph. Two
of the three creators sell their own packs. The free lite version is the
marketing funnel for a paid pack. The pattern repeats across the ecosystem:
CapCut-style presets, sold to Resolve users.

---

## 4. Why Resolve users install add-ons: the gaps

### 4.1 Free versus Studio

The Studio licence costs $295, one time. These Studio-only items each have an
add-on market around them, because free users replace them with plugins. The
source is Blackmagic's own Studio page unless noted.

| Studio-only in Resolve | What free users install instead |
|---|---|
| Neural Engine AI: Magic Mask, Smart Reframe, Object Removal, Speed Warp, Super Scale, face recognition | Mocha, rotoscoping macros, Topaz |
| AI transcription (subtitles from audio) and animated subtitles (needs the transcription) | AutoSubs, Snap Captions, FireCut, AutoCut |
| Temporal and spatial noise reduction, AI UltraNR | Neat Video |
| Film Look Creator, halation and more of the "45+ additional Resolve FX" | Dehancer, FilmConvert, free grain/halation DCTLs |
| IntelliCut silence removal (Fairlight, Resolve 20+) | Recut, AutoCut, FireCut |
| Scene cut detection | manual cutting |
| Python and Lua scripting, Workflow Integration plugins (Python moved in 21.1) | nothing for free users now |
| Timeline above UHD, 10-bit H.264, multi-GPU | nothing |
| On Linux only: H.264, H.265 and AAC decode and encode | Shutter Encoder or FFmpeg to transcode first |

Note: some reviews of Resolve 21 say more AI tools are free (for example
IntelliTrack and "Voice to Subtitle"). Sources disagree. Blackmagic's own page
still lists Magic Mask and Smart Reframe as Studio features. This document uses
the official list.

### 4.2 The Edit page cannot animate quickly

Resolve's Edit page has keyframes. But text animation, bounce easing, per-letter
animation and procedural motion all live in Fusion. The three videos are
evidence: almost every plugin shown is a Fusion macro wrapped so it runs on the
Edit page with sliders. CapCut solves this with preset tabs (Animation In / Out /
Combo, Text Animation). chukcut's wave 3 "motion" and "text & titles" agents are
the right place for this.

### 4.3 Transitions are limited by Resolve's plugin interface

Film Impact sells popular transitions for Premiere and Final Cut, but has
delayed a Resolve port. Their stated reason, on the Blackmagic forum: Resolve
flattens all the layers of a clip before an OFX transition runs, and Resolve
handles alpha itself and ignores what the transition does to it. chukcut owns
its compositor. Its transition hook already draws each side into its own layer
(STATUS, "Transitions render"). So chukcut can do transitions that a Resolve
plugin cannot.

### 4.4 Linux

The free Linux build cannot read typical phone and camera files (H.264/H.265
with AAC). This is the first problem every Linux creator meets in Resolve.
chukcut reads all of these through FFmpeg and already exports H.264/HEVC and
AAC, with VAAPI and NVENC.

### 4.5 Recurring complaints

From forum threads, comparison articles and the videos:

- "Everything that moves needs Fusion." (videos 3.1 and 3.3; Magic Animate's
  whole pitch.)
- No template and asset library like CapCut's: text templates, stickers, music
  and SFX inside the editor. (Blackmagic forum "Davinci resolve vs Capcut";
  CapCut comparison articles.)
- Captions: free users have no transcription, and the native animated captions
  have five styles applied to a whole track. Animation is lost when the
  captions are exported as a separate file.
- Workspace and list-view gaps that Premiere users miss (Adobe community thread).
- Fairlight: loudness scans that need real-time playback, and project-setting
  changes that reset third-party plugin settings (Blackmagic forum).
- 21.1 scripting change: tools such as Puget Bench and AutoSubs stop working on
  the free version.

---

## 5. The master table

Effort is for chukcut, with what exists today (keyframes with a curve view,
transitions in the compositor, titles with stroke/shadow/box, `.cube` LUTs,
audio fades, the effect runtime):
**S** = one agent session, **M** = one agent wave, **L** = several waves or
model work.
Priority: **1** = build in the coming waves, **2** = after the core is solid,
**3** = later or never.

"Plan" refers to chukcut documents: `build-out` = `docs/plan/build-out.md`
waves, `ml` = `docs/research/ml-features.md` sections, `roadmap` =
`docs/ROADMAP.md`.

### 5.1 Captions and automated cutting

| Plugin | What it adds | Why people want it | Open source? | Effort | Prio | Overlap with chukcut plans |
|---|---|---|---|---|---|---|
| AutoSubs (tmoroney) | Local transcription (Whisper, Parakeet and others), speaker labels, styled subtitles back into the timeline, SRT/ASS/VTT | Free Resolve has no transcription; runs offline; 4.3k GitHub stars | Yes, MIT (word alignment model CC BY-NC) | M | **1** | build-out wave 2 "captions"; ml 3.1 |
| Snap Captions | Converts subtitles into animated Text+ caption clips whose in/out animations reset when you cut | Native animated captions are few and track-wide | No (free download) | M | **1** | captions styles, karaoke highlight |
| FireCut | Silence, filler-word, repetition and profanity removal; auto zooms; animated captions; chapters; B-roll; music | One-click short-form edit; about $34/month | No | M | **1** | ml 3.1, 3.8 |
| AutoCut | Silence and repetition removal, animated subtitles, zooms, reframing for social, chapters, podcast multicam | Same; works in free Resolve | No | M | **1** | ml 3.8, 3.7 |
| Recut | Stand-alone silence cutter, exports an XML cut list | Fast rough cut of talking-head footage; $129 | No | S | **1** | ml 3.8 (Silero VAD + `silencedetect`) |
| Resolve IntelliCut (Studio) | Silence removal, dialogue checkerboarding | Shows that Blackmagic sees the same need | — | — | — | — |
| YouTube chapter scripts | Timeline markers to YouTube chapter text | Long-form YouTube | Yes (several on GitHub) | S | 3 | markers (roadmap phase 2) |

### 5.2 Animation and motion (no keyframes)

| Plugin | What it adds | Why people want it | Open source? | Effort | Prio | Overlap |
|---|---|---|---|---|---|---|
| Magic Animate (MrAlexTech) | Zoom, whip, spin, dissolve, reveal and picture-in-picture animations from presets, with in/out timing and curves | "Animation without keyframes"; free, $29.99 full | No | M | **1** | build-out wave 3 "motion" (animation presets) |
| Neo TextMotion / Animated Text Pack | Per letter, word or line text in/out; direction, distance, ease strength | Title animation in Resolve needs Fusion | No (free lite) | M | **1** | wave 3 "text & titles"; `TextMaterial` |
| Neo Zoom Lite, Magic Animate zooms | Punch-in zoom with a draggable pivot (eye level), ease, in/out, offset | The basic short-form "energy" move | No | S | **1** | wave 3 "effects" (zoom) |
| Speed-ramp preset packs (Speed Ramp Pro and others) | One-click velocity curves, often with motion blur, flash and shake | Velocity edits are a short-form genre | No | M | **1** | wave 3 "motion" (speed curves) |
| Shake presets, Sapphire S_Shake | Camera shake with amplitude, frequency, motion blur | Impact on beats and hits | No | S | **1** | wave 3 "effects" (shake) |
| Rev EaseSpline, MotionPal | Easing presets: bounce, elastic, overshoot, bezier | Editing spline curves by hand is slow | No | S | **1** | keyframe curve view exists; add presets |
| MotionVFX (mTransition, titles, mTracker 3D) | Drag-and-drop transitions and titles; 3D camera tracking that sticks text to walls and floors | Fusion-level graphics on the Edit page; $29/month | No | M (transitions), L (3D tracking) | 2 | transitions exist; ml section 2 (tracking) |
| Film Impact transitions | Premium transitions (not available for Resolve) | Popular in Premiere; Resolve's OFX limits block a port | No | M | 2 | `transitions.md`: each side is its own layer |
| Proximity Nodes (LM) | Scale, blur or offset that reacts to a pointer's distance | App and UI promo animations | No | M | 3 | expressions or links between clips |
| Hope's Wave Warp | Procedural wave distortion, animated backgrounds | Abstract motion backgrounds | No (free) | S | 3 | effect runtime |

### 5.3 Shapes, layouts, stickers and assets

| Plugin | What it adds | Why people want it | Open source? | Effort | Prio | Overlap |
|---|---|---|---|---|---|---|
| Alok's Cam Align | Split-screen and stacked layouts with border, roundness, shadow, background | Reaction videos, podcasts, before/after | No (free) | S | **1** | CapCut "Layout"; masks (roadmap phase 2) |
| Rev Rectangle | Per-corner radius, chamfer, chat-bubble tail | Lower thirds, chat-message graphics | No (free) | S | 2 | masks; text box shape |
| Hope's Liquid Glass | Refractive glass panel over a background | Current design trend (Apple liquid glass) | No (free) | M | 3 | effect runtime |
| PaperClip Emoji | Searchable emoji library, static and animated, into the media pool | Emoji are everywhere in short-form captions | No (free) | S | **1** | captions emoji; `open-assets.md` (research running) |
| Soundly | Search a SFX library and drag a region into the timeline | Sound design without leaving the editor | No (free tier) | S (UI) | 2 | `open-assets.md`; audio wave 3 |
| Audiio, "Hans 2" | Music search by text prompt or image | Finding the right track is slow | No | L | 3 | — |
| Reactor (WeSuckLess) | Package manager for community Fusion tools, macros, scripts and LUTs | One place to install free tools | Community repository | M | 2 | effect runtime: packages fetched from a URL the user gives |
| Krokodove | 70+ Fusion tools for shapes, generators, distortion, text; now built into Resolve 21 | Free motion-design toolbox | Free (was a Reactor package) | L (as a set) | 3 | wave 3 "effects" |

### 5.4 Looks: colour, film and lens

| Plugin | What it adds | Why people want it | Open source? | Effort | Prio | Overlap |
|---|---|---|---|---|---|---|
| Dehancer Pro | Film stocks and print emulation; grain, halation, bloom, gate weave, film damage | "Film look" without grading skill; about $300/year | No | M | 2 | colour wave 2 (LUTs); wave 3 effects |
| FilmConvert Nitrate | 19 film stocks with grain from 6K scans | Easy film look, good skin tones; $199 | No | M | 2 | same |
| FilmBox, Cineprint16 | High-end (about $1,000) and budget ($30) film emulation | Same need at two price points | No | M | 3 | same |
| Free grain and halation DCTLs, Resolve's Film Look Creator (Studio) | Grain, halation, bloom, vignette as separate controls | Film Look Creator and halation are Studio-only | Many DCTLs open (GitHub "dctl" topic) | S | 2 | wave 3 "effects" (vignette exists in the plan) |
| LensNode | Vintage lens character: swirl, vignette, fringe, distortion, bokeh | Expensive lens look without the lenses; $49–249 | No | M | 3 | effect runtime |
| DCTL Tetra | Tetrahedral hue shifts (move one colour towards another) | Quick, clean colour swaps | Free | S | 3 | colour wave 2 (HSL) |
| Neo Glow, Sapphire S_Glow | Multi-layer glow with channel and threshold selection | Resolve's Soft Glow looks flat | No | S | 2 | wave 3 "effects" (glow) |
| Neo Light Sweep | Moving shine across text or logos | Logo stings, title polish | No (free lite) | S | 2 | wave 3 "effects" |
| Lattice, False Color | LUT conversion, exposure heat maps | Professional colourists | No | — | 3 (skip) | — |

### 5.5 Repair: noise, audio, flicker

| Plugin | What it adds | Why people want it | Open source? | Effort | Prio | Overlap |
|---|---|---|---|---|---|---|
| Neat Video | Profiled temporal video denoise, also dust and flicker | Low-light footage; Resolve's denoise is Studio-only; $99.90–179.90 | No | L | 2 | not in ml-features yet (only audio denoise) |
| iZotope RX | Dialogue isolation, de-reverb, de-click, spectral repair | Industry standard; $1,349 | No | M (denoise), L (all) | **1** (denoise) | ml 3.9 (DeepFilterNet); audio wave 3 |
| CrumplePop (Boris FX) | One-knob echo, noise, wind, rustle and pop removal; Levelmatic auto level | For creators with no audio knowledge | No | M | **1** | ml 3.9; audio wave 3 (normalise) |
| Flicker Free | Removes LED banding and timelapse flicker | LED lights and phone footage | No | M | 3 | — |

### 5.6 Tracking and VFX suites

| Plugin | What it adds | Why people want it | Open source? | Effort | Prio | Overlap |
|---|---|---|---|---|---|---|
| Mocha Pro | Planar tracking, screen replacement, object removal, AI roto | Tracks surfaces that point trackers lose; $1,095 | No | L | 2 | ml section 2, step T4 (planar) |
| Boris FX Sapphire | 250+ GPU effects (glow, flares, shake, glitch, transitions) | The standard effects suite; $195–2,795 | No | L (as a suite) | 2 (pick the top 10) | wave 3 "effects" |
| Boris FX Continuum | Effects, transitions, particles, lens correction | Broadcast work; $395/year to $1,995 | No | L | 3 | same |
| Resolve Smart Reframe, Magic Mask (Studio) | AI 16:9 to 9:16 crop; AI subject mask | Repurpose long video into shorts; cut-outs | — | M | 2 | ml 3.7, 3.4 |

### 5.7 Workflow and scripting

| Plugin | What it adds | Why people want it | Open source? | Effort | Prio | Overlap |
|---|---|---|---|---|---|---|
| Resolve scripting API community scripts | Batch renders, marker exports, timeline automation, AI assistants | Automation; now Studio-only for Python (21.1) | Many on GitHub | M | 2 | STATUS open work 4: CLI and MCP server |
| SuperBins | Watch folder that mirrors disk folders into bins | No manual import of new files | No | S | 3 | media library |
| Post Haste | Project folder tree from a template | Organised projects | No (free) | S | 3 | project templates (roadmap phase 4) |

---

## 6. The top items, in more detail

### 6.1 Animated captions (AutoSubs, Snap Captions, FireCut)

**What a creator gets.** Speech becomes text clips on the timeline, timed per
word. One click applies a style: bold font, stroke, the current word coloured or
scaled ("karaoke"), emoji next to key words, pop-in per word or per phrase.

**Why it matters.** Most short-form video is watched without sound. Captions are
the main hook. In Resolve the free user has no transcription. The paid user gets
five track-wide styles. This is why free and paid caption plugins sell.

**For chukcut.** The captions agent (build-out wave 2) covers transcription,
word/sentence modes, SRT/VTT, a caption lane, styles and karaoke highlight.
Two lessons from the plugins to add:

- **Animations must survive edits.** Snap Captions' selling point is that the
  in/out animation resets when you cut or trim a caption. Store the animation
  relative to each caption's own start and end, not as absolute keyframes.
- **Style presets are the product.** FireCut sells names ("Hormozi", "pop up",
  "box pop up"). Ship 10–20 named presets on day one. Let users save their own.

### 6.2 Silence and filler-word cutting (Recut, AutoCut, FireCut)

**What a creator gets.** A talking-head recording with all pauses removed
("jump cuts"), with a threshold and padding. Optionally "um", "uh" and repeated
takes are removed too.

**Why it matters.** It is the most time-consuming part of editing a talking-head
video. Blackmagic added IntelliCut to Studio, which confirms the need.

**For chukcut.** `ml-features.md` 3.8 already plans Silero VAD plus FFmpeg
`silencedetect` (size S). Filler words reuse the caption transcript (M). Make
the result a **reviewable list of cuts** in one undo step (the existing
`compose_edits` batch path). Do not hide it in a destructive one-click action.
Every plugin does a preview because a wrong cut cuts a word in half.

### 6.3 Keyframe-free animation presets (Magic Animate, Neo, MotionVFX)

**What a creator gets.** Drag "Zoom in", "Whip left", "Spin", "Slide up",
"Bounce" onto a clip or text. Change duration and strength with sliders.

**Why it matters.** This is the largest group of add-ons in all three videos.
CapCut users expect it, because CapCut's Animation tab is exactly this.

**For chukcut.** Wave 3 "motion" plans "animation presets (in/out/combo) via
keyframes". Two design points from the plugins:

- **A preset is a parameter, not baked keyframes.** Magic Animate and Neo Zoom
  let you change the duration after you apply the preset. If chukcut writes
  keyframes, a trim breaks the animation. Store `{preset, in_duration,
  out_duration, strength, easing}` on the clip and evaluate it in the
  compositor, like CapCut's own animation materials.
- **Pivot and offset matter.** Neo Zoom's draggable pivot at eye level is the
  difference between a good and a bad punch-in.

### 6.4 Text animator by letter, word or line (Neo TextMotion)

**What a creator gets.** The text flies or fades in one letter (or word, or
line) at a time, with stagger, direction and easing, and exits the same way.

**Why it matters.** Titles and hooks in the first second of a short. In Resolve
it needs Fusion's Follower modifier.

**For chukcut.** The text renderer (`docs/research/text-rendering.md`, titles end
to end) already lays out glyphs. A text animator needs a per-glyph transform and
opacity evaluated at render time, with unit = letter, word or line, stagger,
order and an in/out curve. Size M. Neo's control list (section 3.1) is a good
minimum specification.

### 6.5 Auto punch-in zooms (FireCut, AutoCut, Neo Zoom)

**What a creator gets.** Zooms that the editor places automatically, on emphasis
or after each jump cut, centred on the face.

**For chukcut.** Step 1 (S): a zoom preset with a pivot (section 6.3). Step 2
(M): an "auto zoom" pass after silence cutting that alternates 1.0 and 1.15–1.2
across cuts. Step 3: centre the pivot on a face detector (ml 3.7 auto-reframe
uses the same detector).

### 6.6 Voice cleanup and loudness (iZotope RX, CrumplePop)

**What a creator gets.** One "Enhance voice" switch: noise, echo and wind
removed, level made even, and the export loud enough for the platform.

**For chukcut.** `ml-features.md` 3.9 (DeepFilterNet, size S) plus wave 3
"audio" (normalise). Add a **loudness target on export** (−14 LUFS for social),
measured offline with EBU R128 (FFmpeg `ebur128`/`loudnorm`). This avoids the
Fairlight complaint that a loudness scan needs real-time playback.

### 6.7 Short-form effect pack (Neo Glow, Light Sweep, shake, Sapphire)

**What a creator gets.** Glow with a threshold, shake, light sweep, RGB split,
glitch, blur, vignette, and motion blur that follows the motion.

**For chukcut.** Wave 3 "effects" already lists blur, glow, shake, zoom, glitch,
RGB split and vignette. Add from this research: **threshold and channel select
on glow** (Neo's improvement over Resolve's Soft Glow), **light sweep**, and
**motion blur** for zooms, shakes and speed ramps. Do not try to match
Sapphire's 250 effects. Ten good ones with presets cover short-form use.

### 6.8 Layouts and shapes (Cam Align, Rev Rectangle)

**What a creator gets.** Split-screen, top/bottom and picture-in-picture
templates with rounded corners, borders and shadows. Shapes with per-corner
radius and chat-bubble tails.

**For chukcut.** Small (S each) and very visible. A layout is a preset that sets
the transform and a rounded-rectangle mask on two or three clips. Rounded masks,
border and shadow on a clip also serve picture-in-picture facecam, which Magic
Animate sells as a preset.

### 6.9 Transitions that can see both clips (Film Impact, MotionVFX)

**What a creator gets.** Zoom, whip, spin, glitch, light-leak and "seamless"
transitions that move both clips together.

**For chukcut.** The transition hook already draws each side into its own layer.
Seamless transitions (zoom through, spin through, whip with motion blur) need
transforms per side plus a blur pass. This is a place where chukcut can do more
than a Resolve plugin can (section 4.3).

### 6.10 Film look (Dehancer, FilmConvert, halation DCTLs)

**What a creator gets.** A film-stock preset, plus separate grain, halation,
bloom and vignette sliders.

**For chukcut.** The `.cube` LUT support exists. A "Film" effect group of four
shaders (grain, halation, bloom, gate weave) plus a few LUT looks is S to M.
Resolve gates this behind Studio, so it is a visible free feature. Film-stock
accuracy (Dehancer and FilmBox) is a deep colour-science project. It is not
worth it for short-form.

### 6.11 Planar tracking (Mocha Pro)

**What a creator gets.** Put a screenshot onto a moving phone screen. Stick a
logo to a wall in a moving shot.

**For chukcut.** Already step T4 of the tracking plan in `ml-features.md`
section 2. Keep it after point tracking and the "follow" link. It is L.

### 6.12 Video denoise (Neat Video)

**What a creator gets.** Clean low-light footage without smearing.

**For chukcut.** Not covered by `ml-features.md` (only audio denoise is). Phone
footage at night is common in short-form, so it has some value. A temporal
denoise needs motion compensation, which is L. A first step could be FFmpeg
`hqdn3d` or `nlmeans` as a baked, cached pass (S). Priority 2.

---

## 7. What chukcut should not build

- **A Fusion-style node graph.** The three videos show that people install
  plugins to *avoid* nodes. Presets and sliders win for this audience.
- **OFX plugin hosting.** OFX is C++, mostly Windows/macOS binaries, and the
  short-form plugins in this research are Fusion macros, not OFX. chukcut
  already has its own effect runtime and package loader (roadmap phase 3). A
  simple, documented preset/effect format that the community can share (a
  "Reactor" for chukcut) is worth more.
- **Professional finishing:** Dolby Vision, IMF, DCP, remote grading, Lattice-
  class LUT tools, false colour. These are Studio features for a different
  customer.
- **A 250-effect suite.** Pick the ten effects that short-form creators use.

---

## 8. Recommended built-in features, ordered by value for short-form creators

| # | Feature | Replaces | Effort | Where it goes in chukcut |
|---|---|---|---|---|
| 1 | Auto captions with word timing and **animated style presets** (karaoke highlight, pop per word, emoji); animations that survive cuts | AutoSubs, Snap Captions, FireCut | M | wave 2 "captions"; ml 3.1 |
| 2 | **Silence and filler-word cutting** with a reviewable cut list, one undo step | Recut, AutoCut, FireCut, IntelliCut | S (silence) + M (fillers) | ml 3.8 |
| 3 | **Keyframe-free animation presets** (in/out/combo) for clips and text, stored as parameters, with **easing presets** (bounce, elastic, overshoot) | Magic Animate, MotionVFX, EaseSpline | M | wave 3 "motion"; keyframe curve view |
| 4 | **Text animator** by letter, word or line with stagger, direction and curve | Neo TextMotion, Animated Text Pack | M | wave 3 "text & titles" |
| 5 | **Punch-in zoom** with pivot, then **auto zoom** across jump cuts | Neo Zoom, FireCut, AutoCut zooms | S → M | wave 3 "effects"; ml 3.7 face detector |
| 6 | **Voice enhance** (denoise, then de-reverb) and **loudness target on export** (−14 LUFS) | iZotope RX, CrumplePop, Levelmatic | S → M | ml 3.9; wave 3 "audio" |
| 7 | **Speed ramp presets** with **motion blur** | Speed-ramp packs, velocity-edit packs | M | wave 3 "motion" (speed curves) |
| 8 | **Short-form effect pack**: glow with threshold, shake, light sweep, RGB split, glitch, blur, vignette | Neo Glow, Light Sweep, Sapphire basics | M | wave 3 "effects" |
| 9 | **Layouts and shapes**: split-screen, PiP with rounded corners, border, shadow; per-corner radius masks | Cam Align, Rev Rectangle, Magic Animate PiP | S | roadmap phase 2 "masks" |
| 10 | **Seamless transitions** (zoom, spin, whip through, with blur) using the per-side layers | MotionVFX mTransition, Film Impact | M | transitions module |
| 11 | **Motion tracking**: attach text and stickers to a moving object; later planar | Mocha, mTracker | L | ml section 2 (already priority 1 there) |
| 12 | **Emoji, sticker and SFX libraries** inside the editor, searchable, with clear licences | PaperClip Emoji, Soundly | S (UI) + assets | `open-assets.md` research |
| 13 | **Film look group**: grain, halation, bloom, gate weave, plus LUT looks | Dehancer, FilmConvert, Film Look Creator | S → M | wave 3 "effects"; colour wave 2 |
| 14 | **Auto reframe** 16:9 to 9:16 | Smart Reframe (Studio), AutoCut resize | M | ml 3.7 |

Outside this list, but a clear advantage for the project itself: **free
scripting** through the planned CLI and MCP server. Resolve 21.1 took Python
away from free users.

---

## 9. Sources

The three videos (metadata and transcripts via `gscrape yt video`):

- NeoEdit, "I Made 4 FREE Plugins for DaVinci Resolve" —
  <https://www.youtube.com/watch?v=uThK-1RypxQ>; packs at <https://neoeditfx.com>
- Kirk Mihelakos, "The best Davinci Resolve plugins in 2026" —
  <https://www.youtube.com/watch?v=D19i63s1NCQ>
- VortexEdixx, "Free Plugins That Fix What DaVinci Resolve Can't" —
  <https://www.youtube.com/watch?v=NA26_GET3Wg>; plugin links:
  <https://mahatoalok.com.np/product.html?id=CamPlugin>,
  <https://hopeedits.com/plugin-cms/hope-s-wave-warp>,
  <https://payhip.com/b/hiVN7>, <https://reveace.gumroad.com/l/revshape>,
  <https://hopeedits.com/plugin-cms/hope-s-liquid-glass>,
  <https://lmfx-shop.fourthwall.com/en-inr/products/lm-proximity-nodes>,
  <https://reveace.io>

Resolve itself:

- Blackmagic, DaVinci Resolve Studio feature list —
  <https://www.blackmagicdesign.com/products/davinciresolve/studio>
- CineD, free vs Studio comparison —
  <https://www.cined.com/davinci-resolve-an-in-depth-comparison-between-the-free-and-studio-version/>
- ProVideo Coalition, "DaVinci Resolve 21.1 is a huge update" —
  <https://www.provideocoalition.com/davinci-resolve-21-1-is-a-huge-update/>
- CineD, 21.1: Python scripting moves to Studio —
  <https://www.cined.com/es/davinci-resolve-21-1-released-ai-assistant-integration-via-mcp-individual-hdr-trims-and-python-scripting-moves-to-studio/>
- Puget Systems, 21.1 scripting changes —
  <https://www.pugetsystems.com/blog/2026/09/10/how-davinci-resolve-free-v21-1-scripting-changes-affect-puget-bench/>
- Kunal Ganglani, Resolve 21 AI features (claims more free AI than the official
  page) — <https://www.kunalganglani.com/blog/resolve-studio-21-ai-features>
- CineD, Resolve 20 AI features (IntelliCut, animated subtitles) —
  <https://cined.com/davinci-resolve-20-released-with-handful-of-ai-assisted-features>
- Larry Jordan, animated captions in Resolve 20 —
  <https://larryjordan.com/articles/create-animated-captions-using-davinci-resolve-20/>
- FireCut, auto captions in Resolve (transcription is Studio-only) —
  <https://firecut.ai/blog/how-to-use-auto-captions-in-davinci-resolve-a-complete-guide/>
- Neat Video blog, noise reduction in Resolve —
  <https://www.neatvideo.com/blog/post/noise-reduction-davinci-resolve>
- Mixing Light, Film Look Creator —
  <https://mixinglight.com/color-grading-tutorials/film-look-creator-resolvefx-overview-workflow>
- ProVideo Coalition, AAC on Linux, Kdenlive vs Resolve Studio —
  <https://www.provideocoalition.com/aac-audio-kdenlive-beats-davinci-resolve-studio-on-linux/>
- The Geek Page, Resolve 21 no audio on Linux —
  <https://thegeekpage.com/davinci-resolve-21-no-audio-fix-the-aac-codec-bug-on-linux/>
- Blackmagic forum, Fairlight loudness scan —
  <https://forum.blackmagicdesign.com/viewtopic.php?p=655776>; plugin settings
  reset — <https://forum.blackmagicdesign.com/viewtopic.php?p=756074>
- Blackmagic forum, Film Impact on porting transitions to Resolve (page refused
  automated fetch; content from search summaries) —
  <https://forum.blackmagicdesign.com/viewtopic.php?p=423793>
- Blackmagic forum, "Davinci resolve vs Capcut" —
  <https://forum.blackmagicdesign.com/viewtopic.php?p=1144280>
- Adobe community, "What is in Premiere Pro that doesn't exist in DaVinci
  Resolve" —
  <https://community.adobe.com/questions-105/what-is-in-premiere-pro-that-doesn-t-exist-in-davinci-resolve-1628256>

Plugins:

- AutoSubs — <https://github.com/tmoroney/auto-subs>
- Snap Captions — <https://forum.blackmagicdesign.com/viewtopic.php?p=1072500>,
  <https://mediable.notion.site/Snap-Captions-HUB-4e74a79db14748f098be647e40664da8>
- FireCut — <https://firecut.ai>,
  <https://firecut.ai/blog/top-10-davinci-resolve-plugins-for-2026/>
- AutoCut — <https://www.autocut.com/blogs/autocut-davinciresolve>
- Recut — <https://getrecut.com>
- Cutback overview of 2026 plugins —
  <https://cutback.video/blog/best-davinci-resolve-plugins-for-video-editors-2026>
- Magic Animate V3 — <https://www.cined.com/magic-animate-v3-released-animate-without-keyframes-in-davinci-resolve/>
- MotionPal (easing) — <https://davidkohen.gumroad.com/l/MotionPal>
- Speed-ramp and shake packs —
  <https://creativevideotips.com/tutorials/speed-ramp-with-motion-blur-in-davinci-resolve>,
  <https://borisfx.com/blog/how-to-add-shake-effect-in-davinci-resolve-guide/>
- Reactor 2.0 — <https://www.cgchannel.com/2018/05/we-suck-less-ships-reactor-2-0/>
- Krokodove in Resolve 21 —
  <https://digitalproduction.com/2026/07/10/krokodove-a-massive-expansion-of-fusion-and-resolves-creative-arsenal/>
- LensNode — <https://petapixel.com/2025/07/11/davinci-resolve-plugin-emulates-the-look-of-expensive-vintage-lenses>,
  <https://www.newsshooter.com/2025/09/26/node-mill-lensnode-currently-20-off/>
- DCTL Tetra — <https://colorculture.org/color-shift-tetra-dctl-davinci-resolve/>
- Free DCTLs on GitHub — <https://github.com/topics/dctl>
- Dehancer and FilmConvert compared —
  <https://www.provideocoalition.com/review-dehancer-film-emulation-plugin/>,
  <https://bityclips.com/compare/filmconvert-vs-dehancer>
- Halation in Resolve vs Dehancer —
  <https://dvresolve.com/tutorial/resolves-halation-ofx-dehancer-comparison/>
- Boris FX Sapphire pricing and S_Glow —
  <https://www.provideocoalition.com/boris-fx-offers-sapphire-lower-price/>
- Mocha Pro 2026 — <https://www.cgchannel.com/2025/12/boris-fx-releases-mocha-pro-2026/>,
  <https://www.motionmedia.com/mocha-pro-multi-host-perpetual/>;
  planar tracking in Resolve —
  <https://borisfx.com/blog/how-to-use-planar-tracker-in-davinci-resolve/>
- CrumplePop — <https://www.provideocoalition.com/review-crumplepop-2025/>
- Flicker Free 3.0 —
  <https://www.redsharknews.com/digital-anarchy-boosts-flicker-frees-capabilities-with-v3.0>
- YouTube chapters from markers — <https://github.com/matteotrizza/DaVinciYTChaptersGenerator>,
  <https://github.com/oliwiergesla/editorscripts>
- Kirk Mihelakos's plugin links: SuperBins <https://switchtake.com/superbins>,
  Post Haste <https://www.digitalrebellion.com/posthaste/>,
  FilmBox <https://videovillage.com/filmbox/>, Soundly
  <https://getsoundly.com/tools/>
