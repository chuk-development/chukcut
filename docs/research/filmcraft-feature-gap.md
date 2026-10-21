# FilmCraft feature gap

What does FilmCraft (storytold/filmcraft) do that chukcut does not, and what
should we take? The owner's brief: take their functions, keep our CapCut-like
UI, keep FFmpeg, credit them openly.

Date: 2026-10-21. Base: chukcut `master` at `68f2ac5`; FilmCraft `main` at
`70f0fc7` (shallow clone in `/mnt/data/git/_ref-filmcraft`, read only).

## How this was made

- **Their side.** FilmCraft's README, `ROADMAP.md`, `docs/gaps.md`,
  `docs/target-app-parity.md`, and the code of the crates that are not codecs:
  `engine` (650+ commands), `edit`, `audio-dsp`, `interchange`, `captions`,
  `color`, `render`, `scopes`, `speech`, `tts`, `export`.
- **Our side.** Every `pub fn` in `crates/engine/src/modules/*/commands.rs`,
  `EditCommand` in `timeline/ops.rs`, `docs/STATUS.md`, `docs/manual/`, and
  a search of `crates/` for each candidate.
- **Left out on purpose.** Their MCP server and control channel, their
  release workflow (other agents do those), and their FFmpeg replacement
  (their own H.264, HEVC, ProRes, VP9, AV1, AAC, Opus, MP4, MKV, MXF code):
  we keep FFmpeg.

FilmCraft is licensed MIT OR Apache-2.0. We take it under Apache-2.0, which
the GPL-3.0 can absorb; copied or closely adapted files keep their copyright
line, and `NOTICE.md` lists what was taken.

## The two products in one paragraph

FilmCraft is a Premiere clone: three-point editing, trim modes, source and
program monitors, a mixer with automation, broadcast interchange (EDL, XML,
AAF, OMF, OTIO), a big effect catalogue that mirrors Premiere's names. Its
AI is thin (Whisper, a formant and a Kokoro TTS). chukcut is a CapCut clone:
magnetic timeline, templates, stickers, animated captions, and a lot of
local AI (mattes, tracking, RIFE, LaMa, Real-ESRGAN, face and body
landmarks, voice isolation). So the gap runs mostly one way in pro editing
and the other way in creator tools. The table lists what FilmCraft has.

## The table

Port effort: **S** under a day, **M** one to three days, **L** a week, **XL**
more. Value: what it is worth to a chukcut user (creators first, then
editors who hand off to Resolve or Premiere). **Ported** rows are done on
`agent/features`.

| # | Feature | FilmCraft | chukcut before | Port effort | Value |
|---|---|---|---|---|---|
| 1 | Audio clean-up effects: high-pass, low-pass, hum remover, de-esser, noise gate, look-ahead limiter | `audio-dsp/effects` (restoration, essential, dynamics, filters) | EQ, compressor, reverb, echo, pitch, RNNoise denoise | S | high — every voice recording wants a high-pass, de-ess and limiter. **Ported** |
| 2 | Character effects: chorus, flanger, phaser, distortion, stereo width | `audio-dsp/effects` (modulation, special, essential) | voice presets only | S | medium. **Ported** |
| 3 | Timeline interchange export: OTIO, FCPXML, CMX 3600 EDL | `interchange` (also FCP7 XML, AAF, OMF, ALE) | nothing — a cut could not leave chukcut | M | high — hand a cut to Resolve for grading or to Premiere. **Ported** (OTIO, FCPXML 1.10, EDL) |
| 4 | Relink missing media: folder search, same-recording check, folder remap for the rest | `engine/relink.rs` | offline state only (decision 0009 named relink as the next step) | M | high — opening a project on another machine. **Ported** |
| 5 | Marker export: CSV of review notes | `engine/marker_export.rs` | markers with label and colour, no export | S | medium — plus YouTube chapters, which FilmCraft has not. **Ported** |
| 6 | Sync clips by their sound (waveform cross-correlation, GCC-PHAT) | `audio-dsp/sync.rs`, `engine/sync.rs` | nothing | M | high — a separate mic and the camera's sound, lined up in one click. **Ported** |
| 7 | Interchange import (OTIO, FCPXML, FCP7 XML, EDL, AAF) | `interchange` | nothing | L | medium — bring a Premiere or Resolve cut in |
| 8 | Video scopes: waveform, vectorscope, histogram, RGB parade | `scopes` crate, `engine/scopes.rs` | none | M | medium-high for grading; needs a panel in the Colour tab |
| 9 | Text-based editing: delete words in the transcript to cut the video | `edit/transcript` | filler and pause removal from caption words, no word-level editing view | M | high for talking-head creators |
| 10 | Reverse clip playback | `TrackItem.reverse` through decode and audio | none | L | high (CapCut has Reverse); needs backward decode with a GOP cache |
| 11 | Music remix to a target length (beat-similar cut points) | `audio-dsp/remix.rs`, `engine/remix.rs` | beat detection, auto-cut to beat | M | high for creators: fit a song to the video |
| 12 | Three-point editing: insert, overwrite, lift, extract with in/out marks | `edit` | in/out marks for loop and export only | M | medium (pro workflow) |
| 13 | Trim modes: roll, slip, slide, ripple trim to playhead | `engine/trim.rs` | edge trims, delete left/right, magnetic main lane | M | medium |
| 14 | Close gaps / ripple delete on any lane | `edit::ripple_delete`, close gap | magnetic main lane only | S | medium |
| 15 | Audio track mixer: pan, mute/solo, sends, submixes, automation (read/latch/touch/write) | `engine/mixer.rs`, `render/mixer.rs` | per-clip and per-track volume, no pan | L | medium; pan alone is S |
| 16 | Multicam: sync angles, cut between them live | `engine/multicam.rs` | none | XL | medium (podcasts, interviews); builds on #6 |
| 17 | Render previews cache (green bar, content-addressed) | `engine/previews.rs` | proxies, baked ML frames | L | medium; the GPU preview is fast already |
| 18 | Scene edit detection | `engine/scene_detect.rs` | **has it** (`analysis_detect_scenes`) | — | — |
| 19 | Loudness meters (BS.1770 momentary/short/integrated, true peak) | `audio-dsp/loudness.rs` | **has it** (`loudness_measure_*`, export target) | — | live meters during playback would be new (S) |
| 20 | Autosave with versions, crash-recovery journal | `engine/autosave.rs` | **has it** (autosave, recovery claim/restore) | — | rolling versions would be new (S) |
| 21 | Captions: SRT, WebVTT, SCC, CEA-608/708, TTML, STL; burn-in | `captions` | SRT and VTT import/export, styled burn-in | S–M | low for creators (SCC/TTML are broadcast) |
| 22 | Colour: hue-vs-sat/hue/luma and luma-vs-sat curves, HSL secondary key | `color`, `render/lumetri` | basic, HSL bands, RGB curves, wheels, LUTs, auto, match | M | medium — needs shader work |
| 23 | 90+ video effects (Premiere's catalogue: corner pin, posterize, echo, track matte, VR effects, …) | `render/vfx` | 20 effects plus the effect runtime | M per few | medium; pick by what creators use |
| 24 | Keyframe value and velocity graphs, Bezier handles | `ui-egui` Effect Controls | easing presets and a custom curve | M | low-medium |
| 25 | Local TTS (Kokoro, formant) | `tts`, `tts-text` | cloud TTS only | L | medium (privacy) |
| 26 | Speaker diarization, VAD | `speech` | voice detection for silence removal | M | medium (podcast captions per speaker) |
| 27 | Project manager: collect files, consolidate | `engine/project_manager.rs` | none | M | medium (archiving) |
| 28 | Export: PNG sequence, MXF OP1a/OP-Atom, DNxHR, ProRes 422 HQ | `export` | H.264/HEVC/AV1/ProRes/GIF via FFmpeg, PNG still | S–M | low (FFmpeg can do most once wired) |
| 29 | Media browser and source monitor | `engine/media_browser.rs`, `source_monitor.rs` | import dialog, media tab | M | low for the CapCut flow |
| 30 | Essential Sound (tag clips as dialogue/music/SFX, auto-duck) | `engine/essential_sound.rs` | ducking under speech | S | covered |

## Ranked port plan

Ranked by value for reasonable effort, engine first, in our UI:

1. **Audio clean-up and character effects** (#1, #2) — done: 11 new kinds in
   `audiofx`, the inspector's Audio tab lists them like the others.
2. **Timeline interchange export** (#3) — done: `modules::interchange`, Menu ›
   Export timeline as OTIO / Final Cut XML / EDL, `chukcut-cli
   export-timeline`.
3. **Relink missing media** (#4) — done: `media::relink`, a banner with Find
   in folder… in the Media tab, `chukcut-cli relink`.
4. **Marker export** (#5) — done: CSV and YouTube chapters.
5. **Sync by sound** (#6) — done: `analysis_sync_audio`, clip menu "Sync by
   sound", `chukcut-cli sync`.
6. **Music remix to length** (#11) — next: beat analysis exists; port the
   self-similarity planner from `audio-dsp/remix.rs`, render through
   `audiofx` (a remixed clip is a segment list on one material).
7. **Text-based editing** (#9) — the captions already have word times; a
   transcript view in the Captions tab that cuts on delete, through the
   existing `silence` cut code.
8. **Video scopes** (#8) — port `scopes` (CPU, from the preview frame's
   readback), a panel in the Colour tab.
9. **Close gaps / ripple delete on any lane** (#14) — small.
10. **Interchange import** (#7) — OTIO and FCPXML first.
11. **Reverse** (#10) — the most asked of the large ones; needs decoder work.
12. **Colour curves (hue-vs)** (#22), **pan** (#15), **live loudness meter**
    (#19).

## What chukcut has that FilmCraft does not

For the record, so nobody ports backwards: local ML (mattes, select object,
tracking, RIFE slow motion, LaMa remove object, Real-ESRGAN, face and body
landmarks, voice isolation), templates, stickers and the animated sticker
library, animated captions with karaoke, the text animator, speed curves and
speed effects, auto reframe, beat cuts, silence and filler removal, cloud
integrations, CapCut-style effect packages, hardware decode zero-copy into
the compositor.
