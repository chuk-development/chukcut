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

    SetLinkGroup { segment_id, before: Option<Id>, after: Option<Id> },

    AddKeyframe       { segment_id, property, keyframe },
    RemoveKeyframe    { segment_id, property, keyframe },
    MoveKeyframe      { segment_id, property, from_time, to_time,
                        before_value, after_value },
    SetKeyframeEasing { segment_id, property, time, before, after },

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

## Expansions: the two things that depend on more than one segment

A command names one segment, and two things in the document do not fit in one.
Both are handled the same way — the command is expanded into a `Composite`
inside `History::apply`, before it is recorded — because that is the single
point every edit passes through, so no caller can forget.

- **Linked clips.** A file imported with sound is two segments on two lanes
  (decision `0005`). `ops::mirror_linked_edits` adds the partners' move, trim or
  delete. One gesture stays one undo step, and undoing it puts every member
  back. Split is not expanded here: `split_at` builds its own composite, because
  the new right-hand halves need a link group of their own.
- **Transitions.** A structural edit can leave a transition describing a cut
  that no longer exists, and the primitive that broke it cannot put it back.
  `ops::detach_broken_transitions` prepends the removals. It runs *after* the
  link expansion, so it judges the whole edit rather than half of it.

Neither recurses into a `Composite`. The composites the app builds — a split, an
import — already know about both.

**That last sentence is why a multi-clip edit cannot simply be a `Composite` the
webview built.** A selection dragged as one gesture is one command per clip, and
if it arrived as a composite the link expansion above would skip it entirely —
the sound of a linked pair would stay where it was. So a batch goes through
`timeline_apply_many` → `ops::compose_edits`, which is the one place that:

1. expands the link partners of every part, and **drops any partner the batch
   already names**, so a selection holding both halves of a pair moves it once
   rather than twice;
2. orders the parts so that no intermediate state overlaps — a block moving
   later is applied right-to-left, a block moving earlier left-to-right, because
   a composite whose first part is refused rolls the whole gesture back;
3. returns a batch of one *unwrapped*, so single-clip editing stays on the path
   above with its own mirroring intact.

A mirrored edit that cannot apply fails the whole composite, so a linked pair
moves as one thing or not at all. Half a move is a pair that no longer lines up.

## Invariants

Enforced by the commands, checked by `Project::validate()`:

1. Segments within a track are sorted by `target_range.start`.
2. Segments within a track never overlap.
3. `render_index` is derived from track order, recomputed centrally by
   `reindex_render_order()` after any structural change. No command sets it
   by hand.
4. Every `material_id` resolves in the pool.
5. Ids are unique: no two tracks and no two segments share one. `track_mut`
   and `segment_mut` return the *first* match, so a duplicate makes the other
   unreachable and every later edit aimed at it silently hits the wrong thing.
6. `source_range.duration = target_range.duration × speed`, to within the
   rounding one derivation costs (`document::speed_slack`). Every command that
   writes a source duration writes `document::source_duration_for(...)`, so the
   document never accumulates the microsecond two callers rounded differently
   and undo stays byte-exact.
7. No number in the document is a NaN or an infinity. `serde_json` writes both
   as `null`, so one that reaches disk is a save that reports success and a
   file that never opens again.
8. A `KeyframeTrack` exists exactly while the property is animated, holds each
   property once, and its keyframes are sorted with distinct times. Tracks are
   kept in the declaration order of `AnimatableProperty` — the order means
   nothing to the renderer, and pinning it is what lets undo put a removed
   track back where it was.

An edit that would break any of these is rejected with an error rather than
applied and repaired. "Target range is occupied" is a legitimate outcome of
dragging a clip onto another one; the UI is expected to show it, not to have
prevented it. The same is true of a trim whose source range does not match its
timeline range at the clip's speed — the arithmetic is the caller's to get
right, and the message says what it should have been.

## Keyframes

Keyframe times are relative to the segment start, never timeline time and never
scaled by speed. A keyframe may only be *placed* inside the clip, but one that
ends up outside it — which trimming a tail legitimately produces — is kept, and
`validate()` reports it as a warning rather than an error. Deleting it would
mean undoing the trim no longer brings the animation back.

Clearing a property's animation is a `Composite` of `RemoveKeyframe`s. The last
one takes the track with it.

## Autosave

`respond()` in `timeline/commands.rs` persists the document after every edit,
undo and redo, through `project::autosave`. It is deliberately **not** an entry
in `History`: undo reverses what the user did, and a background save is not
something they did. See `project/autosave.rs` for why the write is queued on one
thread rather than done inline.

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

A gesture that touches several clips — a multi-selection dragged, trimmed or
deleted — is still one gesture, and sends one `timeline_apply_many` carrying one
primitive per clip plus a label for the undo menu. It sends nothing for a clip's
link partners: those are a fact about the document, and Rust adds them.

The response (`EditResponse`) carries the whole updated project plus the undo
and redo labels, and the frontend store replaces itself with it.
