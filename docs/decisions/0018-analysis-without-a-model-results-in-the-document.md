# 0018 — Scene, shake, beat and reframe analysis: own detectors, results in the document

Status: accepted (2026-10-03, analysis agent)

## Decision

Scene detection, stabilisation, beat detection and auto reframe run **in the
engine, in plain Rust, with no model and no ML runtime** —
`crates/engine/src/modules/analysis/`. Each is a background job (progress,
cancel) that ends in **one** undo step. Results that drive edits — scene
cuts, beats, the camera path a clip is stabilised along — are **tagged
entries in `MaterialPool::extras`** referenced from the clip's `extras`, in
source time. Raw per-frame results (scene scores, camera motion) are cached
under `cache_root()/analysis/` for speed only.

| Feature | Detector | Licence | Why not the alternative |
|---|---|---|---|
| Scenes | colour histogram + 16×9 block difference against a local median | own code | FFmpeg `scdet` is in the system build, but its per-frame score would have to be parsed back from filter metadata; our score is cached, so a new sensitivity re-thresholds without decoding. TransNetV2 (MIT) when the ML worker exists, for dissolves. |
| Stabilisation | the tracking module's KLT + RANSAC (shift consensus, then similarity) per frame, Gaussian smoothing per shot, applied as a moving crop window + counter-turn in the compositor | own code | vid.stab (`vidstabdetect`, GPL-2+) writes local motion fields, not the global transform we need to apply ourselves; parsing `.trf` and re-deriving the global motion is the same work as measuring it. Applying in our compositor is what makes preview = export. |
| Beats | spectral-flux onsets, harmonic-weighted autocorrelation tempo, Ellis's DP beat tracker | own code | Beat This! (MIT) through `ort` is the better tracker and belongs in the ML worker (`docs/research/ml-features.md` §5.1). madmom's models are CC-BY-NC-SA: excluded. |
| Reframe | saliency = motion that is not the camera's (KLT-compensated frame difference) + centre/surround contrast + skin tone; best window per frame; per-shot hold / smoothed pan; written as position keyframes on a clip filled to the canvas | own code | YuNet (OpenCV Zoo, MIT) and RT-DETR (Apache-2.0) are the right detectors, but ONNX Runtime must not enter the editor process (§5.1), and the worker + model registry + downloader do not exist yet. |

## Why the pool's `extras` map, not new pool categories

- **No schema change and no new undo variant.** A clip swaps one entry id for
  another with `RemoveSegment` + `InsertSegment` in one `Composite`, exactly
  like `voice::cleanup`. `split_at` clones `extras`, so both halves of a cut
  clip keep their analysis.
- **Entries are immutable.** A changed setting is a new entry under a new id;
  undo puts the old id back. That makes the id a complete memo key, so the
  compositor parses a stabilisation once (`stabilise::applied`) instead of
  per frame.
- The camera path (heavy) and the stabilisation settings (light) are separate
  entries, so changing the strength does not copy the path.
- Cost: an entry no clip names any more stays in the pool, inert (a few KB;
  a camera path is ~40 bytes per frame). Nothing prunes them yet.

## Why position keyframes for reframe, not crop keyframes

The crop path is written as `PositionX` (or `PositionY`) keyframes on the
clip scaled to cover the canvas. That is the same picture as a moving crop
window, it uses the keyframe system the inspector and timeline already edit,
and it adds no animatable property for the speed-ramp work (keyframe graph
editor) to account for. Shot changes get `Easing::Hold` so the window jumps
with the cut. A ratio switch and its reframe are one undo step through
`DocumentHistory::apply_configure_and_edit`.

## What would change our minds

- The ML worker landing: swap the detector behind each job (TransNetV2,
  Beat This!, YuNet/RT-DETR); the document shape stays.
- Users asking for per-frame crop editing of a reframe: then an animatable
  crop, and reframe writes that instead.
- Long clips making camera paths large in project files: thin the path with
  Ramer–Douglas–Peucker as the research suggests for tracks.
