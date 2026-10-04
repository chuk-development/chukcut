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
