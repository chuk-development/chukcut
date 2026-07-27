# 0008 — One undo stack, two command kinds

**Decided:** 2026-07-27, while building the project-settings dialog.

## What was decided

Project-level settings — name, canvas size, fps, canvas background — are edited
by a new invertible command, `project::ConfigureCommand`, which lives in the
**project** module and shares one undo stack with every timeline edit through
`state::DocumentHistory`. `AppState.history` is now a `DocumentHistory`, an
enum-of-two-kinds history (`EditCommand` | `ConfigureCommand`) with the same
method names and signatures `timeline::History` had, so no caller changed.
`timeline::History` itself is untouched and still exists.

## Why not a new `EditCommand` variant

That was the obvious design: `EditCommand::ConfigureProject { before, after }`
in `timeline/ops.rs`, and `History` needs nothing at all. It was not done
because the timeline module is owned by other concurrent work and this
repository has already paid, in a documented night of twelve agents, for two
streams of work editing one file. An enum variant is exactly the kind of edit
that conflicts: it touches the enum, `label`, `apply`, `invert`, and the
`serde` wire shape in one place.

If the timeline ever grows a document-level variant of its own, collapsing
`DocumentHistory` back into `History` is mechanical: move `ConfigureCommand`'s
three methods into the variant, delete `state::DocumentHistory`, point
`AppState.history` back at `History`.

## The cost, written down so it is checked rather than discovered

`DocumentHistory::apply` **reproduces** the two pre-apply expansions
`History::apply` performs (`ops::mirror_linked_edits`, then
`ops::detach_broken_transitions`), because they are what make linked clips move
together and transitions detach when their cut disappears. If `History::apply`
ever gains a third expansion, `DocumentHistory::apply` must gain it too — the
comment on `DocumentHistory` says so, and this file is the second place. The
symptom of forgetting would be: an edit made through the menu bar behaves
differently from the same edit made in the timeline panel.

## What would change our minds

- The timeline team adding a document-level command family themselves — then
  fold back as above.
- A third kind of document command appearing (markers? per-project settings
  beyond these?) — then `DocumentCommand` is already the extension point, and
  the pressure to move it into the timeline enum drops further.
