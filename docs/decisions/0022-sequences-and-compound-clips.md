# 0022 — Several timelines per project, and compound clips, as swapped sequences

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
and lane volume, mutes and the inner clips' volume keyframes map exactly. A
project without compound clips is borrowed, not copied.

### Export, proxies, thumbnails

The export renders the **root timeline** even while a compound clip is open in
the editor (`sequence::export_root`, applied in `export::commands`). Missing
media inside compound clips is reported (`export::job::missing_media`
recurses). Proxies are per material and pool-wide already, so media inside a
compound clip gets its proxy like any other. The timeline draws a compound
clip's filmstrip from the clips inside it (`sequence::picture_at`, recursive),
out of the same thumbnail caches as every other clip.

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

- **A nested render per compound clip per frame**, plus a clone of the pool
  and the sequence's lanes to build its view, and a texture that is not
  pooled (the source frame still refers to it). Fine at the depths people use;
  measure before nesting deep compound clips in a long project.
- **A compound clip's own volume keyframes and its own speed curve do not
  reach the sound** (the picture follows the curve; the sound plays at the
  clip's constant speed and plain volume).
- **Flattening needs normal speed** on the compound clip, and refuses to cut
  a speed-curved clip at the window's edge. What the compound clip itself
  carried — transform, grade, effects — goes with it.
- **An older build opening a multi-timeline project drops the parked
  timelines** on its next save: the format is additive, as every feature so
  far has been, and an old build ignores keys it does not know. The schema
  version was not bumped because that would rewrite every existing file.
- **Deleting a timeline leaves its compound clips' sequences in the pool**,
  unused, so undo finds them.
- **Analysis, silence cutting, captions placement and loudness measurement**
  see the active sequence only; they do not look inside compound clips.

## What would change our minds

- Deep nesting in real projects making the per-frame clone show up in the
  preview profile — then cache the nested views per document generation.
- A need to edit two timelines side by side (two panels) — then the active
  sequence stops being unique and the sequence id has to be threaded after
  all.
- A format change for another reason — then bump the schema so old builds
  refuse multi-timeline files instead of dropping timelines.
