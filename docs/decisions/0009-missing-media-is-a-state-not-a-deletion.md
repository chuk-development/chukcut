# 0009 — Missing media is a state, not a deletion

Date: 2026-07-28. Status: accepted.

## The problem

Three complaints from the owner, which turned out to be one architecture bug
and two missing contracts ("Logisches Behältiger halt"):

1. A saved and reopened project showed an **empty media library** even though
   every import was in the file. Importing writes `project.materials` and
   saving persists it — but `MediaLibrary` rendered a session-local list in
   the media store, which a restart forgets. The media *was* saved; the panel
   could not see it.
2. Removing a library tile only removed the session row. Nothing could take a
   material out of the project, and nothing said what would happen to the
   clips using it if something did.
3. A clip whose material or file was gone simply vanished from the composite
   (a hole showing the background) and its dangling reference made
   `validate()` report the document as **erroneous** — i.e. corrupt.

## The decision

**The library is a view of `project.materials`.** The panel derives its rows
from the document (`media/lib/library.ts::libraryItems`); the media store
keeps only what the document cannot know — import progress, the last error,
and which pool files are currently missing on disk. The library and the
timeline are thereby linked automatically: same pool, same ids. Session
niceties (thumbnails, waveforms) stay keyed by path, so they survive nothing
and need to survive nothing.

**Removing media is an ordinary document edit.** `EditCommand::RemoveMaterial`
carries the whole pool entry plus its index (the `RemoveSegment` contract:
undo restores byte-exactly, position included), inverts to `AddMaterial`, and
sits on the one undo stack. It deliberately does **not** touch the segments
that reference the material, and it never touches the file on disk. The
context menu says what will happen before it does ("Remove from project —
N clips will go offline").

**A clip without media is offline, not deleted and not an error.** The same
state has two causes — material removed from the pool, file removed from
disk — and everything renders it honestly:

- `Project::validate` reports a dangling `material_id` as a **warning** (it
  was an error; that branded a normal, user-reachable state as corruption).
  Missing-file warnings now cover audio and images as well as video.
- The timeline clip draws red-tinted with an offline icon, label kept
  (`Segment.tsx`, `data-missing`); the library card likewise ("missing on
  disk", via the `media_missing_files` command — the webview cannot stat).
- The compositor composites a flat dark-red field
  (`media::MISSING_MEDIA_RGBA`) where the clip would be. The placeholder is
  served by `MediaSourceProvider` — unknown id, or a path that fails
  `exists()` — so every consumer of the provider gets it for free. A hole was
  rejected because it reads as "my clip was deleted", which is precisely the
  fear this design exists to remove.
- The audio mixer already treated an unreadable file as silence per segment;
  a removed material falls out of the plan. Unchanged.
- **The export still refuses**, but now up front and by name:
  `export::job::missing_media` runs before the writer opens and the message
  lists each clip and why ("2 clips reference media that is missing: …").
  A placeholder is honest in a preview and a defect in a delivered file.

## Costs and edges

- The provider stats video/image paths once per frame request. A stat is
  nothing next to a decode; a dead network mount would stall the render
  thread, but it would have stalled the decoder one line later anyway.
- A removed material whose clip sits on a non-audio lane is assumed to have
  been visual (the pool entry that knew its kind is gone), so a removed
  *audio* material dropped on a video lane draws the placeholder. Harmless
  and rare; the alternative is guessing.
- The frontend's missing-on-disk state is polled only when the document
  changes, not on a timer — deleting a file behind a running app shows up on
  the next edit or reopen, not instantly. `chukcut-frame` previews notice
  immediately regardless, because the provider checks per frame.

## What would change our minds

A relink flow ("locate missing file…") would sit naturally on top of this —
the offline state already names the path — and nothing here blocks it. If
pool categories ever get reordered by the UI, `RemoveMaterial`'s index check
gains a new caller to be wrong about; the stale-gesture refusal is the guard.

## Proof

`src-tauri/tests/missing_media.rs` is the owner's scenario end to end:
import two files, place clips, remove one material through `History`, save,
reload through `migrate::load` — the clip is still there, validate warns,
the compositor's pixel at the clip's centre is the placeholder, and one undo
restores the document byte-exactly. Plus `ops.rs` (exact undo, stale index,
duplicate id), `provider.rs` (placeholder for both causes, one shared
texture), `tests/compositor.rs` (placeholder over the old hole), and on the
frontend `MediaLibrary.test.tsx` (pool rendering, the reopen case, the
remove edit's wire shape, the missing card), `Segment.test.tsx` (both
missing causes draw the state, titles do not), `store.test.ts` (missing
check semantics), `ipc-contract.test.ts` (the `remove_material` bytes).
