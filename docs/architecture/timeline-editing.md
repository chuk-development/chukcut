# Timeline editing

Every mutation of a project goes through an `EditCommand`. There is no other
sanctioned path — not in the frontend, not in another Rust module. Code lives
in `src-tauri/src/modules/timeline/`.

## Why commands and not snapshots

Undo could be implemented by snapshotting the document before each edit. It
would be less code. It is the wrong trade:

- A snapshot of a project with hundreds of clips is orders of magnitude larger
  than the delta.
- A snapshot cannot tell the user *what* undo will reverse. "Undo Split clip"
  requires knowing the edit, not the before-picture.
- Snapshots hide bugs. If an edit corrupts the document, snapshot-undo restores
  it and nobody notices until the corruption survives a save.

A command that must be able to invert itself is a command that has to be honest
about what it changed.

## The shape

```rust
enum EditCommand {
    AddTrack     { track, index },
    RemoveTrack  { track, index },
    InsertSegment{ track_id, segment, index },
    RemoveSegment{ track_id, segment, index },
    MoveSegment  { segment_id, from_track, to_track, from_start, to_start },
    TrimSegment  { segment_id, before_target, before_source,
                                after_target,  after_source },
    SetTransform { segment_id, before, after },
    SetSpeed     { segment_id, before, after },
    SetVolume    { segment_id, before, after },
    Composite    { label, commands },
}
```

Every variant stores **both sides** of the change. `RemoveSegment` carries the
entire segment and its index, because undo has to put it back exactly where it
was. `SetSpeed` carries the old speed, because deriving it later is impossible.

`invert()` is then mechanical and total — there is no command that can be
applied but not undone.

## Composites

Higher-level edits are composed, not added as primitives:

- **Split** = trim the original to the cut point + insert a new segment for the
  remainder. `split_at()` builds this.
- **Ripple delete** = remove + move everything after it left.
- **Paste** = insert, possibly several.

A `Composite` applies its parts in order and undoes them in reverse, so a split
is one entry in the undo stack even though it touched two segments.

If applying a composite fails partway, the parts that already applied are
inverted before returning the error. A failed edit leaves the document exactly
as it was.

## Invariants

Enforced by the commands, checked by `Project::validate()`:

1. Segments within a track are sorted by `target_range.start`.
2. Segments within a track never overlap.
3. `render_index` is derived from track order, recomputed centrally by
   `reindex_render_order()` after any structural change. No command sets it
   by hand.
4. Every `material_id` resolves in the pool.

An edit that would break 1 or 2 is rejected with an error rather than applied
and repaired. "Target range is occupied" is a legitimate outcome of dragging a
clip onto another one; the UI is expected to show it, not to have prevented it.

## Snapping (frontend)

Snapping is a *UI* concern and lives in the frontend: while dragging, the
timeline computes candidate positions (clip edges, playhead, markers, second
boundaries) and snaps the ghost position before the drag ends. Rust receives
the already-snapped value.

Putting it in Rust would mean a round trip per mouse-move. Putting it in the
frontend means the drag is smooth and Rust still validates the result.

## What the frontend sends

One command per completed gesture, not per mouse-move. A drag emits a single
`MoveSegment` on release with `from_start` and `to_start`. The intermediate
positions are local UI state and never reach Rust.

The response (`EditResponse`) carries the whole updated project plus the undo
and redo labels, and the frontend store replaces itself with it.
