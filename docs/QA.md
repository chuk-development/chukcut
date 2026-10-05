# QA pass, 2026-10-03

What the QA agent tested after the night's merges, what passed, what it fixed,
and what is still broken. Branch `agent/qa`. Ordered by severity inside each
list. Machine: RTX 3060 (NVDEC, NVENC; no VAAPI driver), UI on a private Xvfb
display with lavapipe as the window's Vulkan driver.

## How it was tested

- **Engine, end to end:** `crates/engine/tests/full_workflow.rs` drives one
  whole session through the command layer, the way the app does: new project,
  three imports (two clips with sound, a still with a stock licence record),
  split, ripple delete, move, a multi-clip delete, a grade (brightness,
  contrast, a LUT generated in the test, a master curve), an effect and an
  effect clip, a clip animation, a title with a text animator, captions from an
  SRT plus a word-timed caption with karaoke, a transition, tracking of a
  generated moving disc with a follower title, silence removal with
  everything kept in sync, a loudness target of −14 LUFS, and an export on
  software x264 and every usable H.264 hardware encoder (here: NVENC). ffprobe
  and ffmpeg check the frame count, the duration and the loudness (±1 LU); the
  decoded frames show the title box, the warm grade, the lit karaoke word and
  the follower moving exactly as far as the disc. Then the captions sidecar,
  the credits file, a save → open → save that must be byte-identical, undo
  back to the empty project and redo to the end. About 75 s.
  `cargo test -p chukcut-engine --test full_workflow -- --nocapture`.
- **UI:** the app on Xvfb at 1920×1080 and 1366×768 with `HOME`, every
  `XDG_*` directory and the session D-Bus pointed into `_scratch/`, ALSA
  pointed at a null device. Media came in on the command line (no portal on
  Xvfb). Covered: start screen, new project, settings (every section, adding an
  account), shortcuts sheet, every asset tab and category, every inspector tab
  for video, audio, image, title, caption and effect clips, the timeline
  toolbar, context menu, split, drag, inline title edit, keyframes,
  transitions, freeze frame, tracking (box, run, follow, Ctrl+X bake prompt),
  silence removal, the player menus, the export dialog with a −14 LUFS export
  (checked: 428 frames, −14.0 LUFS), the unsaved-changes guard, crash
  recovery, and Ctrl+S on an opened project (byte-identical to the engine's
  save). No panic in any log.

## Fixed on this branch

1. **Transition edits were never autosaved** (`888f1ed`). The transitions
   module built its own edit response and skipped the working-copy write, so
   a crash lost every transition added since the last edit of another kind.
   Found because a restored session came back without its transition.
2. **Ctrl+X on a tracked clip skipped the bake prompt** (`b3bb4a8`). Cut now
   asks like Delete does.
3. **Freeze-frame stills**: named after the source and the frame time
   (`take frame 0m03.016s a177b914.png`), which is what the media library
   shows; and the stills this process made are deleted on save and close when
   neither the document, the undo/redo stacks nor a project file saved in this
   process uses them (`cbdabbc`).
4. **Asset rail at 1366×768**: Captions and Transitions wrapped onto two lines
   and Stock was cut off. The tabs now share the width and ellipsize
   (`fb5ced9`).
5. **A title's Video tab** offered Remove background / Mask / Retouch and the
   Stabilise … Optical flow rows, and kept a video clip's last sub-tab
   (`69e92d9`).
6. **Inspector Effects tab** put on the design kit: tokens, `IconButton`,
   `EmptyState`, pixel type scale (`9d0b9fe`).

## Fixed by the polish pass (`agent/polish`, same day)

1. **File access without a portal.** Every file question goes through
   `editor::files::choose`: the portal first, and when it errors our own
   browser (places, recent folders, typed paths, filter, hidden files,
   multi-select, save name with a replace check). The export folder is
   chosen there too and the next export starts in the last one used.
   Verified on Xvfb with no D-Bus: Open, Import, Save, the export folder
   over the export dialog.
2. **A chosen canvas is kept.** `Project::canvas_chosen` (absent = not
   chosen, so old files round-trip unchanged): set by clicking a canvas or
   frame rate on the start screen, by project settings and the player's
   ratio menu. Only an unchosen project adopts the first clip's shape.
3. **Ruler clicks, scrubs, split, freeze and Q/W land on frames**, and a
   drop, move or trim that reaches no snap target puts the edge on a frame.
   Verified: ruler click + split cut at exactly frame 214.
4. **Saved files leave out unreferenced parameter materials** (grades,
   effects, animations, speed curves, follow links, analysis extras). Only
   the written copy is pruned; the live pool keeps everything undo, redo
   and the clipboard can reach. `full_workflow` checks the reopened file
   against the pruned document and still saves byte-identical.
5. **Analysis maps time through `TimeMap`**, so beats, scene cuts, reframe
   keys, snap-to-beat trims and stabilisation follow speed curves.
6. **Karaoke on imported subtitles**: word times are estimated per cue on
   import.
7. Lows: empty "Untitled" working copies are not offered back and the
   restore prompt only appears on the start screen; old status messages
   leave the title bar after 8 s; Menu shows Ctrl+N; Settings → Hardware
   reports zero-copy only when a VAAPI decoder works.

## Still broken or missing

### High

- ~~No way to save or choose a file without a portal~~ — fixed above.
- ~~**There is no text styling UI.**~~ **Closed on `agent/titles`.** A
  title's inspector has a Text tab (words, font, size, bold/italic/underline,
  letter and line spacing, alignment, colour and opacity, outline, shadow,
  box with padding and radius, a 3×3 position grid), every change one undo
  step; the asset panel's Text tab has 29 styles and 10 templates drawn by
  the compositor. Verified on Xvfb: typing, a colour drag and a slider drag
  each undo in one step; a template click restyles and animates the selected
  title; a tile dragged onto the timeline lands where it was dropped.

### Medium

All four below were fixed by the polish pass; kept for the record.

- ~~**The canvas the user picked is replaced by the first clip's shape.** The
  start screen preselects 9:16; importing a 16:9 clip into the empty project
  silently makes it 1920×1080 (`import_material`, "canvas adopted from the
  first imported clip"). Fine when the user did not choose, wrong when they
  clicked 9:16 on purpose. It needs a "chosen" flag in the document, so it is
  left for the owner's call.
  Steps: start screen → 9:16 → New project → import a landscape clip.~~
- ~~**Clicking the ruler puts the playhead between frames**, and split and
  freeze frame cut at that exact microsecond (a cut at 3.016761 s, not at
  frame 90). Export samples by frame, so the result is right, but clip edges
  off the frame grid make later frame-accurate edits and the timecode
  readout disagree.
  Steps: click the ruler near 00:03, split, read the left clip's duration in
  the saved file.~~
- ~~**Grade edits leave unreferenced colour materials in the saved file.**
  Every grade commit mints a new `ColorAdjustMaterial`; the old one stays in
  the pool (3 of 4 unreferenced after four grade edits in the end-to-end
  test). Undo needs them while the session lives, but the file keeps them for
  ever. Titles and effects do the same, by design ("an unreferenced material
  is inert"). A prune of unreferenced colour and effect materials at save
  time (file only, not the live pool) would keep files small.~~
- ~~**Karaoke does nothing on captions imported from an .srt**, because an SRT
  has no word timing (`Cue::words` is empty and the Captions tab says so).
  Estimating words over each cue, as already done for providers without word
  timing, would make it work.~~

### Low

- ~~The restore prompt offers to bring back an empty "Untitled" (0 clips) and
  shows even when a project was opened from the command line.~~
- ~~The title bar's "Autosaved … ago" does not move after a transition edit.~~
  Checked on agent/polish2 in the running app (Xvfb, lavapipe): the label
  said "Autosaved 1 min ago"; a Cross dissolve from the Transitions tab
  turned it into "just now", and the working copy on disk had the
  transition. Fixed by `888f1ed` already; nothing left to do.
- ~~The export dialog does not remember the resolution or loudness target
  (the folder it now remembers), and its size estimate was 24 MB for a
  3.4 MB file.~~ Fixed on agent/exportq: the dialog opens with the project's
  last settings (else the last ones of any project), and the size is
  measured by a sample encode (`export::estimate`, within ±25 % by test,
  2 % measured).
- ~~Settings → Hardware says "Zero-copy decode: Yes" on a machine where no
  VAAPI decoder works.~~
- ~~The title bar keeps the last error until the next status message.~~
- ~~A tooltip that is open while its button changes keeps the old text
  (the effect's eye shows "Turn off" after it was turned off).~~ Fixed on
  agent/upkeep: the buttons whose tooltip follows a state (effect and mask
  eyes, the key eyedropper, the file browser's hidden-files switch, an export
  queue row's stop/skip) carry the state in their element id, so a click makes
  a new element whose tooltip starts closed. The player's play/pause button
  (`preview.rs`) does the same since agent/polish2 (`player-play` /
  `player-pause`). Not toggled on screen: starting playback on the test
  display would need audio output.
- ~~Menu has no New project shortcut hint although Ctrl+N works.~~
- ~~Projects whose clips were placed before the frame snapping keep their
  off-grid edges, and a drop that snaps to such an edge stays off the grid.~~
  Decided on agent/polish2: touching an off-grid edge wins over the grid. A
  snap to such an edge stays exactly on it (no gap, no overlap). The real
  bug was the other case: a move or drop that reached no snap target was
  rounded to a frame *into* the off-grid neighbour, by a few microseconds;
  the move was refused and a drop landed at the end of the lane instead.
  `timeline::gesture::clear_of_neighbours` now slides such a clip flush
  against the edge (one frame of slack), in the clip drag and the media
  drop. Tests: `gesture::tests::a_rounded_edge_slides_flush_…`,
  `…a_flush_place_is_accepted_by_the_move_it_feeds`, and the app's
  `a_drop_rounded_into_an_off_grid_clip_lands_flush_against_it`.

## Not tested, and why

- The portal file chooser itself (no portal on Xvfb). The built-in browser
  that replaces it was tested for Open, Import, Save and the export folder;
  caption import/export, LUT import and Save frame use the same code path.
- Real audio output and real-time playback: the null ALSA device consumes
  samples at once, so the audio clock runs far faster than real time. The
  player itself is the performance agent's (`player.rs`); nothing there was
  changed or judged.
- VAAPI decode/encode and QSV: no Intel or AMD GPU in this machine.
- Cloud providers with real keys (TTS, sound, music, stock, fal.ai, DeepL).
  With a fake ElevenLabs key the voice list fails cleanly with "HTTP 401".

## Showcase pass, 2026-10-04 (`agent/demo`)

Built `_scratch/demo/showcase.chukcut` with `scripts/demo.sh` (CLI only),
opened it in the release app on Xvfb + lavapipe, exported it with the CLI.
`docs/demo.md` describes the project and the app.

Fixed on this branch:

- **Shift+wheel did not scroll the timeline's lanes** on X11. GPUI's X11
  backend turns a shifted wheel into a horizontal delta, and the handler read
  only the vertical one, so lanes below the panel could not be reached (the
  showcase's music lane). `on_timeline_scroll` now takes whichever axis moved.

Open (CLI gaps; for the CLI coverage owner) — all four closed on
agent/upkeep (checked on agent/polish2: `sticker`, `lane_add`,
`title add --track`, `--duration` on the title commands, and the
`docs/cli.md` glow and contrast notes):

- **No sticker command.** `library_sticker_index/fetch` exist in the engine,
  but the CLI and MCP cannot search or add a sticker. Workaround: `import` a
  PNG and `place` it.
- **`title add` has no `--track`, and no command adds a lane.** A title that
  overlaps another title in time is moved to the next gap on the same lane;
  `move --track` refuses a lane that does not exist. So a headline and a
  subtitle cannot be on screen together from the CLI.
- **`title template` and `title style` have no `--duration`**; a `trim`
  afterwards is needed.
- **`docs/cli.md`**: the example `effect add … glow --set intensity=0.8` uses
  the wrong scale (the parameter is 0..100). The grade control list says
  "saturation 1 is no change" but not that `contrast` also rests at 1
  (`contrast=0.1` flattens the picture).

Open (app, low):

- ~~With a tracked follower selected, the player draws the track's path also
  when the playhead is outside the follower's time.~~ Fixed on
  agent/polish2 (`editor/tracking.rs`): no path or box outside the
  follower's time. Not looked at on screen.
- ~~The Scene detection section draws "Split at scene changes" as the primary
  button while the clip says "not analysed".~~ Fixed on agent/polish2: the
  primary button is the next step, Detect scenes until scene changes were
  found, then Split. Seen on Xvfb.
- On lavapipe the first preview frame of an opened project takes 15 to 30 s.
  Not judged on a real GPU.

## Polish pass 2, 2026-10-04 (`agent/polish2`)

Template follow-ups (decision 0022) and the lows above.

- **A project made from a user template copies the template's media** into
  `<data>/template-media/<project id>/`; deleting the template no longer
  takes files out of the project (`tests/templates.rs` deletes the template
  and checks every path still exists).
- **A split slot stays one slot**, on the left half; the right half, a
  pasted or duplicated copy and a freeze frame's still are plain clips.
- **"Replace media…" in the timeline clip menu** for any video or photo clip
  on an unlocked picture lane (a slot or not), one undo step. Seen on Xvfb:
  the clip took the chosen file in place.
- **The media library hides the template's slot placeholders and music
  bed.** Seen on Xvfb with a Quick Cuts project made by the CLI: only the
  user's clip is listed.
- **Media given on the command line is imported by absolute path.** It was
  stored as typed (`_scratch/media/a.mp4`), so the project broke when opened
  from another directory, and the same file chosen again in a dialog became
  a second material. Found while checking Replace media.

## Compound clips 3, 2026-10-04 (`agent/compound3`)

- **Export dialog cover black** (from the shared-preview pass): reproduced on
  Xvfb with lavapipe on the showcase project by putting the playhead at the
  end of the timeline — the player and the cover were both black, shared
  and readback paths. Opening the dialog at any other instant, or before the
  first frame arrived, showed the frame. Fixed: at or past the end the
  preview shows the last frame (`preview::clock::shown_time`); seen on
  screen after the fix (the ball frame at 19:20).
- **Compound clip inspector:** made a compound clip with Alt+G on the
  showcase and selected it: tabs Video (Basic, Mask), Audio, Speed,
  Animation, Adjust, Effects, and the Scene detection section. Running an
  analysis from the UI was not tried on screen (covered by
  `tests/analysis.rs` through the command layer).
- **Not checked on screen:** a compound clip's audio effects in the preview
  (would play on the owner's speakers); covered by `tests/compound.rs`
  through the plan and the block mixer.

## QA pass 2, 2026-10-04 (`agent/qa2`)

The features of waves 7–9, end to end in the release build: the app on a
private Xvfb display (lavapipe, `CHUKCUT_FILE_DIALOG=builtin`, no session
bus, `HOME` and every `XDG_*` folder under `_scratch/`, ALSA on a null
device), the CLI and the ML worker on the RTX 3060 with the CUDA 13 bundle
that was already installed (the ML folder linked into the isolated cache,
nothing downloaded again). Generated media only. Per feature: does it
render, is it one undo step, does it survive save and reopen, does the
export match the preview (`render-frame`, the export compositor, against
frames decoded from the exported file).

| Feature | Renders | Undo | Save / reopen | Export = preview |
|---|---|---|---|---|
| Compound clip (Alt+G, CLI `compound create`), open, breadcrumbs, flatten | yes; nested = flattened byte for byte | yes (create, redo) | yes | yes (mean 1.5 code values, the H.264 encode) |
| Second timeline (+ tab, `timeline new/duplicate/switch`) | yes | yes | yes, open timeline kept | export renders the open one |
| Templates (start screen fill dialog, `template apply`) | yes | n/a (new project) | yes | yes (mean 1.5) |
| Shortcut editor (rebind Go to end to F9, reset) | yes | n/a | `shortcuts.json` written and cleared | n/a |
| Frame blending, Smooth slow-mo (RIFE on CUDA, 239 frames ~50 s at 1080x1920) | yes | one step for speed + mode | yes | yes (mean 0.4) |
| Motion blur on a moving animated GIF sticker | yes | yes | yes | yes |
| Animated stickers (Animated tab, Noto Lottie; imported GIF) | yes, Play once switch | yes | yes | yes |
| VitTrack (`track --tracker vittrack`, follower title) | yes, 120 frames in 2 s | engine tests | yes | yes |
| Remove background RVM (240 frames 6.4 s) and BiRefNet (240 frames 107 s) | yes | yes (toggle, Ctrl+Z) | yes | yes |
| Select object (MobileSAM + VitTrack, 120 frames 13 s) + `apply-to --grade background` + `remove-background --off` | yes, colour pop | yes | yes | yes |
| Shared preview texture | "preview frames reach GPUI sharing=Shared" on lavapipe | | | |
| MCP | 139 tools listed by `tools/list` | | | |

Also walked: every asset tab (Media, Audio, Text, Stickers, Captions,
Effects, Transitions, Filters, Stock, Templates), the inspector for video,
compound, sticker, title and audio clips, Settings (AI acceleration,
keyboard shortcuts, hardware, logs), the export dialog (a 6 s export with
NVENC, 180 frames). No panic in any log.

Fixed on this branch:

1. **"Finish missing frames" after every complete bake**, in Speed › Frame
   blending (optical flow) and in Video › Remove background. Both panels
   showed the button whenever the mode was on. They now read the coverage
   (at most every two seconds, again when a bake ends): nothing when
   everything is baked except "All N in-between frames are baked." for flow,
   and "N of M frames …" with the button when some are missing.
2. **The save dialog refused its own suggestion** for a project from the
   Before / After template: "Before / After.chukcut" contains a `/`, the
   dialog said only "Type a file name". Suggested names (Save, Save as, Save
   frame) replace `/` with `_` like the export dialog; a typed `/` is
   refused with "A file name cannot contain /".
3. **Media lost its Added badge** once its clips were moved into a compound
   clip (or used only on another timeline): the panel looked at the open
   timeline's lanes only. It now looks at every sequence.
4. **Settings › AI acceleration: "Models run on" squeezed to one letter per
   line** when the answer is long (a CUDA runtime path). The answer now sits
   under the label and wraps.
5. **Details showed a relative project path** for a project given on the
   command line (`chukcut media/x.chukcut`); it is opened by its absolute
   path now.
6. **The export dialog said "measuring…" for good** when Export was pressed
   before the sample encode for the size estimate ended: the cancelled
   measurement stayed in place without an answer, so the Done page still read
   "Size: about 5.1 MB · measuring…" and the dialog never measured again.
   A cancelled measurement is now dropped. Seen on Xvfb: "Size: about 1.7 MB".
7. CLI: `timeline list` printed only the counts; it prints the table the
   docs promise. `keyframe --property x|y` is accepted (`set` calls them
   `--x`/`--y`). `frame-blend` without `--mode` says "89 of 89 frames
   baked" in the human line too.

Open:

- ~~**Medium: a template cannot be applied into an open project.** Templates
  only make new projects (`template_build_project`); there is no "use as a
  new timeline" or "insert at the playhead". The showcase therefore renders
  the Quick Cuts project and puts its video on the "Template cut" timeline,
  which is not editable there. Steps: open any project, Templates tab, click
  a template: it asks for clips and opens a new project.~~ Fixed on
  agent/polish3: the fill dialog offers New project, New timeline and
  Compound clip at playhead; CLI `template apply --into PROJECT [--as
  timeline|compound] [--at T]`.
- ~~**Low: slots inside a compound clip are not slots any more.** After Alt+G
  on a template project's slot clips, Templates › This project and
  `template slots` say the project has no slots, and Replace media is only
  reachable by opening the compound clip. Steps: `template apply x.chukcut
  before-after a.mp4 b.mp4`, select both slot clips, Alt+G.~~ Fixed on
  agent/polish3: slots are listed in every timeline and inside compound
  clips, and Replace media fills them where they are.
- ~~**Low: ML status names the CUDA runtime by path when the ML folder is a
  symlink.** With `~/.cache/chukcut/ml` reached through a link, `ml status`
  and Settings say "CUDA 13 on the GPU with the CUDA runtime at
  /home/…/libcudart.so.13" instead of "with chukcut's CUDA libraries": the
  ML root is compared without resolving the link. Left to the ML owner
  (`modules/ml`).~~ Fixed on agent/polish3: both paths are resolved first.
  Seen: `ml status --probe` through the linked folder says "CUDA 13 on the
  GPU with chukcut's CUDA libraries".

Not tested, and why: playback with sound (Space and J/L are not pressed on
this machine), voiceover recording, the portal file chooser, cloud
providers, VAAPI/QSV (no Intel or AMD GPU here), and Select object by
clicking on the player (covered by the ml2 pass; here through the CLI).

## Polish pass 3, 2026-10-04 (`agent/polish3`)

The three open items of QA pass 2 (above, struck through) and these, each
seen in the debug app on Xvfb (lavapipe, `CHUKCUT_FILE_DIALOG=builtin`, no
session bus, `HOME` and every `XDG_*` folder under `_scratch/`, the ML
folder linked in, models on the RTX 3060):

- **Template into the open project.** Quick Cuts as a compound clip at the
  playhead: it went on a new lane above the busy main lane, selected, with
  "The template was made for 1080×1920; it is laid out on this project's
  1920×1080 canvas" in the title bar. Travel Diary as a new timeline: a
  second tab "Travel Diary", opened, its slot 2 placeholder at 00:03.
- **Slots inside the compound clip.** Templates › This project: "Slots (0 of
  6 filled)"; the dialog lists six slots `in "Quick Cuts"`; Fill… on slot 1
  with the built-in browser put the clip in, the compound clip's filmstrip
  showed it, and the timeline stayed on Timeline 01.
- **Bakes on open.** A project saved with Remove background on one clip
  (its mattes then deleted from the cache) and Smooth slow-mo on another
  (frames never baked) opened with "Preparing 209 frames · 43 % Stop" in
  the title bar; 90 mattes and 119 flow frames were made in about 14 s and
  the chip went away.
- **Bake progress in a short window** (1366×700): Enhance 4x started from
  the Enhance tab shows "Enhancing… 2 of 90", Stop and the bar pinned
  under the body while the sections above scroll; Stop ended it and the
  strip went away. Auto remove on the Remove background tab shows
  "Removing the background… 20 of 120 frames" the same way.
- **Stabilisation on a compound clip**: through the command layer
  (`tests/analysis.rs`, RTX 3060); the inspector shows the Stabilise section
  on a compound clip's Video › Basic and the clip menu enables Stabilise.
  Not run from the UI on screen.

Not tested, and why: the optical-flow strip was not seen running on screen
(the prepare run baked that clip first; the strip is the same element as
the other two); a user template whose own media is copied out, applied
into a project (covered by the copy-out path `template_build_project`
shares with it).

## UX gaps, 2026-10-04 (`agent/ux`)

Seen in the debug app on Xvfb (lavapipe, `CHUKCUT_FILE_DIALOG=builtin`, no
session bus, `HOME` and every `XDG_*` folder under `_scratch/`) with a
generated 1920×1080 clip:

- **Video › Crop.** 9:16 put a 608×1080 box in the middle of the whole
  picture, the rest dimmed; dragging inside the box moved it live, and on
  release the clip on Basic showed the cropped part fitted to the canvas.
  Undo is one step (the crop is one `inspector_set_crop`).
- **Video › Basic.** "Reduce image noise" ticked adds the denoise effect
  with Strength and Keep detail (50 / 50); "Enhance quality" and "Optical
  flow" show a line and a button that opens their tab.
- **Speed › Speed effects.** Smooth montage lit its tile, the clip got the
  Montage ramp (6 s → 8.8 s on the timeline, "Montage ·" on the clip) and
  frame blending; Ctrl+Z took both back at once.
- **Settings › Performance.** Video decoding "Software" wrote
  `"decode": "software"`, and the next decoded frames logged
  `path=Software`; AI runtime lists Automatic and the three packs, marked
  "(not installed)" in the empty test home, and wrote `"ml_runtime": "cpu"`.
- **Export queue.** Two 4K exports queued; Ctrl+Q asked "Export running —
  quit anyway?", Quit anyway led to the unsaved-changes prompt, and Don't
  save quit the process within a second. The next start showed "2 exports
  from the last session are waiting" with Run now in the queue; Run now
  started them.
- **Packaging.** `packaging/tarball.sh --no-build` with stand-in binaries
  packed `bin/chukcut`, `bin/chukcut-ml-worker` and `bin/chukcut-cli`;
  `scripts/install.sh` from the unpacked tarball installed and uninstalled
  all three into a scratch `--bindir`.

Not tested, and why: a real release tarball (a release build of all three
takes long, and the stand-ins exercise the scripts); Hero moment and Bullet
time on screen (they start an optical-flow bake, covered by the Smooth
slow-mo tests); the crop box on a rotated or stabilised clip (the overlay
uses the compositor's placement chain, and its pixel ↔ source mapping has a
unit test).

## QA pass 3, 2026-10-04 (`agent/qa3`)

The features of waves 10 and 11, end to end in the release build: the app
on a private Xvfb display (`:171`, lavapipe, `CHUKCUT_FILE_DIALOG=builtin`,
no session bus, `HOME` and every `XDG_*` folder under `_scratch/qa/`, ALSA
on a null device), the CLI and the ML worker on the RTX 3060 (CUDA 13
bundle, TensorRT add-on, Fast mode on). The ML folder was linked into the
isolated cache; nothing was downloaded again. Generated media and NASA's
public-domain portraits only. Per feature: does it render, is it one undo
step (a batch with the operation and one `undo` gives back the document
byte for byte, and Ctrl+Z in the app), does it survive save and reopen,
does the export match the preview (`render-frame` against the frame
decoded from the exported file, mean difference in code values).

| Feature | Renders | Undo | Save / reopen | Export = preview |
|---|---|---|---|---|
| Remove object, click (MobileSAM + VitTrack + clean plate), 90 frames of 1280×720 | yes, the red box gone, 19 s | one step (CLI; app: tick off, Ctrl+Z) | yes; frames re-made on open after the cache was cleared | 2.0 (mandelbrot detail) |
| Remove object, box (LaMa every frame), 60 frames of 270×480 in the showcase | yes, the "watermark" gone | one step | yes | 1.7 |
| Enhance quality 2x (640×360 → 1280×720), 4x in the showcase | yes, 90 frames in 6 s (Fast) | one step | yes; "Made at 1280×720" in the app | 1.7–1.8 |
| Auto adjust | yes ("exposure +0.36 …") | one step (CLI, app) | yes | 1.5 (showcase at 2 s, with glow) |
| Colour match | yes, L\*a\*b\* 13.9 → 2.6 in 4.4 s | one step | yes | 0.9–2.1 |
| Grade presets (save, apply to another project) | yes, the same render as the clip it was saved from (0.0) | one step | presets in `XDG_DATA_HOME` | |
| Retouch (Sculpt) | yes | one step (CLI; app: Natural, Ctrl+Z back to Sculpt) | yes; faces re-found on open | 1.5 |
| Follow face (forehead) | yes, the title moves with the face | one step | yes | 1.5 |
| Body landmarks, follow body part (left hand) | yes, 120 frames in 2.3 s | one step | yes | 1.3 |
| Auto reframe with people | 9:16 on a person with the head covered: "faces in 15 % · people in 85 %", he stays in the window | one step | yes | 1.5 |
| Isolate voice (HTDemucs) | speech SDR 1.5 → 15.8 dB in the WAV export; 8 s in 11.8 s; worker peak 1.47 GB | one step (CLI; app: Medium, Ctrl+Z) | yes | the export is the isolated sound |
| Speed effects (Montage, Hero with RIFE TensorRT at 1080×1920, Flash in) | yes; Hero 264 new frames in 113 s incl. 84 s TensorRT preparation | one step (CLI, app) | yes | 1.7 |
| Crop (CLI box, app 9:16) and GPU Reduce noise | yes, the cropped part fills the clip's frame | one step | yes | 0.5 |
| Stabilise on a compound clip | yes, "cropped 14 %" | one step | yes | 1.5–1.7 |
| Template into an open project (compound at playhead, new timeline) | yes, app dialog and CLI | one step (app: the lane goes too) | yes | 1.3–1.6 |
| Slots inside a compound clip (`template replace --clip slot:2`) | yes | one step (the file stays in the media library, as an import does) | yes | 1.3 |
| Prepare on open | "Preparing 301 frames" after the remade frames and the face track were deleted; the showcase: "Preparing 451 frames and 1 voice" | | | |
| Fast mode, TensorRT | Settings shows the switch, the add-on (3.0 GB) and "TensorRT · fp16 · prepared in …" per model | | | |
| Decode and AI runtime pickers | Settings › Performance lists Automatic, VAAPI, NVDEC, Software and the three ONNX Runtime packs | | | |
| Export queue, quit guard | Add to queue, Ctrl+Q: "Export running — quit anyway?", Keep exporting finished the file | | queue file kept the done item | app export vs `render-frame`: 0.4–1.5 |
| Installed layout (`scripts/install.sh --no-build --bindir _scratch/install/prefix/bin`) | the installed app found `prefix/bin/chukcut-ml-worker` with `PATH=/usr/bin:/bin` and analysed faces with it; `chukcut-cli ml status --probe` reported the worker beside it, also through a symlink on `PATH` and from `<prefix>/libexec/chukcut/` | | | |
| MCP | 154 tools in `tools/list` | | | |

Fixed on this branch:

1. **Misspelt arguments in a batch file or an MCP call were ignored.**
   `{"op":"remove_object","point":[…]}` (the field is `points`) ran without
   the click and answered "the clip has no object removed" as a success.
   Serde skips unknown fields, and `deny_unknown_fields` does not work with
   the flattened argument groups, so the names are now checked against the
   schema the MCP server publishes: "unknown argument "point"; the
   arguments are: …".
2. **A short clip's faces or people were analysed again on every
   request.** The landmark and body coverage stepped a rounded period
   (33 333 µs) while `t < end` and counted one step past the last frame (30
   of 31 for a 1 s clip at 30 fps). Below the 98 % done share, a clip
   shorter than about 1.6 s was never done, so every `face-landmarks`, the
   preparation on open and each export ran the analysis again. Steps now
   count while a frame can start in them; both modules have a test.
3. **The crop ratio stayed lit after an undo.** 9:16, then Ctrl+Z: the box
   went back to its free shape but 9:16 stayed selected, and the next handle
   drag would have forced 9:16 again. A picked ratio now holds only while
   the crop has its shape.
4. **`ml status` and Settings listed an engine the model never uses**: a
   leftover fp16 RIFE engine next to the fp32 ones ("input-1x6x180x320"
   twice, "5 sizes"). Only engines of the planned precision are listed.
5. **The CLI said nothing when an import changed the canvas.** `new --width
   1280 --height 720` and a 1280×720 import gave a 1920×1080 project (by
   design: the first clip sets an unchosen canvas, `configure` chooses it).
   The import's answer now says "the canvas took the clip's shape:
   1920x1080 at 30 fps (`configure` changes it)".
6. `speed-effect` without options now lists the effects in its answer, as
   `docs/cli.md` says.
7. Manual: isolate voice said the worker uses 7 to 8 GB (1.5 GB since
   agent/mlspeed); Fast mode and the TensorRT add-on were not described in
   Settings or AI tools; the GPU costs did not have the Fast numbers;
   Troubleshooting did not name the `libexec` worker places or the
   "Preparing TensorRT" wait.

Open:

- ~~**Medium: `chukcut-cli export` crashed once with a segmentation fault
  after the export was written.** In one of four runs of `scripts/demo.sh
  --export` (load 17–22 from `cargo test` runs), the CLI printed "exported
  …/showcase.mp4 (831 frames, 29.7 MB) in 44.7 s" and then died with
  SIGSEGV (exit 139), so the file was complete but the script stopped. Not
  reproduced in 12 more exports of the showcase under gdb (with and
  without `--sidecar srt`) nor in 12 of a 3 s project. It looks like
  teardown at exit (the Vulkan device, the export threads); STATUS.md "The
  hang that was not the device" has the earlier device-teardown crashes.
  Steps: `scripts/demo.sh --export` in a loop under load, or `chukcut-cli
  export _scratch/demo/showcase.chukcut out.mp4 --preset user_showcase
  --sidecar srt` in a loop.~~ Fixed on agent/shutdown, cause found: the
  export thread sent "done" and only then dropped the job, whose decoders
  free cached GPU textures in the Vulkan driver; the CLI returned from
  `main` meanwhile, and libc's `exit` ran the NVIDIA libraries' destructors
  under that thread. Reproduced at 5 crashes (and 1 hang in the NVDEC
  library's destructor) in 90 exports, six at a time on four cores; the
  cores all show the export thread in `drop(ExportJob)` → `TextureView`
  → `libnvidia-glcore` while the main thread is in `_dl_fini`. The job is
  now dropped before "done" is sent, and the CLI and the app leave through
  `lifecycle::exit` (stop the exports and the ML worker, flush, `_exit`).
  After: 0 in 180 under the same load. STATUS.md, "The crash after the
  export".

Open (low; none blocks a feature):

- ~~**A project path on the command line that cannot be read** opens the
  start screen with no message; the reason is only on stderr ("cannot read
  /aq.chukcut: No such file or directory"). Steps: `chukcut /nonexistent.chukcut`.~~
  Fixed on agent/shutdown: the start screen shows the reason (and the
  editor's status line does, when media files came with it).
- ~~**`template slots` numbers the slots per sequence and does not name the
  open timeline**: with Quick Cuts as a compound clip and Travel Diary as an
  opened timeline, the list has two "slot 1" lines; the compound clip's say
  `in compound clip "Quick Cuts"`, the timeline's say nothing. `--clip
  slot:N` picks the open timeline's.~~ Fixed on agent/shutdown: the numbers
  run across the project (each sequence's slots together), every line says
  where its slot is when there is more than one place ("on the open
  timeline "Travel Diary""), and `slot:N` takes those numbers. JSON keeps
  the fill order as `index` and adds `number` and `open_timeline`.
- ~~**The CLI names clips by their full id** in some answers ("matched to
  8a2966a5-a61a-…", "follows the forehead of the face in bf34ba1c-…")
  where `info` uses the short prefix or the file name.~~ Fixed on
  agent/shutdown: `colour-match`, `follow-face` and `follow-body` say
  "8a2966a5 (portrait.mp4)", as `info` lists it.
- ~~**Remove object keeps its estimate line** ("90 frames: about 23 s on a
  GPU, 4 min on the CPU") under "All 90 frames are made."~~ Fixed on
  agent/shutdown: the estimates of Remove object and Enhance quality go
  once every frame is made.
- ~~**The app starts a session bus of its own** when
  `DBUS_SESSION_BUS_ADDRESS` is unset (`dbus-launch --autolaunch`, seen
  after an export finished on Xvfb); the `dbus-daemon` and the AT-SPI bus
  stay after the app quits. Only matters for test displays.~~ Fixed on
  agent/shutdown: it was the "export finished" desktop notification —
  `notify-send` is GLib, and GLib autolaunches a bus on X11 when it finds
  none. The notification is now skipped without a session bus (no
  `DBUS_SESSION_BUS_ADDRESS` and no `$XDG_RUNTIME_DIR/bus`).

Not tested, and why: playback with sound, voiceover recording, VAAPI and
QSV (no Intel or AMD GPU here), the portal file chooser, cloud providers;
Paint on player and Select on player for Remove object (covered by the
ml4 pass; here through the CLI); the people fallback of auto reframe on a
clip where YuNet finds nothing at all (here 15 % of frames had a "face").

## Robustness, 2026-10-05 (`agent/robust`)

What is tested and how to check it again. Decision 0034 has the policy.

| Check | Test | GPU |
|---|---|---|
| The streamed mix is bit-identical to the old whole-buffer mix (busy project, ranges, block sizes, 1 and 2 channels, unclamped) | `export::audio::tests::the_streamed_mix_is_the_buffered_mix_bit_for_bit` | no |
| A file read in pieces is the same samples as one read (AAC, 44.1 kHz) | `tests/export_audio_stream.rs` `reading_a_file_in_pieces…` | no |
| A 2 s range of a 3 h timeline peaks at 2.3 MB of allocations (bound 64 MB) | `tests/export_audio_stream.rs` `a_short_range…` | no |
| The same through `run_export` to a WAV | `tests/export_audio_stream.rs` `a_range_export…` | yes |
| A clip at 9·10¹⁸ µs: no allocation abort, never decoded | `export::audio::tests::a_clip_at_an_absurd_time…` | no |
| The panic hook reports message, thread, place and backtrace | `lifecycle::tests` | no |
| The player stops on a panic, says why and restarts | `preview::player::tests::a_panic_in_the_render_thread…` | yes |
| The queue fails a panicking item and goes on; a panic outside the export does not leave the queue stuck | `export::queue::tests` | no |
| An MCP request that panics answers `-32603` and the next request is served | `chukcut-cli` `mcp::tests::a_panicking_request…` | no |
| An ML worker request that panics answers an error | `chukcut-ml-worker` `tests::a_panicking_request…` | no |
| A panicking bake fails only that bake | `tests/matting.rs` `a_panic_in_a_bake…` | no |
| A panic while preparing a clip is a failure on the status line, and the next run works | `tests/prepare.rs` `a_panic_while_preparing…` | no |
| Canvas 0 or 100000, fps 0 or 100000, speed 0, a clip at 9·10¹⁸ µs, near `i64::MAX`: errors, no overflow | `project::document::tests::absurd_canvases…` | no |
| The CLI refuses `export` and `render-frame` on six damaged files; `validate` still opens them | `chukcut-cli` `tests/guard.rs` | the last step |
| Undo gives back the canvas the first clip set; a 1 fps clip keeps the project at 30 fps and says so | `project::commands::tests` | no |
| A job's commit for an older generation is refused and changes nothing | `state::tests`, `analysis::commands::tests::an_analysis_for_a_closed_project…` | no |

End to end against the master release CLI (`_scratch/cmp/cmp.sh` in the
worktree): a whole-project WAV export and a 1.3–4.7 s range export of a
two-lane project (a video clip at volume 0.7 and an MP3) are byte-identical.

**Not verified:** the in-app Restart button and the open/import notices were
not looked at on screen. The loop that should show the exit SIGSEGV of
`tests/export.rs` gone was cut short when the NVIDIA driver hung (STATUS,
"Robustness"). Run it again after the reboot, one binary at a time:
`scripts/loop-test.sh target/debug/deps/export-<hash> 40 _scratch/loop.txt`.
