# 0024 — Several timelines per project, and compound clips, as swapped sequences

Date: 2026-10-04. Status: accepted.

## Decision

A project holds **sequences**. A sequence is a stack of lanes and a list of
markers — exactly what `Project::tracks` and `Project::markers` are. Two kinds
share one type (`modules::sequence::Sequence`): a **timeline** is a tab
(CapCut's "Timeline 01"), a **compound** is what "Create compound clip" makes.

One sequence is **active**. Its lanes *are* `Project::tracks`; which one it is
lives in `Project::sequence` (`ActiveSequence`: id, name, kind, its slot in the
tab order, and the breadcrumb path of sequences entered on the way). Every
other sequence is **parked** in `MaterialPool::sequences`. Switching timelines,
opening a compound clip and closing it all swap a parked sequence into
`Project::tracks` and park the old one (`sequence::edit::activate`).

A sequence is a **material**. A segment whose `material_id` names a parked
sequence is a compound clip (`MaterialKind::Sequence`); its source range is a
range of the nested sequence's time, and speed, trims, splits, transitions,
effects, grades, masks and keyframes work on it as on a video.

Everything is an edit. `EditCommand::Sequence { edit }` carries four
primitives (`SequenceEdit`: `Add`, `Remove`, `Rename`, `Activate`); every
gesture — new, rename, delete, duplicate and switch timeline; create, open,
close and flatten a compound clip — is a `Composite` of those and the ordinary
segment and lane commands, built in `sequence/build.rs`. Navigation is on the
undo stack.

Old files round-trip byte for byte: `Project::sequence` is not written while
it is the default (the main timeline, id `main`, "Timeline 01"), and
`MaterialPool::sequences` is not written while empty. `SCHEMA_VERSION` stays 1.

### Rendering

The compositor draws a compound clip by rendering its sequence, whole, at the
clip's source time into a texture of the frame's size (`Compositor::
nested_frame`) and drawing that as the clip's source frame. The nested render
clears to transparent, so a compound clip shows what is under it where its own
lanes are empty. That texture holds premultiplied colour; the quad that draws
it carries a new grade feature bit, `PREMULTIPLIED`, and `quad.wgsl` divides
by alpha before anything else, so `fs_premultiplied` multiplies it back once.
Depth is bounded by `sequence::MAX_DEPTH` (8), counted per thread.

### Sound

Sound is a sum, so it is taken apart instead: both mixers (the preview's
`audio::mixer::plan`, the export's `export::audio::mix_timeline`) are given
`sequence::audio::flatten_audio(project)`, a copy in which each compound clip's
sound-bearing clips sit on lanes of their own, moved, cut to the window and
retimed into the outer timeline. Position, the window, constant speed, clip
and lane volume, mutes and the inner clips' volume keyframes map exactly; the
compound clip's own volume keyframes multiply in, and its speed curve is
composed with each inner clip's speed (`sequence::retime`, exact unless both
are curves). A project without compound clips is borrowed, not copied.

### Export, proxies, thumbnails

The export renders the **root timeline** even while a compound clip is open in
the editor (`sequence::export_root`, applied in `export::commands`). Missing
media inside compound clips is reported (`export::job::missing_media`
recurses). Proxies are per material and pool-wide already, so media inside a
compound clip gets its proxy like any other. The timeline draws a compound
clip's filmstrip as nested renders of its sequence (`sequence::thumbs`),
cached under a digest of its contents.

### Cycles

`EditCommand::InsertSegment` — the way every clip, a paste included, enters a
lane — refuses a clip whose sequence contains the active one, is on the
breadcrumb path, or would nest past `MAX_DEPTH` (`sequence::check_insert`).
`SequenceEdit::Add` refuses a sequence that shows itself. `Project::validate`
reports cycles, depth, duplicate sequence ids and the ordinary lane errors of
every parked sequence.

## Why swap the active sequence into `Project::tracks`

Because everything already works on `Project::tracks`: about sixty files —
every edit command, the timeline UI, the inspector, captions, analysis,
tracking, silence cutting, the preview and the export. The alternative, a
sequence id threaded through all of them, is weeks of change across modules
other agents own, and every place that forgets the id edits the wrong lanes.
Swapping makes "the timeline being edited" the only timeline any of that code
can see.

## Why navigation is an edit

Every command on the undo stack names segments and lanes of the sequence that
was active when it was made. Undoing an edit made inside a compound clip only
works while that compound clip is open again. With navigation on the same
stack, undo walks back out through the close, undoes the edit inside, and
walks back in — in step by construction. A separate per-sequence stack would
have to answer what Ctrl+Z means after an edit outside that deleted the
compound clip.

## Why the tab order is a slot

The order of all sequences is the pool list with the active one inserted at
its slot. A switch takes the target out of that list and parks the old one in
its place, so the list is the same before and after, and a switch followed by
its inverse leaves the pool byte-identical. Tabs never jump around.

## What it costs

- **A nested render per compound clip per frame**, and a texture that is not
  pooled (the source frame still refers to it). The nested view (the pool
  clone) and the nested frame are cached per digest of the compound clip's
  contents (`render::nested`, `sequence::digest`, added 2026-10-04), so a
  paused frame, or two clips of one sequence, render the inside once.
  Playback still renders the inside every frame: each frame is a new inner
  instant.
- **Flattening** composes the compound clip's speed into the clips inside
  (constant or curve) but refuses a curve under a curve, and a speed-curved
  clip cut at the window's edge. What the compound clip itself carried —
  transform, grade, effects, its own volume keyframes — goes with it.
- **An older build opening a multi-timeline project drops the parked
  timelines** on its next save: the format is additive, as every feature so
  far has been, and an old build ignores keys it does not know. The schema
  version was not bumped because that would rewrite every existing file.
- **Deleting a timeline** removes the compound sequences only it reached, in
  the same composite, so undo brings them back.
- ~~**Picture analyses** (scenes, beats, reframe) see the active sequence only.~~
  Silence cutting and loudness on a compound clip measure its contents' mix;
  captions and the mix loudness hear compound clips through the mixer.

## Amendment, 2026-10-04 (agent/compound3)

- **Analyses read a compound clip's contents.** Scene detection and auto
  reframe render its sequence (`analysis::frames::Walk::sequence`), beat
  detection mixes it. Results are stored in the sequence's time, which is
  the compound clip's source time, so they ride its time map like a video's
  results ride the file's. For reframing, a compound clip is a picture of its
  contents' shape (the largest full-frame picture inside), because its
  rendered texture is always canvas-sized.
- **A compound clip that processes its own sound is mixed down.** Taking the
  sound apart (above, "Sound") is exact for gains and speeds but not for a
  compressor, a reverb, noise reduction or a loudness gain measured on the
  sum. Such a clip is heard through a cached mix-down of its sequence
  (`sequence::bounce`), an audio file in sequence time, which the existing
  cleanup, effect and speed renders then treat as the clip's source. The
  export and every measurement render it first; the preview waits for a
  background render and plays the contents dry meanwhile, the rule
  `audiofx::cache` already follows. Cost: a full decode and mix of the
  sequence per content change, and one more cached file. What would change
  it: long compound clips with effects edited often from the outside — then
  mix down only the window the clip shows.
- **The nested-render digest includes file identities** (size and
  modification time of every path its entries name), so files edited or
  restored on disk invalidate cached nested frames without a document edit.

## What would change our minds

- Deep nesting in real projects making the per-frame clone show up in the
  preview profile — then cache the nested views per document generation.
- A need to edit two timelines side by side (two panels) — then the active
  sequence stops being unique and the sequence id has to be threaded after
  all.
- A format change for another reason — then bump the schema so old builds
  refuse multi-timeline files instead of dropping timelines.
