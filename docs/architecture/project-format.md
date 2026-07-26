# Project format

A project is one JSON file: `<name>.chukcut`. The authoritative definition is
`src-tauri/src/modules/project/document.rs`; this document explains the choices
behind it.

## Lineage

The shape is informed by CapCut's `draft_content.json`, which was studied
during reverse engineering. Not because we import it — we do not — but because
it is a format that has survived years of a feature-hungry editor, and its
structural decisions are worth inheriting rather than rediscovering.

What was worth taking:

- Materials in a flat pool, referenced by id from segments.
- Materials grouped by kind rather than one heterogeneous list.
- Segments carrying both a *target* range (where on the timeline) and a
  *source* range (which part of the media).
- Durations in microseconds.
- An explicit render index rather than implicit array order.

What was left behind: their 50 material categories, cloud/draft/agency
metadata, tracking ids, and the encrypted key store. We add categories when we
build the feature that needs them.

## Time

Every time value is `i64` microseconds.

Not floats: adding a thousand clip durations accumulates error, and two clips
that should abut end up with a one-sample gap that clicks on playback.

Not frames: it forces a frame rate into the data model. A 24 fps clip on a 30
fps timeline has no integer frame position, and changing project frame rate
would have to rewrite every number in the document.

Microseconds are exact for every frame rate anyone uses, and convert to frames
only at the edges — the renderer, the exporter, the ruler labels.

## Structure

```jsonc
{
  "id": "…",
  "schema_version": 1,
  "name": "My project",
  "canvas": { "width": 1080, "height": 1920, "background": [0,0,0,1] },
  "fps": 30.0,

  "materials": {
    "videos": [ { "id": "v1", "path": "/…/clip.mp4", "width": 1920, … } ],
    "audios": [ … ],
    "images": [ … ],
    "texts":  [ … ],
    "extras": { "<id>": { /* effect / transition / animation params */ } }
  },

  "tracks": [
    {
      "id": "t1",
      "kind": "video",
      "name": "Video 1",
      "muted": false, "locked": false, "hidden": false,
      "segments": [
        {
          "id": "s1",
          "material_id": "v1",
          "target_range": { "start": 0,       "duration": 4000000 },
          "source_range": { "start": 2000000, "duration": 4000000 },
          "render_index": 0,
          "speed": 1.0,
          "volume": 1.0,
          "transform": { "position": [0,0], "scale": [1,1], "rotation": 0, "opacity": 1, … },
          "crop": null,
          "extras": [],
          "keyframes": []
        }
      ]
    }
  ]
}
```

## The two ranges

This is the part that trips people up, so it is worth being explicit.

```
source file:  [────────────────────────────────────]  0 … 10s
                        ├──── source_range ────┤       2s … 6s

timeline:     [──────────────────────────────────]
              ├── target_range ──┤                     0s … 4s
```

- `target_range` — where the clip sits on the timeline.
- `source_range` — which part of the material it shows.

Trimming the left edge moves both starts. Sliding the clip moves only
`target_range.start`. A slip edit moves only `source_range.start`. Speed
changes make the durations differ by the speed factor:
`source_range.duration = target_range.duration × speed`.

`Segment::source_time_at()` is the one function that maps between them; nothing
else should do the arithmetic.

## Normalized transforms

`Transform::position` is in normalized canvas units — `[0,0]` is the center,
`1.0` is half the canvas dimension. `scale` is a multiplier of the fit size.

Storing pixels would mean every clip jumps when the project is resized from
9:16 to 16:9. Normalized values survive that, which matters for an editor whose
main job is producing the same cut for three aspect ratios.

## Keyframes

Keyframe times are **relative to the segment start**, not the timeline. Move a
clip and its animation moves with it; trim its head and the animation stays
attached to the frames it was authored against.

Interpolation is per-keyframe: each keyframe carries the easing used to reach
the *next* one. `Easing::Hold` gives step animation.

## Validation

`Project::validate()` distinguishes two things:

- **Warnings** — the project is fine, the world is not. Missing media file, for
  instance. The app must open and show a relink prompt.
- **Errors** — the document is internally inconsistent: overlapping segments on
  one track, unsorted segments, a segment referencing a material that is not in
  the pool, two things sharing an id, a range that starts before zero, a
  non-finite number, two ranges that disagree about the segment's speed, or a
  keyframe track that is empty, unsorted or duplicated. These indicate a bug in
  an edit command and should never reach disk.

A keyframe *outside* its segment is a warning, not an error: trimming a clip's
tail legitimately leaves one behind and the document keeps it so that undoing
the trim brings the animation back.

## Migrations

`schema_version` gates loading. When the format changes incompatibly, bump the
constant and add a step in `project/migrate.rs` that walks the JSON from the
old version to the new one before deserializing. Never silently accept an
unknown version — a project the app half-understands is worse than one it
refuses.

`migrate::load` is the **only** place allowed to turn bytes into a `Project`.
A bare `serde_json::from_str::<Project>` anywhere else is the bug this gate
exists to prevent. It also does the other half of not-guessing: a file from a
newer build is refused with a message naming both versions, and one damaged by
the rule below is repaired rather than rejected.

## Numbers that cannot be written

No float in the document may be a NaN or an infinity. JSON has no way to
express either, and `serde_json` writes `null` — so a project that took one
saved successfully and then failed to load forever after with "invalid type:
null, expected f32". A save that reports success and a file that never opens is
the worst failure this format can have, so non-finite values are refused at the
edit-command boundary and are errors in `validate()`.

Because those files already exist, **every float field carries a `serde`
default**, and `migrate::repair_non_finite` drops the `null`s so the defaults
apply. Dropping the key rather than zeroing it is the point: an opacity comes
back at 1.0, which is a clip the user can still see.
