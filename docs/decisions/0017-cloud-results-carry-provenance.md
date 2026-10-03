# 0017 — Cloud results carry their provenance: asset.json beside the file, `origins` in the project

Status: accepted (2026-10-03, integrations agent)

## Decision

Every file the cloud module writes — a voiceover, a sound effect, a piece of
music, a stock download, a fal.ai result — lives in a directory of its own
next to an `asset.json` sidecar holding an `Origin`
(`modules/cloud/provenance.rs`): provider, model or endpoint, prompt, request
id, time, cost, the account's plan at that moment (ElevenLabs `free`,
`creator`, …), creator and source page for stock, and a normalised licence
(`commercial: yes | no | unknown`, `attribution_required`, `share_alike`,
credit line).

`project_import_media` reads the sidecar of the file it imports and copies the
record into `MaterialPool::origins` (material id → `Origin`). The export
dialog's licence summary and the credits file (`modules/cloud/credits.rs`)
read only `origins`, never the sidecars.

Generated files go under `data_root()/generated/` (they cost money; "Clear
cache" must not delete them). Stock downloads go under
`cache_root()/library/<provider>/<kind>-<id>/` (they can be fetched again),
and search answers are cached there for 24 hours.

## Why

- A project must keep its credits after the cache is cleared or the project
  moves to another machine (`docs/research/open-assets.md`, "Local cache with
  licence metadata"). Holding the record in the project is the only way.
- A sidecar next to the file keeps the record when the file is used in a
  second project, and needs no new import path: any import of that file, by
  any shell, picks it up.
- A pool map keyed by material id is additive: no material struct, no
  constructor and no existing test changed, and an old project without the
  key loads unchanged (`skip_serializing_if = "BTreeMap::is_empty"`).
- The free-tier flag must be recorded at generation time. The plan can change
  later, and what counts is the plan that made the file.

## Costs

- Importing reads one small JSON file per import.
- `origins` is not cleaned when a material is removed from the pool; an
  orphan entry is inert, like an unreferenced link group.
- The import is not undoable (as before), so recording the origin is not
  either.

## What would change our minds

- A content-addressed library cache shared between projects
  (`open-assets.md` proposes one): the sidecar would then move with the
  hashed file, and `origins` would stay as it is.
- A provider whose licence depends on more than a plan name (per-generation
  rights): the `Licence` record would grow a field, not change shape.
