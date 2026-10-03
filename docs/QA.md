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

## Still broken or missing

### High

- **There is no text styling UI.** A title's inspector has Video, Animation,
  Tracking and Effects; nothing sets the font, size, colour, stroke, shadow or
  box. The engine has it all (`text_set`); the wave-3 "text & titles" row in
  the plan was never run. Captions are styled in the Captions tab, titles not
  at all.
  Steps: Text tab → Default text → select the title → look for a font or
  colour control.
- **No way to save a new project or choose any file without a desktop
  portal.** Save as, Open, Import, the export folder and caption import all go
  through `xdg-desktop-portal`; when it is missing (minimal window managers,
  some i3/sway setups) the status line says "File dialog failed" and a new
  project can never be saved. The export folder field is read-only, so exports
  then always go to `~/Videos`. A typed-path fallback would close this.
  Steps: run without a portal (e.g. `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent`),
  new project, Ctrl+S.

### Medium

- **The canvas the user picked is replaced by the first clip's shape.** The
  start screen preselects 9:16; importing a 16:9 clip into the empty project
  silently makes it 1920×1080 (`import_material`, "canvas adopted from the
  first imported clip"). Fine when the user did not choose, wrong when they
  clicked 9:16 on purpose. It needs a "chosen" flag in the document, so it is
  left for the owner's call.
  Steps: start screen → 9:16 → New project → import a landscape clip.
- **Clicking the ruler puts the playhead between frames**, and split and
  freeze frame cut at that exact microsecond (a cut at 3.016761 s, not at
  frame 90). Export samples by frame, so the result is right, but clip edges
  off the frame grid make later frame-accurate edits and the timecode
  readout disagree.
  Steps: click the ruler near 00:03, split, read the left clip's duration in
  the saved file.
- **Grade edits leave unreferenced colour materials in the saved file.**
  Every grade commit mints a new `ColorAdjustMaterial`; the old one stays in
  the pool (3 of 4 unreferenced after four grade edits in the end-to-end
  test). Undo needs them while the session lives, but the file keeps them for
  ever. Titles and effects do the same, by design ("an unreferenced material
  is inert"). A prune of unreferenced colour and effect materials at save
  time (file only, not the live pool) would keep files small.
- **Karaoke does nothing on captions imported from an .srt**, because an SRT
  has no word timing (`Cue::words` is empty and the Captions tab says so).
  Estimating words over each cue, as already done for providers without word
  timing, would make it work.

### Low

- The restore prompt offers to bring back an empty "Untitled" (0 clips) and
  shows even when a project was opened from the command line.
- The title bar's "Autosaved … ago" does not move after a transition edit.
- The export dialog does not remember the last folder, resolution or loudness
  target, and its size estimate was 24 MB for a 3.4 MB file.
- Settings → Hardware says "Zero-copy decode: Yes" on a machine where no
  VAAPI decoder works (it reports the adapter's DMA-BUF import, not whether
  any decoder can use it).
- The title bar keeps the last error ("b_disc.mp4: cannot open …") until the
  next status message.
- A tooltip that is open while its button changes keeps the old text
  (the effect's eye shows "Turn off" after it was turned off).
- Menu has no New project shortcut hint although Ctrl+N works.

## Not tested, and why

- File chooser flows (Import button, Open, Save as, caption import/export,
  Save frame as image, export folder): no portal on Xvfb.
- Real audio output and real-time playback: the null ALSA device consumes
  samples at once, so the audio clock runs far faster than real time. The
  player itself is the performance agent's (`player.rs`); nothing there was
  changed or judged.
- VAAPI decode/encode and QSV: no Intel or AMD GPU in this machine.
- Cloud providers with real keys (TTS, sound, music, stock, fal.ai, DeepL).
  With a fake ElevenLabs key the voice list fails cleanly with "HTTP 401".
