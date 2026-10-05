# Gap analysis, October 2026

What does a real user still miss in chukcut? This document compares chukcut
with CapCut desktop, DaVinci Resolve, Premiere, Kdenlive and Shotcut. It
ranks the gaps by value. It proposes the next three waves.

Date: 2026-10-05. Base: `master` at `bf5f36b` (waves 1 to 11 merged).

## How this was made

- **Our side.** I read `docs/STATUS.md`, `docs/plan/build-out.md`,
  `docs/manual/`, the earlier research files and the CapCut screenshots in
  `docs/reference/capcut/`. Then I searched `crates/` for each candidate gap.
  Each "missing" claim below names the file or the search that proves it.
- **Their side.** Official release notes and help pages: capcut.com help and
  resource pages, blackmagicdesign.com (Resolve 20 press release, Resolve 21
  "What's new"), helpx.adobe.com (Premiere "What's new" 25.x to 26.5),
  kdenlive.org release notes (25.04 to 26.08), shotcut.org blog (25.07 to
  26.9). Versions on 2026-10-05: Resolve 21, Premiere 26.5, Kdenlive 26.08.1,
  Shotcut 26.9.27.
- **The users.** Hacker News threads (OpenCut, "State of Kdenlive", Resolve
  21.1), discuss.kde.org, forum.shotcut.org, Lemmy/PieFed and press articles
  about the 2025 CapCut terms and price change. Reddit was not readable with
  the tools that we had, so the Reddit signal is only indirect (through press
  articles). The sources are at the end.

Two limits apply. CapCut has no public changelog, so some of its "Pro" flags
come from marketing pages. The claim "free Resolve on Linux decodes no
H.264/H.265 and no edition decodes AAC on Linux" comes from community
sources and HN, not from a Blackmagic page.

## What users ask for, in short

These are the requests that came up most often (rough count of distinct
threads or articles):

1. Stability: no crashes and no lost projects (about 7 threads).
2. Free, local auto captions (about 7). CapCut moved them behind Pro in 2025.
3. Phone footage that "just works" on Linux: H.264, HEVC, AAC, variable
   frame rate (VFR), rotation flags (about 8 for Resolve codecs, about 5 for
   VFR drift).
4. A beginner-first workflow: open a clip and edit, no project setup (about 5).
5. Animated, styled captions with word-by-word highlight (about 5).
6. GPU speed for preview and effects (about 5).
7. Good titles and animated text presets (about 5).
8. One-click effects, transitions and templates (about 3 each).
9. Background removal without a green screen (about 3).
10. Privacy and no subscription (about 4, mostly press after the CapCut
    terms change in 2025).

chukcut already answers 1 to 4, 6 and 9 well: autosave and crash recovery,
local whisper.cpp captions, FFmpeg decode with VFR by timestamp
(`crates/engine/tests/decode.rs`, "a variable frame rate file returns the
frame its own timestamps name") and rotation, VAAPI/NVDEC, local RVM/BiRefNet
mattes. The largest open demands are **5 (animated captions)**, **7 (text)**
and **8 (one-click assets)**. Long video to shorts and text-based editing
also appear, mostly as "I use a separate CLI tool for this".

## The baseline in 2026

Three or more of the five competitors have these. chukcut status in the
right column.

| Baseline feature | Who has it | chukcut |
|---|---|---|
| Speech-to-text captions | all five | yes (local and cloud) |
| Word-by-word animated captions | CapCut, Resolve (Studio), Premiere 26.3 | **weak**: one highlight colour |
| Keyframes with easing | all five | yes, but only transform, opacity, volume, masks, effects |
| Proxies, hardware decode and encode | all five | yes |
| Multicam with audio sync | CapCut, Resolve, Premiere, Kdenlive | **no** |
| AI object mask (SAM-style) | CapCut, Resolve, Premiere 26.0, Kdenlive | yes (MobileSAM) |
| Speed ramps, optical flow | CapCut, Resolve, Premiere | yes (RIFE) |
| Vertical workflow, safe areas, auto reframe | all five | reframe yes; **no safe-area overlay** |
| Audio cleanup, loudness, ducking | all five | yes |
| Text to speech | CapCut, Resolve 21, Shotcut 25.10 | cloud only (own key) |
| Beat detection | CapCut, Resolve, Premiere | yes |
| Text-based editing | CapCut (Pro), Resolve (Studio), Premiere | **no** (silence and filler cut only) |
| Video scopes | Resolve, Premiere, Kdenlive, Shotcut | **no** |
| 10-bit / HDR | Resolve Studio, Premiere, Kdenlive, Shotcut 26.6 | **no** |
| Direct upload to social sites | CapCut, Resolve 21, Premiere | no |
| Asset packs (effects, stickers, SFX, templates) | CapCut, Resolve 21, Premiere 25.5, Shotcut 26.8 | small (20 effects, 11 templates) |

## 1. Missing or weak features, ranked

Rank = value to a short-form or YouTube creator, divided by effort.
Effort: **S** = part of one agent's wave, **M** = one agent for one wave,
**L** = more than one wave.

| # | Feature | Who has it | Why users want it | Effort | Risk / licence | chukcut owner | Evidence in code |
|---|---|---|---|---|---|---|---|
| 1 | **HDR phone footage shown correctly** (HLG/PQ to SDR tone map, BT.2020 to BT.709 gamut) | Resolve, Premiere, Shotcut 26.6, CapCut | iPhones and many Android phones record HDR HEVC by default. Without a tone map the clip looks grey and flat. | M | none | `render/source.rs`, `render/shaders/yuv.wgsl`, `media/decoder.rs` (probe) | `YuvMatrix` has `Bt2020` but no code reads the transfer function; no match for `tonemap`, `smpte2084`, `arib`, `HLG` in `crates/` |
| 2 | **Correct colour in every export** (BT.709 matrix and colour tags) | all | Saturated colours shift a little in every export today. Users see "the export looks different". | S | none | `export/encoder.rs`, `render/shaders/yuv.wgsl` | `encoder.rs` module doc: swscale uses BT.601 and the stream is untagged; `compositor.rs` test note: `yuv.wgsl` is "BT.601 limited"; only `set_color_range` is called |
| 3 | **Move, scale and rotate a clip on the player** (click to select, handles, snap guides) | all five (Kdenlive 25.08 added a rotation handle) | This is the main CapCut gesture. Numbers in the inspector are slow for a title or a sticker. | M | none | `app/editor/preview.rs`, new `app/editor/canvas.rs`; engine hit test in `render/layout.rs`; `SetTransform` exists | `preview.rs` overlays: motion pivot, mask, crop, enhance, caption, tracking. No transform overlay |
| 4 | **Animated caption styles** (active word pops or gets a box, keyword colour, word entrance, caption templates) | CapCut (Pro), Resolve (Studio), Premiere 26.3 | The top caption request on Shotcut and KDE forums. Manual work costs 3 to 5 min per caption. | M | none | `modules/captions/`, `modules/text/` (spans), `app/editor/captions/` | `CaptionStyle.highlight` is one colour; `karaoke.rs` lights a byte range only |
| 5 | **Rich text** (colour per word, gradient fill, second stroke, font weights) | CapCut, Resolve 21 (per-character style) | "Yellow keyword" captions and thumbnail-style titles need it. Also the base for #4. | M | none | `project/document.rs` `TextMaterial`, `modules/text/` | `TextMaterial` has one `color`, one stroke, `bold` only; no `gradient` in text code |
| 6 | **Text-based editing** (a transcript panel: delete words to cut, search, keep sync) | CapCut "Transcript" (Pro), Premiere, Resolve IntelliCut (Studio), Premiere 26.5 Paper Edit | Talking-head and podcast creators cut by reading. chukcut already has word times and the ripple machinery. | M | none | new `modules/transcript/` on `silence::cut`; new `app/editor/transcript.rs` | no match for a transcript UI; `captions/mod.rs` has `Transcript` and `TimedWord` only for captions |
| 7 | **Reverse a clip** | CapCut (toolbar), all others | Basic short-form trick. CapCut has it in the toolbar. | M | none | `modules/speed/` + a baked cache like `speed/flow` | no match for `reverse` as a clip feature in `crates/` |
| 8 | **Long video to shorts** (find highlights, make 9:16 clips with captions) | CapCut Auto Cut (Pro), Opus Clip-style tools | Users run separate tools for this. It is a strong reason to pick an editor. | M | LLM optional (own key or local server); a heuristic fallback needs no model | new `modules/highlights/`; reuses `analysis::reframe`, `captions`, `template` | no match for `highlight`, `long to short`, `auto_clip` |
| 9 | **Local text to speech** (Piper, Kokoro) and **local translation** (Opus-MT) | CapCut, Resolve 21, Shotcut (Kokoro) | TTS voice-overs are standard in short-form. Today they need a paid account. | M | Piper code GPL-3.0 (fine), each voice has its own licence; Kokoro Apache-2.0; Opus-MT CC-BY-4.0 (credit); NLLB is NC, exclude | `crates/ml-worker/` (new `piper.rs`, `kokoro.rs`, `marian.rs`), `modules/cloud` provider shape | Kokoro appears only as a remote server in `cloud/registry.rs`; `docs/research/ml-features.md` 3.2, 3.3 recommend it, not built |
| 10 | **Multicam** (sync angles by audio, switch while playing) | CapCut (Dec 2025), Resolve, Premiere, Kdenlive 26.08 | Podcasts and interviews with two or three cameras. | L | none | new `modules/multicam/` on compound clips | no match for `multicam` |
| 11 | **Video scopes** (waveform, vectorscope, histogram, RGB parade) | Resolve, Premiere, Kdenlive, Shotcut | Needed to grade by numbers, not by eye. | S–M | none | new `render/scopes.rs`, app panel in the Adjust tab | no match for `vectorscope`, `parade`, `waveform_monitor` |
| 12 | **Audio mixer**: track volume, pan, solo, meters while playing | Shotcut 26.9 (track meters), Kdenlive, Resolve, Premiere | Users cannot see levels while they edit. There is no pan at all. | M | none | `audio/mixer.rs`, `audiofx/`, timeline headers | `Track.volume` exists but no command and no UI sets it; no `pan`, no `solo`; the only meter is in the export dialog (`export/loudness.rs`) |
| 13 | **Grade and LUT on an adjustment clip** | CapCut (filter/adjust layers), Premiere, Shotcut 26.9 | One look over many clips, trimmed on the timeline. | S–M | none | `render/compositor.rs` `Draw::Adjust`, `grading/` | `Draw::Adjust` carries only an effect chain |
| 14 | **Keyframes on grade, crop and text style** | CapCut, Resolve, Premiere | Colour ramps, animated crops, text that changes colour. | M | none | `project/document.rs` `AnimatableProperty`, `grading/`, crop | `AnimatableProperty` = position, scale, rotation, opacity, volume; manual: "A crop has no keyframes" |
| 15 | **Blur faces and objects; bleep words** | Premiere 25.6 (Censor Transcript), Resolve, CapCut | Vlogs, street footage, privacy. All parts exist (YuNet, VitTrack, SAM, Pixelate). | S–M | none | `modules/fx/`, `modules/landmarks/`, `modules/tracking/`, `audiofx/` | no match for `censor`, `face blur`, `bleep` |
| 16 | **Shapes and simple motion graphics** (box, line, arrow, progress bar, lower third) | CapCut, Resolve (Text+, Krokodove), Kdenlive titler | Explainers and tutorials use them in every video. | M | none | new shape material in `project/`, renderer in `render/`, Text tab | no `ShapeClip`, no `progress_bar` |
| 17 | **Cover / thumbnail editor** and a cover in the MP4 | CapCut | The main-lane "Cover" tile is there but does nothing. | S | none | `app/editor/timeline.rs`, `export/` (attached picture) | `render_cover_column` has no click handler |
| 18 | **More effects, and user GLSL effects** | CapCut (thousands), Premiere 25.5 (+90), Shotcut 26.8 | "One-click effects" is a top request. 20 effects is small. | M (loader), ongoing (content) | own or permissive shaders only; never ByteDance packages in the repo | `modules/fx/` catalog, a user effects folder | `effects-and-transitions.md` lists 20 effects |
| 19 | **Media bins, sort and smart search** (search by spoken words, by objects) | Premiere Media Intelligence, Resolve IntelliSearch, CapCut smart search | Long projects with many files. | S (bins) / L (semantic) | SigLIP Apache-2.0, CLIP MIT | `app/editor/assets/media.rs`, `ml-worker` | search by file name only (`media.rs` `matches(&item.name, …)`) |
| 20 | **Screen and webcam recording** | CapCut, Shotcut 25.10 | Tutorial and gaming creators. | M | PipeWire portal on Wayland | new `modules/capture/` | no match for `screen record`, `webcam`, `v4l` |
| 21 | **Interchange: OTIO, FCPXML, EDL** | Kdenlive (OTIO), Resolve, Premiere | Send a cut to Resolve for a colourist or a mix. | M | OTIO Apache-2.0; we write the JSON ourselves | new `modules/interchange/` | no match for `otio`, `fcpxml`, `edl` |
| 22 | **Depth map: background blur, relight** | Resolve (Studio), CapCut (Pro) | "Cinematic" fake bokeh on phone footage. | M | Depth Anything V2 **Small** is Apache-2.0; Base/Large are CC-BY-NC, exclude | `crates/ml-worker/`, `modules/fx` | no match for `depth`, `relight` |
| 23 | **AI dubbing** (translate, TTS per caption, fit to time) | CapCut (Pro), Premiere | Reach other languages. | M after #9 | voice cloning: abuse complaints, add a consent step; most lip-sync models (Wav2Lip) are non-commercial, avoid; no ByteDance-authored models | chain in `modules/voice` + #9 | captions translate exists (`captions` Translate); no TTS chain |
| 24 | **Direct upload to YouTube and TikTok** | CapCut, Resolve 21, Premiere | Saves one step. | M | Google OAuth app verification for upload scope; TikTok Content Posting API needs an audit, unaudited posts are private. Business risk: work that can be blocked by a platform | `modules/cloud` | no upload code |
| 25 | **UI in other languages** | CapCut, all others | Reach outside English. The owner's own language is German. | M (scaffold) | none | `crates/app` strings, Fluent | no match for `i18n`, `gettext`, `fluent_` |
| 26 | **Plugins: OFX, LV2, VST** | Shotcut 26.6, Kdenlive (LV2, frei0r), Resolve (OFX) | Power users bring their tools. | L | VST2 SDK licence is closed, use VST3 (GPL option) or CLAP | `modules/audiofx`, `modules/fx` | no match; `docs/research/resolve-plugins.md` covers OFX |
| 27 | **Speaker labels (diarization)** | Resolve (IntelliCut), Premiere | Caption colour per speaker, multicam auto switch. | M | check each model's licence | `ml-worker`, `captions` | no match for `diariz`, `speaker` |

Not ranked, because the value is low or the cost is high: generative video
(Generative Extend, text to video; `ml-features.md` 3.17 puts it out of
scope, fal.ai already covers some of it), AI avatars, collaboration, an undo
history panel, project version snapshots.

## 2. UX and workflow gaps

These features exist, but they are hard to find or slower than in CapCut.
Each was checked in the code.

1. **No direct manipulation on the player** (see #3). To place a title you
   type numbers in **Video › Basic › Transform**. CapCut users drag. Only
   captions, masks, crop, the zoom pivot and tracking boxes are draggable
   (`preview.rs` overlays).
2. **The timeline toolbar is fixed, not context-sensitive.** It has undo,
   redo, split, delete left/right, delete, marker, voiceover, magnet,
   snapping and zoom (`timeline.rs`, `tool_button` calls). CapCut changes
   the toolbar by clip type: crop, freeze/reverse/mirror/rotate, remove
   background, auto adjust, split scenes for video; beat detect, isolate
   voice for audio (`docs/reference/capcut/23-toolbar-tooltips.png`,
   `22-audio-clip-selected.png`). In chukcut these are in the right-click
   menu or two levels deep in the inspector.
3. **Drag and drop is incomplete.** An effect dragged onto a clip becomes an
   effect clip on an effect lane; it does not go onto that clip
   (`assets/effects.rs` `on_effect_drop`). Transitions, filters (looks) and
   stickers have no drag type at all (`MediaDrag`, `EffectDrag`, `TitleDrag`
   only). A transition goes onto the cut to the right of the selected clip,
   so the user must first select the correct clip.
4. **No preview before you add.** A click on a media tile only selects it
   (`manual/media.md`). There is no skimming (CapCut "Preview axis", key
   `S`). Effect and transition tiles do not play on hover; CapCut and
   Kdenlive 26.04 show animated previews.
5. **The Cover tile is a dead button** (#17). It looks like CapCut's, so
   users will click it.
6. **No safe-area or platform overlay on the player.** Kdenlive 25.12 and
   Shotcut 26.4 have 9:16 guides. Caption placement already knows that the
   bottom fifth is covered (`captions/style.rs` `Placement::y`), but the
   user cannot see it. Effort S.
7. **Full screen shows the whole window.** The player's full-screen button
   calls `window.toggle_fullscreen()` (`preview.rs`). Users expect the
   picture only. Effort S.
8. **Track volume has no control.** The document has `Track.volume`, but no
   command and no widget sets it. A track solo does not exist.
9. **Speed and keyframe gaps that users meet early:** a freeze frame is
   always 3 s first (`timeline/freeze.rs` `DEFAULT_FREEZE`); a crop has no
   keyframes; the grade has no keyframes.
10. **Few ready-made assets for one-click work.** 20 effects, about 130
    transitions, 29 text styles plus 10 text templates, 6 caption presets,
    11 project templates. CapCut refugees judge an editor by its asset
    panel. Content work, not code, closes most of this.
11. **Delivery to Linux users.** There is a `.deb`, an AppImage and a
    tarball, but no Flatpak (`packaging/` has `appimage`, `deb`, `linux`,
    `macos`, `windows`). Fedora Silverblue, Bazzite and SteamOS users install
    from Flathub first. FFmpeg comes from the `org.freedesktop.Platform.ffmpeg-full`
    extension; the CUDA bundles already download into the user's data
    folder, so they work in a sandbox. Effort M.

## 3. Performance and format gaps

### What is already good

- Every format that FFmpeg reads imports. VFR clips play by timestamp, and
  the rotation flag is read (`decoder.rs` keeps `rotation`). This removes the
  most common Linux complaints (VFR audio drift, portrait clips that come in
  landscape, AAC/H.264 refused by free Resolve on Linux). Say this on the
  landing page.
- VAAPI zero-copy decode, NVDEC through NV12 textures, NVENC/VAAPI/QSV export
  with a test encode, proxies with an automatic policy, shared preview
  textures. 4K with five layers plays at 30 fps on a quiet machine
  (`STATUS.md`, player bench).

### Gaps

| Area | Gap | Evidence | Effort |
|---|---|---|---|
| Export colour | BT.601 matrix, no colour tags. Players assume BT.709 for HD and shift the hues. | `export/encoder.rs` module doc "## Colour"; `yuv.wgsl` is BT.601 limited | S |
| HDR input | No HLG/PQ transfer, no BT.2020 to BT.709 gamut map. Phone HDR looks grey. | no transfer handling in `media/` or `render/` | M |
| 10-bit decode | VAAPI P010 surfaces are not in the import format table; NVDEC P010 falls back to the software path. | `hardware-decode.md` "10-bit and HDR"; `decoder.rs` `seek_and_download_nv12` doc | M |
| 10-bit / HDR export | Only ProRes is 10-bit. No HEVC Main10, no AV1 10-bit, no HLG/PQ output. Kdenlive 25.08 and Shotcut 26.4/26.6 have these. | `VideoCodec`, `encoder.rs` `upload_format` | M (after HDR input) |
| Alpha export | No ProRes 4444, no VP9 alpha, no PNG sequence. CapCut offers "RLE (alpha)". Needed for overlays and lower thirds made in chukcut. | no `yuva`, `qtrle`, `4444` in `crates/` | S–M |
| DNxHR export | Not offered. Resolve users on Linux transcode to DNxHR; a DNxHR master is the cleanest hand-over. | `dnxhd` appears only in `proxy/decision.rs` and `proxy/generate.rs` notes | S |
| VP9 / WebM, Opus, MKV | In the engine and CLI, not in the dialog. | `export/settings.rs`: "VP9 is not offered in the dialog" | S |
| Audio formats | Export: AAC, MP3, WAV. No FLAC, no Opus in the dialog. Mix is stereo only (5.1 is folded down). | `manual/export.md`; `audio/decode.rs`, `device.rs` | S (FLAC/Opus); L (surround) |
| Frame rates | Dialog: 23.976 to 60. No 100/120 fps for high-frame-rate delivery. | `export/settings.rs` `FRAME_RATES` (8 entries) | S |
| Chapters | Markers have labels and colours (`Marker`), but the export writes no chapters. Shotcut 25.08 writes markers as chapters. YouTube reads chapters from the description; a "copy chapters" text helps too. | no `chapter` in `crates/` | S |
| Render cache | No persistent render cache for heavy regions (Resolve smart cache, Premiere render in-to-out). The frame ring holds about 2 s. Heavy AI plus effect stacks will drop frames. | `preview/cache.rs` | M–L |
| Multicam | None (see #10). Needs several decoders at once, which the VAAPI pool note warns about. | `hardware-decode.md` "Whether holding many mapped frames at once starves the decoder" | L |
| Long timelines | Not measured. No bench for a 60 min podcast with 500 clips: timeline draw time, autosave time (the whole JSON), undo memory. | no long-timeline case in `chukcut-bench` or `player_bench` | S (bench), then fixes |
| AMD and Intel | Not checked regularly (`STATUS.md` "Rough"). Backlog 11 waits for the owner's laptop. | `plan/build-out.md` backlog 11 | S (needs hardware) |
| Wayland | Only X11 is verified on screen. GPUI supports Wayland, but the patched shared-texture path was not checked there. | `STATUS.md`, `QA.md` mention X11/Xvfb only | S (check) |

## 4. Proposed next waves

Each wave has four agents at most (the machine budget in
`plan/build-out.md`). Each package names the files it owns, so agents do not
collide. The order inside a wave is the merge order.

### Wave 12: correct pictures and the CapCut feel (highest value)

| Agent | Scope | Owns | Done when |
|---|---|---|---|
| **colour-io** | BT.709 matrix and colour tags in every export (software and GPU NV12 path). Probe transfer and primaries. HLG and PQ to SDR tone map and BT.2020 to BT.709 gamut map in the source shader. P010 in the VAAPI import table and on the NVDEC upload. Details tab shows the clip's colour space. | `export/encoder.rs`, `render/shaders/yuv.wgsl`, `render/source.rs`, `media/decoder.rs`, `media/dmabuf.rs`, `app/editor/inspector/details.rs` | ffprobe shows `bt709` tags; a colour-bar round trip stays within 2 code values; an HLG and a PQ fixture (made with ffmpeg `zscale`) render within a set tolerance of a reference tone map; `tests/every_card.rs` passes |
| **canvas** | Click on the player selects the top clip under the pointer. Box with corner handles (scale), a rotate handle, drag to move. Snap to centre, edges and other clips, with guide lines; Shift turns snapping off. Writes keyframes when the property has keyframes. Safe-area overlay (TikTok, Reels, Shorts) and a thirds grid in the player menu. Player-only full screen. | new `app/editor/canvas.rs`, `app/editor/preview.rs`; engine hit test in `render/layout.rs`, exposed as `inspector_hit_test` in `inspector/commands.rs` (plus CLI) | drag, scale and rotate are one undo step each; the hit test matches the compositor's order on rotated and cropped clips; works on titles, stickers, video, compound clips |
| **captions2** | Text spans in `TextMaterial` (colour, scale, background per byte range). Active-word styles: colour, pop (scale), box, underline. Word entrance animations. Keyword emphasis (manual pick, plus an automatic pick: longest or rarest words; LLM pick when an account exists). 10 caption templates. | `modules/text/`, `project/document.rs` (`TextMaterial` spans, serde default), `modules/captions/`, `app/editor/captions/`, `app/editor/inspector/text_style.rs` | old files open unchanged; preview equals export; CLI and MCP reach the new commands (`tests/reachability.rs`) |
| **toolbar** | Context toolbar by clip kind (crop, freeze, reverse, mirror, rotate; remove background, auto adjust, split scenes; beats and isolate voice for audio). Drag effects, looks and stickers onto a clip, transitions onto a cut. Cover tile opens a cover picker (frame or image, plus a title), and the export embeds it as the MP4 cover. Freeze frame asks for its length. | `app/editor/timeline.rs` (toolbar only), `app/editor/assets/*.rs`, `app/editor/mod.rs` (drops), `export/` (attached picture) | every new button has a keymap action (`keymap_doc` blessed); drops are one undo step |

Reverse goes with **toolbar** only as a button. The engine part (a baked
reversed copy in the cache, reversed audio, invalidation like `speed/flow`)
is a fifth package. Run it in wave 12 if a build slot is free, else first in
wave 13.

### Wave 13: the talking-head workflow

| Agent | Scope | Owns | Done when |
|---|---|---|---|
| **transcript** | Transcript panel: the words of the timeline with times, click to seek, select words and delete (ripple cut that keeps sync, through `silence::cut`), search, mark fillers. "Bleep" a word (tone or mute). | new `modules/transcript/`, new `app/editor/transcript.rs`, CLI `transcript` | a delete of 5 words is one undo step and keeps captions and music in sync; works inside compound clips |
| **shorts** | Long video to shorts: score windows by speech density, loudness, scene cuts and (optional) an LLM over the transcript; make N new timelines, each reframed to 9:16 with captions and an optional template. | new `modules/highlights/`, CLI `shorts`, a dialog in `app/editor/` | a 20 min test file gives 3 to 5 timelines without an account; with an account, the LLM choice is used and the cost shows first |
| **voice-local** | Piper and Kokoro TTS and Opus-MT translation in the ML worker. TTS as an account-free choice in the Audio tab. Dubbing chain: translate captions, TTS per caption, fit the time (stretch or gap). Consent checkbox for any cloned voice. | `crates/ml-worker/src/{piper,kokoro,marian}.rs`, `ml-worker/src/registry.rs`, `modules/voice/` or new `modules/tts/`, `app/editor/assets/library_audio.rs` | a German and an English voice work offline on the CPU; licences show in Settings › AI acceleration; the credits file names the voice |
| **mixer** | Track volume, pan and solo; pan per clip (new `AnimatableProperty::Pan`, keyframable); level meters on the track headers and a master meter during playback; one effect chain per track. | `audio/mixer.rs`, `audio/engine.rs`, `audiofx/`, `project/document.rs` (Track fields with serde defaults), timeline headers in `app/editor/timeline.rs` | the export mix equals the preview mix; old projects play the same |

### Wave 14: depth for longer and pro work

| Agent | Scope | Owns | Done when |
|---|---|---|---|
| **scopes** | Waveform, RGB parade, vectorscope, histogram, computed on the GPU from the preview frame; a panel beside the Adjust tab. | new `render/scopes.rs`, new `app/editor/scopes.rs` | under 2 ms per frame at 1080p on the RTX 3060; matches a reference image on lavapipe |
| **grade-keys** | Grade and LUT on effect clips (adjustment clips). Keyframes on grade values, crop and text colour and size. | `grading/`, `project/grade.rs`, `render/compositor.rs` (`Draw::Adjust`), crop in `inspector/`, `AnimatableProperty` | adjustment clip renders the same in preview and export; keyframed grade round-trips save and undo |
| **formats** | Export: ProRes 4444 and VP9 with alpha, DNxHR, HEVC Main10 and AV1 10-bit, HLG/PQ tags when the source is HDR, VP9/WebM, FLAC and Opus, 100/120 fps. Chapters from markers (MP4 chapters and a text list to copy). | `export/presets.rs`, `export/encoder.rs`, `app/editor/export/` | each codec passes a test encode and an ffprobe check; the dialog hides what this FFmpeg build lacks |
| **multicam** | Multicam clip from 2 to 9 clips, sync by audio (cross-correlation of onset envelopes) or by timecode, angle view in the player, switch by number keys while playing, result as cuts inside a compound clip. | new `modules/multicam/`, `app/editor/multicam.rs` | three generated angles with offset audio sync within one frame; switching is one undo step per cut |

### After wave 14

In value order: face and object blur (#15), shapes and lower thirds (#16),
user GLSL effects and more built-in effects (#18), Flatpak (UX 11), UI
translation with German first (#25), depth blur (#22), media bins and smart
search (#19), screen recording (#20), OTIO/FCPXML export (#21), a
long-timeline bench, a render cache. Direct upload (#24) stays last, because
a platform can block it at any time.

## Sources

Competitors:

- CapCut: https://www.capcut.com/resource/capcut-pro-pc ,
  https://www.capcut.com/resource/capcut-auto-cut ,
  https://www.capcut.com/help/why-do-i-have-to-pay-for-auto-reframe ,
  https://www.capcut.com/resource/multicam-editing ,
  https://www.capcut.com/help/export-videos-in-capcut ,
  https://www.capcut.com/help/valid-devices-for-standard-subscription
- Resolve: https://www.blackmagicdesign.com/media/partial/release/20250404-02 ,
  https://www.blackmagicdesign.com/products/davinciresolve/whatsnew
- Premiere: https://helpx.adobe.com/premiere/desktop/whats-new/whats-new.html ,
  https://blog.adobe.com/en/publish/2025/04/02/introducing-new-ai-powered-features-workflow-enhancements-premiere-pro-after-effects ,
  https://blog.adobe.com/en/publish/2025/11/19/adobe-premiere-adds-smarter-search-faster-edits-seamless-collaboration
- Kdenlive: https://kdenlive.org/news/releases/25.04.0/ ,
  https://kdenlive.org/news/releases/25.08.0/ ,
  https://kdenlive.org/news/releases/25.12.0/ ,
  https://kdenlive.org/news/releases/26.04.0/ ,
  https://kdenlive.org/news/releases/26.08.0/
- Shotcut: https://www.shotcut.org/blog/new-release-26.4.30/ ,
  https://www.shotcut.org/blog/new-release-26.6.25/ ,
  https://www.shotcut.org/blog/new-release-26.9.27/

Users:

- https://news.ycombinator.com/item?id=44553752 (OpenCut, Jul 2025)
- https://news.ycombinator.com/item?id=47815118 (State of Kdenlive, Apr 2026)
- https://news.ycombinator.com/item?id=49610181 (Resolve 21.1, Sep 2026)
- https://discuss.kde.org/t/kdenli-ve-capcut-vs-fi-lmora/47297
- https://discuss.kde.org/t/some-features-kdenlive-should-have/41270
- https://discuss.kde.org/t/portrait-format-import-variable-bit-rate/48175
- https://forum.shotcut.org/t/addition-of-animated-captions-subtitles-in-shotcut/50895
- https://forum.shotcut.org/t/these-features-are-a-must-have-for-2026/50309
- https://www.provideocoalition.com/aac-audio-kdenlive-beats-davinci-resolve-studio-on-linux/
- https://www.descript.com/blog/article/capcut-captions-arent-free-anymore-heres-a-better-option
- https://www.techloy.com/capcuts-latest-terms-of-service-raises-big-questions-about-content-ownership/
