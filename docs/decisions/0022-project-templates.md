# 0022 — Project templates: a project with slot markers, in a directory

Status: accepted (2026-10-04, templates agent)

## Decision

A **template is an ordinary project** whose picture clips are **slots**. A
slot is a clip that carries a marker: a `template_slot` entry in
`MaterialPool::extras`, referenced from the clip's own `extras`, the way
analysis results are (decision 0019). The marker holds the fill order
(`index`, from 1), an optional `label`, the slot's picture shape (`aspect`,
reduced, e.g. `[9, 16]`), what it `accepts` (`any`, `video`, `image`) and
whether it is `filled`. Until it is filled, the clip's material is a
placeholder picture we draw: a dark field, an aqua frame and the slot number.

On disk a template is a **directory**:

```text
<data>/templates/user/<id>/
  template.json     {"format": "chukcut-template", "version": 1, id, name,
                     description, category, tags, cover_time, project}
  media/            files the project uses that are not ours, copied in
```

`project` is a whole `.chukcut` document. It is read through
`project::migrate`, so a template from an older build opens like an old
project. A **relative media path is relative to the template's directory**;
our own placeholders, music beds and looks stay absolute paths under
`<data>/templates/` and `<data>/luts/`, and are drawn again when missing
(`template::assets::ensure_for`), so they are never copied.

The built-in templates are **not files**. They are built in code
(`template/builtin.rs`) with the same edit builders the commands use, from our
own title styles, animations, effects, transitions, looks, placeholders and
synthesised music beds (`template/music.rs`). No ByteDance asset is involved.

**Filling a slot** keeps the slot's place, length, transform, animations,
effects, transitions and keyframes and changes only the material, the source
range, the speed and the crop (`template/fill.rs`):

- a longer clip is trimmed (from its start, or from a chosen start);
- a shorter clip is slowed down until it spans the slot (never a gap — the
  template's cuts sit on its music), refused below 0.01×;
- a picture of another shape is cropped, centred, to the slot's shape, so it
  lands exactly where the placeholder was;
- a speed ramp on the slot is dropped.

"New project from template" fills the slots in index order and leaves the rest
showing their placeholders; more files than slots, a file that does not read,
or a photo in a video-only slot is refused by name. "Replace media" is the same
fill as one `EditCommand` (`Composite` of `RemoveSegment` + `InsertSegment`,
the marker entry inserted into the pool first) and works on any picture clip.
"Save as template" works on a copy: the chosen clips become placeholders of
their displayed shape, their linked sound and unused media go, and the rest of
the media is copied into `media/`.

## Why

- **A slot is a clip, not a new kind of thing.** Everything a template wants
  to carry — an In animation on slot 3, a transition into slot 4, a LUT on
  every slot — is already a clip property. A separate "slot" type would need
  its own copy of each of them and of every edit on them.
- **The marker lives in `extras`, not on `Segment`.** No schema change, no
  migration, and every other clip in the project saves byte for byte as
  before. `prune` keeps the marker because the clip names it.
- **A project inside a manifest, not a template language.** A template is
  edited in the editor and saved from it; a format with its own vocabulary
  would need a second editor or a translator and would lag behind every new
  engine feature. The cost is that a template is as large as a project.
- **A directory, not a single file.** Media has to travel with a template
  (a logo, a sound) and copying it into a zip on every save is slower and
  harder to inspect than a folder.
- **Built-ins in code.** Ids and parameters are checked by the compiler and by
  `every_builtin_builds_into_a_valid_project_with_slots`; a template made of
  JSON would rot silently when a preset is renamed.
- **Slow down rather than leave a gap or loop.** A gap breaks the beat and
  every cut after it; a loop shows the same second twice. Slowing is what
  CapCut does and is one speed value.

## What it costs

- Templates in a user's data directory point at our assets by absolute path.
  Moving a template to another machine works (the assets are redrawn under
  that machine's data directory only when the path matches); a template
  copied between users with different home directories keeps the old
  absolute paths for our assets. Bundling them too would fix it at a few
  hundred kilobytes per template.
- A project made from a user template references the template's `media/`
  folder by absolute path. Deleting the template takes those files away
  from the project (they go offline, decision 0009).
- A slot carries picture only. The user's clip's own sound plays at the
  slot's volume, which the built-ins set to 0 under music (and 1 for the
  talking-head and reaction templates).
- Splitting a filled slot gives both halves the same marker; the fill dialog
  then lists the slot twice. Harmless, but untidy.

## What would change our minds

- Users exchanging templates often: then a single-file `.chukcut-template`
  (zip of the directory) with our assets inside.
- A slot that wants its clip's sound and the music at once (ducking under
  it): then a `sound` field on the marker that the fill turns into a volume
  and a duck.
