# 0005 — Linked audio and video

Status: decided 2026-07-26. Implementation in `modules/project/document.rs`
(`MaterialPool::links`), `modules/timeline/ops.rs` (`SetLinkGroup`,
`mirror_linked_edits`, `link`, `unlink`, `split_at`) and
`src/modules/timeline/lib/edits.ts` (`planPlacement`).

## The decision

**Importing a file that carries both streams puts two clips on the timeline**,
not one: the picture on a video lane and the sound on an audio lane, at the same
`target_range`, referencing the same material, and **linked** so that moving,
trimming, splitting or deleting either one does the same to the other. The user
can break the link from the clip's context menu, after which the two are
ordinary clips.

Before this, a video segment carried its own audio and there was no way to see
it or to move it. That is not a missing feature so much as a missing
*substrate*: ducking the music under a line, keeping the room tone while cutting
away from the speaker, and sliding the sound two frames to fix lip sync are all
"move the audio, not the picture", and none of them can be expressed at all
when there is one segment.

## Where the linkage is stored, and why not the two obvious places

Three shapes were considered.

**A field on `Segment` — `link_group: Option<Id>`.** The clearest to read, and
rejected on a technicality that turned out to matter: `Segment` is constructed
as a struct literal in a dozen places across `render/`, `export/`, `preview/`
and the test suites, and adding a field to it edits all of them. That is churn
in files this change has no business touching, and in a repository where
several agents work at once it is the shape of change that produces the merge
accidents `CLAUDE.md` describes.

**A table on `Project` — `links: Vec<(Id, Id)>` naming the pairs.** Rejected for
the reason `TransitionMaterial` gives for rejecting the same idea, which is
already written down in `document.rs` and has been paid for once: a side table
that names segment ids is a **second place segment ids appear**, so every
structural edit has to remember to fix it up, and the one that forgets leaves a
row pointing at a segment that no longer exists. `RemoveSegment` would have to
carry the row so undo could put it back; `split_at` would have to write a row
for the new halves; a fuzzer would find the one that was missed.

**An id in `Segment::extras`, with the live group ids in the material pool.**
Chosen. `extras` is already a list of ids with no type tag — the kind of an id
is whichever pool category it resolves in, which is CapCut's trick and is
documented in `docs/research/draft-format.md`. Two segments are linked when
they carry the same id, and `MaterialPool::links` is the category that makes
such an id recognisable as a link rather than an effect.

What that buys, all of it for free:

- `RemoveSegment` already snapshots the whole `Segment`, so undoing a delete
  restores the linkage without knowing links exist.
- `MoveSegment` and `TrimSegment` never touch `extras`, so a linked clip carries
  its group through every edit.
- `split_at` clones the segment, so the only work is one line — the same line
  that already strips the transition — plus giving the two new halves a group of
  their own.
- Adding it changed no existing `Segment` construction and no existing test
  fixture, exactly as adding transitions did.

The price is that a group id in `extras` means nothing on its own, so
`MaterialPool::links` has to be maintained. Exactly one command writes it —
`SetLinkGroup` — which registers a group on the way in and drops it when the
last member leaves. That is what keeps the two directions byte-exact inverses,
which is what undo requires.

One deliberate asymmetry: **`RemoveSegment` does not prune.** Deleting every
member of a group leaves its id in the pool with nothing pointing at it, because
pruning would mean the undo of that delete had to put the id back, and
`RemoveSegment` does not carry it. An unreferenced group is 38 bytes of JSON
that no code path reads.

## How linked edits happen

`History::apply` expands the command before recording it, next to and just
before `detach_broken_transitions`, and for the same reason: it is the one place
every edit passes through, so no caller can forget. `mirror_linked_edits` turns
a move, a trim or a delete of a linked clip into a `Composite` holding that
command and its partners' — **one undo step**, which is the whole point.

Three rules in it are worth stating because the opposite is defensible:

- **Only the time is shared, not the lane.** Dragging the picture from one video
  track to another leaves the sound on its audio track. Sharing the lane would
  drag audio onto a video track, which is where it cannot be found again.
- **A trim mirrors as two deltas**, how far the head moved and how far the tail
  moved, rather than as the same absolute range. Linking is not only for a clip
  and its own sound: two clips a user linked by hand may sit at different
  places, and dragging one's tail must not teleport the other.
- **A mirrored edit that cannot apply takes the whole thing down.** If the audio
  lane is occupied where the sound would have to land, the picture does not move
  either. Half a move is a pair that no longer lines up, which is worse than a
  refusal — and the frontend avoids ever asking, by searching for a free slot on
  both lanes at once.

`split_at` is not mirrored generically; it builds its own composite, because the
two new right-hand halves need a **new** group. Leaving all four in the original
group would mean dragging the second half of the picture dragged the first half
of the sound.

## Who plays the sound

Both segments name the same material, and the mixer's rule is "a video material
whose container carries audio makes sound, wherever it sits". Left alone that
mixes the same waveform with itself: 6 dB louder, and phasing with every
microsecond the two are out by.

So `Project::sound_is_on_a_linked_lane` decides, and the segment that is *not*
on the audio lane defers. The check is about where the partner sits rather than
about a stored role, so dragging the sound onto a video lane — or the picture
onto an audio one — does the obvious thing instead of silencing both.

`audio::mixer::plan` asks it. **`export::audio` does not yet**, because it holds
its own copy of the same resolution and that file was being edited by other work
when this landed. Until it does, an export of a project containing an imported
pair sums that clip's audio twice. It is one call in
`export/audio.rs::audio_path`'s caller, against the same document method.

## What would change our minds

- **If linking ever needs parameters** — a named group, an offset, a lock icon
  the user can toggle per pair — `MaterialPool::links` should become
  `Vec<LinkGroup>` alongside `transitions`, which is a schema bump and a
  migration step rather than a redesign. The membership stays where it is.
- **If groups routinely grow past two members**, the "mirror every partner"
  expansion becomes a composite of N commands per gesture and the atomic
  all-or-nothing rule starts refusing edits people expect to work. At that point
  the refusal should become "move what can move", which is a change to
  `mirror_linked_edits` and to nothing else.
