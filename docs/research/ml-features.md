# Machine-learning features: what CapCut has, and how chukcut can build them

Research, 2026-10-03. No code was written for this document. It answers one
question from the owner: which ML features does an editor like CapCut have, and
how should chukcut implement each one — which model, which runtime, which
hardware.

The owner's first priority is **motion tracking**: the user marks an object in
the player, the editor tracks it through the clip, and a text, sticker or effect
follows it. Section 2 covers that in depth. The other features follow in
shorter sections.

## How to read the numbers

Every number has one of three tags:

- **[src]** — taken from a cited source (paper, model card, README).
- **[meas]** — measured or verified on the development machine
  (RTX 3060 12 GB, driver 610.57, Ubuntu FFmpeg 6.1.1).
- **[est]** — an estimate. It comes from published numbers on other hardware,
  scaled by rough GPU ratios (an A100 is about 3–4× an RTX 3060 for fp16
  inference; an Intel Xe iGPU is about 10–20× slower than an RTX 3060). Treat
  an estimate as an order of magnitude. Measure it before you rely on it.

"Licence OK" means: the licence of the **code and the weights** lets us ship or
download the model for a GPL-3.0 application **and** lets us charge money for a
feature that uses it later. Decision [0010](../decisions/0010-open-source-under-gpl.md)
says chukcut is not sold today; the brief says paid extras are possible later.
So a non-commercial (NC) licence is a hard "no" here. It blocks the paid path
and it adds a use restriction that sits badly next to the GPL.

Effort: **S** ≤ 3 days, **M** 1–2 weeks, **L** 3 weeks or more, for one
engineer (or one agent) who knows the codebase.

---

## 1. Summary

| Feature | CapCut has it | Best open option | Licence OK? | Runtime | Effort | Prio |
|---|---|---|---|---|---|---|
| **Motion tracking (attach to object)** | yes (`video_trackings`, `Tracking.dll`) | own KLT + similarity fit; then VitTrack / MixFormerV2-S; then SAM 2.1-tiny / EfficientTAM | yes (own code, Apache-2.0, MIT) | pure Rust → `ort` | L (in 3 steps) | **1** |
| Auto captions + word times | yes (Captions tab, `speechsdk.dll`) | Whisper large-v3-turbo (whisper.cpp); Parakeet-TDT-0.6B-v3 | yes (MIT; CC-BY-4.0) | `whisper-rs`; `ort` | M | **1** |
| Silence removal | yes (transcript tools) | Silero VAD + FFmpeg `silencedetect` | yes (MIT) | `ort` (CPU) | S | **1** |
| Noise reduction (audio) | yes ("Entrauschen", Pro) | DeepFilterNet3; RNNoise (`arnndn`) | yes (MIT/Apache; BSD) | `deep_filter` crate (tract); FFmpeg | S | **1** |
| Scene / shot detection | yes ("Szenen aufteilen") | FFmpeg `scdet`; TransNetV2 | yes (LGPL/GPL; MIT) | FFmpeg; `ort` | S | **1** |
| Stabilisation | yes ("Stabilisieren", Pro) | vid.stab via FFmpeg; later own KLT path | yes (GPL-2+) | FFmpeg; Rust | S → M | **1** |
| Background removal (video) | yes ("Hintergrund entfernen") | RVM (people); BiRefNet-lite (objects); SAM 2.1 (click) | RVM GPL-3.0 (OK for GPL, not for closed extras); MIT; Apache | `ort` | M | 2 |
| Voice isolation / stems | yes ("Stimme isolieren") | HTDemucs; (Mel-)RoFormer | MIT; RoFormer weights unclear | `ort` | M | 2 |
| Beat detection | yes (`beats`, `markBeat`) | Beat This! (small) | yes (MIT) | `ort` / `beat-this` crate | S | 2 |
| Auto reframe 9:16 | yes ("Auto-Formatänderung", 771 MB SmartCrop models) | face + person detector + shot cuts + smoothed crop path (AutoFlip method) | yes (YuNet MIT, RT-DETR/D-FINE Apache) | `ort` | M | 2 |
| Translation | yes (`AiTranslate`, audio translation) | Opus-MT (Marian); MADLAD-400; small LLM | yes (CC-BY-4.0; Apache) | `ort` / `candle`; llama.cpp | M | 2 |
| Text-to-speech | yes (`pc_tts`, SAMI) | Piper; Kokoro-82M; Chatterbox | yes (GPL-3 code, per-voice; Apache; MIT) | `ort` | M | 2 |
| Colour auto-adjust / match | yes ("Automatische Anpassung", colour match) | classic statistics (no model) | yes (own code) | Rust / wgpu | S | 2 |
| Filler-word removal | yes (transcript) | Whisper with prompt; Parakeet; no good permissive model | partly | as captions | M | 3 |
| Frame interpolation | yes ("Optischer Fluss", `CCLensVFIFilter`) | RIFE 4.x; FILM | yes (MIT; Apache) | `ort` CUDA/TensorRT | M | 3 |
| Upscaling / quality | yes ("Qualität optimieren", Pro) | Real-ESRGAN general-x4v3 / x4plus | yes (BSD-3) | `ort` | M | 3 |
| Face / body landmarks | yes (`tt_face`, `tt_skeleton`) | MediaPipe Face Landmarker; RTMPose | yes (Apache) | `ort` | M | 3 |
| Retouch / beauty | yes (full "Retuschieren" tab) | landmarks + skin mask + wgpu filters | mostly (face-parsing weights are a risk) | `ort` + wgpu | L | 3 |
| Object removal (inpaint) | yes ("KI-Entfernen") | LaMa per frame (flickers) | yes (Apache) | `ort` | L | 3 |
| Generative (AI extend, remix, lip sync, eye contact, relight, avatar) | yes | out of scope | — | — | — | — |

The top five recommendations are in section 7.

---

## 2. Motion tracking — attach text, stickers and effects to a moving object

### 2.1 What CapCut does

**What the user sees.** Third-party guides describe the same flow on mobile and
desktop ([miracamp], [antwort], [filmora]): select the overlay (text, sticker)
or the clip, open **Tracking** in the inspector, place and resize a **box** on
the object in the player, choose the direction (forward, backward or both) and
start. The overlay then follows. There are two modes: **Position** and
**Position & Scale**. No guide mentions rotation. If the tracker loses the
object, the advice is "move the box and run it again" or "split the clip and
track the parts". These guides are weak sources. Our own UI captures
(`docs/reference/capcut/*.png`) do **not** show the overlay's Tracking tab.
**Action:** capture the text inspector's tracking tab and the box on the
player from the `win10` VM, with `_scratch/vm/inspector.sh`.

The video inspector's **Basic** tab (capture `30-…`) also lists
"Kameraverfolgung" (camera tracking, Pro), "KI-Bewegung" (AI motion) and
"Bewegungsunschärfe" (motion blur). Camera tracking is a different feature
(see 2.9).

**What the binaries and the project format say** (static evidence only, see
[engine-symbols.md](engine-symbols.md), [ui-inventory.md](ui-inventory.md),
[draft-format.md](draft-format.md)):

| Evidence | What it tells us |
|---|---|
| `Tracking.dll`, 1.6 MB | The tracker is a small separate library. 1.6 MB is too small to hold a modern learned tracker's weights. So the tracker is classic CV, or its model is downloaded separately. (Inference.) |
| `fastcv.dll` (Qualcomm FastCV) | FastCV contains pyramidal Lucas–Kanade optical flow and corner detectors. This fits a classic feature tracker. (Inference.) |
| `CCObjectTrackerFilter` in the render graph | Tracking is a node in the render graph, so the engine evaluates it at render time. |
| `SwingObjectTracker`, `SwingSurfaceTracker`, `SwingSurfaceTrackingController`, `AETrackable`, `AEPlaneAnchor` | Two tracker kinds: an object tracker (box) and a surface (planar) tracker. |
| `materials.video_trackings`; `SegmentText.video_tracking`; segment fields `surface_trackings`, `corner_pin`, `object_locked` | The track result is a **material in the pool**. The text segment **refers** to it. So CapCut uses a "follows track" link, not keyframes baked into the text. `corner_pin` + `surface_trackings` is planar tracking (screen replacement). `object_locked` is most likely "lock the object in place" (stabilise on the tracked object). We have no draft with a populated `video_trackings` entry, so the field contents are unknown. |
| QML `view/tracking_control` (7 files), `view/custom_tracking`; 43 `pc_track_*` strings; `camera_tracking.ini` | The player has its own tracking overlay (the box and its handles). Camera tracking has its own panel descriptor. |

Summary: CapCut does box tracking with an offline analysis step. It stores the
result as a separate material. Overlays reference that material, and the
compositor applies the track at render time. That is the design we recommend
below.

### 2.2 The job, stated exactly

Input: one video segment, one start frame, one box (later also a click point),
and a direction. Output: for every source frame in the range, an object pose —
centre `(x, y)`, size `(w, h)`, optional rotation, and a **confidence**.

The hard cases, in the order users meet them:

1. **Fast motion** (a thrown ball). The object moves more than its own size
   between frames. Classic local trackers search near the last position and
   lose it.
2. **Motion blur.** The appearance changes. Template and feature trackers lose
   texture.
3. **Scale change** (the ball flies toward the camera).
4. **Occlusion** (a hand covers the ball, a person walks in front).
5. **Out of frame and back.** Re-detection is necessary.
6. **Rotation** (a turning head, a sign). This is only important if the overlay
   must rotate.
7. **Similar objects nearby** (two balls). The tracker can switch to the other
   object.

A product-quality tracker needs three things beyond the raw tracker: a
confidence signal (so it can stop or ask for help), user corrections (anchors),
and smoothing.

### 2.3 Tracker options compared

| Option | Kind | Licence | Size | RTX 3060 | Intel iGPU / CPU | Fast motion | Blur | Occlusion | Scale | Rotation | Rust path |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **Template matching (NCC)** | classic | own code | 0 | n/a (CPU) | ~1–3 ms/frame at 640 px with a search window [est] | weak; better with a large window and a motion prediction | weak | none | no (only with a scale pyramid) | no | pure Rust; `imageproc::template_matching` exists |
| **KCF / CSRT** | classic correlation filter | OpenCV contrib, Apache-2.0 | 0 | n/a | KCF 100+ fps, CSRT ~20–40 fps CPU [est] | weak | medium | weak; CSRT drifts | CSRT yes | no | `opencv` crate (system OpenCV, heavy C++ build); or re-implement (M) |
| **KLT feature tracking + robust fit** | classic optical flow | own code | 0 | n/a | ~1–5 ms/frame for ~100 points at 640 px [est] | weak (pyramid helps up to ~30–50 px/frame at analysis size) | weak | points die; needs re-seeding | **yes** (similarity fit) | **yes** (similarity fit) | pure Rust (≈800 lines); later wgpu compute |
| **VitTrack** (OpenCV zoo) | learned single-object tracker (SOT) | Apache-2.0 [src] | ~0.7 MB ONNX, int8 variant too | CPU is enough | **2.0 ms/frame** on an i3-10105, 4 threads [src] | good | good | medium; gives a confidence score [src] | box | no | `ort` CPU |
| **MixFormerV2-S / -B** | learned SOT | MIT [src] | tens of MB | B: 165 fps "on GPU" [src]; ~80–120 fps [est] | S: **30 fps on CPU** [src] | good | good | medium | box | no | export PyTorch → ONNX (not provided; plausible), `ort` |
| **OSTrack** | learned SOT | MIT | ~90 M params | 105 fps on the paper's GPU [src]; ~50 fps [est] | slow on CPU | good | good | medium | box | no | export → `ort` |
| **CoTracker3** | point tracker (many points jointly) | **CC-BY-NC-4.0** [src] | ~25 M params | offline; ~200 ms/frame for 1024 points on an RTX 3090 (derived from [lite]) | too slow | very good | very good | **very good** (predicts occlusion) | via points | via points | `ort` after export — **excluded by licence** |
| **LiteTracker** | CoTracker3 optimised | uses CoTracker3 weights → **NC** | as above | 29.7 ms/frame for 1024 points on an RTX 3090 [src] | — | as above | | | | | **excluded** |
| **TAPIR / BootsTAPIR (online)** | point tracker | **Apache-2.0, code and checkpoints** [src] | ~30 M params | online TAPIR: 17 fps at 480×480 on a Quadro RTX 4000 [src]; similar on a 3060 [est] | ~1–3 fps [est] | good | good | good (occlusion output) | via points | via points | PyTorch version exists → ONNX export (unverified) → `ort` |
| **SAM 2.1 video predictor** (tiny / small) | promptable segmentation + memory | **Apache-2.0** [src] | tiny 38.9 M, small 46 M params [src]; ~150–180 MB fp32 | 91 / 85 fps on an A100 (compiled PyTorch) [src]; **~20–30 fps** with ORT fp16 [est] | **~2–5 fps** with OpenVINO [est] | very good | very good | good (memory bank) | **yes** (mask) | **yes** (mask orientation) | `ort` with a community ONNX export of the video path (memory attention needs a real-valued RoPE rewrite) [src: hf sam21-tiny-video-onnx] |
| **SAMURAI** | SAM 2.1 + Kalman motion model + better memory selection | Apache-2.0 [src] | same as SAM 2.1 | like SAM 2.1, a little slower [est] | like SAM 2.1 | **better than SAM 2** on fast motion and distractors [src: paper] | very good | better | yes | yes | same weights as SAM 2.1; the motion logic is plain code we can port |
| **EfficientTAM** | SAM-2-style, lighter image encoder | **Apache-2.0** [src] | smaller than SAM 2.1-tiny | **~40–60 fps** [est] | >10 fps on an iPhone 15 [src]; ~5–10 fps on an iGPU [est] | good | good | good | yes | yes | no ONNX export published [src]; export work needed |
| **Cutie / XMem** | video object segmentation; needs a first-frame mask | MIT / MIT [src] | ~35 M params | ~30–45 fps on an A100 at 480p (approximate, from the paper) [est] | slow | good | good | **very good** | yes | yes | export → `ort`; needs SAM for the first mask |
| **SAM 3** | text-promptable detect + track | "SAM License" (custom) [src] | ~840 M params, ~3.4 GB [src] | too heavy for interactive use | no | — | — | — | — | — | not recommended: size, plus a custom licence to read first |

What the table says:

- **Classic trackers** give instant results with no download and no GPU. They
  fail on exactly the owner's example: a thrown ball (fast motion plus blur).
  KLT has one big strength: from many points inside the box, a robust
  similarity fit gives **position, scale and rotation** at the same time.
- **Learned single-object trackers** (VitTrack, MixFormerV2-S) are the sweet
  spot for "box in, box out". They are tiny, they run on the CPU, and they are
  robust to fast motion and blur. They also give a confidence value. They give
  no rotation.
- **Segmentation trackers** (SAM 2.1, SAMURAI, EfficientTAM) are the most
  robust and also give a mask. With a mask we get rotation (from the mask
  moments), a matte for "cut out the object", and a mask for "blur the face".
  They need a GPU to feel interactive, and the video path is not trivial to
  run outside PyTorch.
- **CoTracker3 and LiteTracker are out** because of the licence.
  **TAPIR/BootsTAPIR** is the permissive point-tracker alternative if we ever
  need dense points.

### 2.4 Getting position, scale and rotation from each tracker

| Tracker output | Position | Scale | Rotation |
|---|---|---|---|
| Box (SOT, NCC, KCF) | box centre | `sqrt(w·h)` relative to the first frame (stable against aspect jitter) | — |
| Points (KLT, TAPIR) | centre of the fitted similarity transform | from the transform | from the transform |
| Mask (SAM family) | mask centroid (or box centre; the user chooses) | `sqrt(area)` relative to the first frame | principal axis from second-order moments; unreliable for round objects — gate it on eccentricity |

A practical hybrid (recommended for step T2): a **learned SOT** gives the box
for each frame (robust). **KLT inside that box** gives rotation and fine scale.
If the SOT confidence drops, the frame is marked "lost" and is not used.

### 2.5 From a track to edit data

#### The document model

Two options:

**A. Bake keyframes.** Write `PositionX/Y`, `ScaleX/Y` and `Rotation` keyframes
onto the overlay segment (the `AnimatableProperty` set in
`project/document.rs` already has these properties).

- Good: no new render concept, and the user can edit the keyframes directly.
- Bad: it breaks when the **video** segment changes. Overlay keyframes are
  relative to the *overlay's* start. The track is in the *video's source* time.
  Trim, slip, speed-change, move, crop or scale the video clip, and the text no
  longer sits on the ball. A re-track overwrites manual keyframe edits. A 30 s
  clip creates 900 keyframes per property.

**B. A track material plus a "follows" link** (what CapCut does, see 2.1).

```text
materials.tracks[id] = Track {
    media_id,                 // the source asset that was analysed
    source_range,             // in source Micros
    tracker: "klt@1" | "vittrack@2023sep" | "sam2.1-tiny@1" …,
    samples: [ TrackSample { source_time: Micros, cx, cy, w, h, angle,
                             confidence, flags: USER_ANCHOR | LOST } ],
    smoothing: { position, scale, rotation },   // strengths, 0 = raw
}
segment.follow = Some(Follow {
    track_id,
    target_segment_id,       // the video segment whose pixels were tracked
    mode: Position | PositionScale | PositionScaleRotation,
    offset: Transform,       // where the overlay sits relative to the object
})
```

Samples are in **source-normalised coordinates** (0…1 of the source frame) at
**source times**. At render time the compositor does this for the overlay at
timeline time `t`:

1. `src_t = target_segment.source_time_at(t)` — the one function that maps
   timeline to source time (see `project-format.md`), so trim, slip and speed
   changes work automatically.
2. Sample the track at `src_t`. Interpolate between samples. Apply the
   smoothing. Hold the last good value over `LOST` frames.
3. Map the source point through the video segment's **crop** and
   **transform** into canvas coordinates. Now a scaled or moved video still
   keeps the text on the ball.
4. Compose with the overlay's own `offset`, and then with the overlay's own
   keyframes. So the user can still animate the text (pop in, wiggle) while it
   follows.

- Good: robust to every later edit of the video segment. A re-track updates
  every follower. One track can drive several overlays (text + arrow + blur).
  Undo is uniform: setting a track and attaching it are `EditCommand`s.
- Bad: a new render concept (moderate: one function in the compositor's
  transform evaluation). An orphan link (target segment deleted) needs
  validation: a warning, and the overlay falls back to its static transform.

**Recommendation: B, plus a "Bake to keyframes" command** for users who want
to edit the motion by hand. After a bake the link is removed.

**Storage.** The samples go **into the project document**, not only into the
cache. A track is edit data. Re-running a model on another GPU, or with another
model version, gives slightly different numbers, and the text would move.
Size: 30 fps × 60 s = 1800 samples × ~40 bytes ≈ 72 KB of JSON before
compression [est]. That is acceptable. To keep large projects small, drop
samples that linear interpolation reproduces within 0.1 px
(Ramer–Douglas–Peucker); this typically removes most of them [est].

#### Smoothing

Raw tracks jitter, and the jitter is most visible in scale and rotation. The
whole track is known after the analysis, so use an **offline** smoother, not a
real-time filter: a Kalman filter with a Rauch–Tung–Striebel backward pass, or
simply a Gaussian over time with a per-channel strength. Defaults: light on
position, stronger on scale, strongest on rotation. Never smooth across a
`USER_ANCHOR` (the user said "here exactly") or across a shot cut.

#### Editing the track afterwards

This is what makes tracking usable in practice (Mocha, Resolve and After
Effects all work like this):

- The track shows on the player as a path with one dot per frame. Frames with
  low confidence or `LOST` show in red, on the player and as a band on the
  timeline segment.
- The user goes to a bad frame and drags the box. That sample becomes a
  `USER_ANCHOR`.
- **"Re-track from here"** runs forward (or backward) from the anchor until the
  next anchor or the end. Between two anchors, track from both sides and blend
  the two results with weights by distance to each anchor. This removes drift.
- "Delete frames" marks a range as `LOST`. The follower holds or interpolates
  across it.

#### Caching

The cache is for speed. It is not a store of record.

- Key: `blake3(media identity, tracker id + version, analysis resolution,
  init box / anchors, range)`. Media identity = size + mtime + a hash of
  sampled bytes, so a moved file still hits the cache.
- Location: `workspace::paths::cache_root()/analysis/<media-hash>/…`.
  Deleting it loses nothing (the document has the samples).
- Content: raw per-frame results plus tracker state (KLT points, SAM memory
  features if they are small enough). This makes "re-track from frame N"
  start at once.
- Analysis frames: decode at a reduced size (long side 640–1024 px). Use the
  proxy file if one exists (`modules/proxy`). Decode is often the bottleneck,
  not the tracker.

### 2.6 The UI flow

1. **Start.** Select an overlay. The inspector shows a **Tracking** tab (as in
   CapCut). Alternative: select a video clip, then use a toolbar button
   "Track". The target video defaults to the topmost video segment under the
   overlay at the playhead. A dropdown lets the user change it.
2. **Mark.** The player enters tracking mode: a box with resize handles
   appears over the object at the playhead. Later (step T3): **click** the
   object, and SAM makes the box from the mask.
3. **Options.** Mode (Position / + Scale / + Rotation), direction (forward /
   backward / both), quality (**Fast** = classic or VitTrack on the CPU,
   **Accurate** = SAM on the GPU; greyed out with the download size if the
   model is not installed).
4. **Run.** A background job. The inspector shows a progress bar and
   **Cancel**. The timeline segment shows a filling band. The player **shows
   the frame being tracked, with the box**. The user sees at once if it goes
   wrong and can cancel. On cancel, the frames done so far are kept.
5. **Result.** The path appears on the player. The overlay follows at once.
   Red frames show where to check.
6. **Fix.** Scrub to a red frame, drag the box, press "Re-track from here".
7. **Detach / bake.** "Stop following" (keeps the current position) and "Bake
   to keyframes".

### 2.7 Command surface (engine)

Following the "everything is a command" rule in `CLAUDE.md`, all of this is in
a new engine module `modules/tracking/commands.rs`. The UI, the CLI and MCP
call the same functions.

```text
tracking_start(segment_id, at: Micros, init: TrackInit { box | point },
               opts: { direction, quality, mode }) -> JobId
tracking_cancel(job_id)
tracking_status(job_id) -> progress, frames_done, current_pose   // or via shell::Channel events
tracking_set_anchor(track_id, source_time, box)                  // EditCommand
tracking_retrack_from(track_id, source_time, direction) -> JobId
tracking_attach(overlay_id, track_id, mode, offset)              // EditCommand
tracking_detach(overlay_id)                                      // EditCommand
tracking_bake_keyframes(overlay_id)                              // EditCommand
```

The job writes into a **pending** track. When it finishes (or is cancelled),
one `EditCommand::SetTrack` puts the result into the document. So the undo
history gets one step, not 900. The job runner copies `modules/proxy/queue.rs`:
the worker holds no lock that the UI wants, and cancellation is a flag that is
checked per frame.

### 2.8 Recommendation and effort

| Step | What | Why first | Effort |
|---|---|---|---|
| **T1** | Document model (track material + follow link), compositor evaluation, the full UI flow (2.6), anchors and re-track. Tracker: **KLT + robust similarity fit** in pure Rust, with an NCC re-detection fallback and a confidence value. CPU only, analysis at 640 px. | Instant, no download, works on every machine, gives rotation. 80 % of the work is the model, the compositor and the UI. All later trackers reuse it unchanged. | **L** (~3 weeks): model + compositor M, UI M, KLT S–M |
| **T2** | **VitTrack** (0.7 MB ONNX, Apache-2.0) through `ort` on the CPU as the default "Fast" tracker. KLT inside its box for rotation. Optional: MixFormerV2-S if VitTrack is not robust enough (needs an ONNX export). | Fixes the thrown-ball case at almost no cost. The first real use of the ML runtime (section 5). | **S–M** after the ML foundation exists |
| **T3** | **SAM 2.1-tiny** (or EfficientTAM once an export exists) through `ort` CUDA / OpenVINO as the "Accurate" tracker, with **SAMURAI's motion-aware memory selection** ported (plain code). Click-to-select. The mask is also reused for cut-out and blur. | Occlusion, hard shots, and the mask. | **L** (ONNX video path, memory bank management, GPU paths) |
| **T4** | Planar tracking (homography from KLT + RANSAC) → corner pin. Needs a perspective-quad mode in the compositor. | Screen replacement, signs. CapCut has it (`surface_trackings`, `corner_pin`). | M |

Do **not** start with SAM. Its value depends on the T1 infrastructure, and its
GPU and export risks would hold back a feature that the classic tracker plus
VitTrack can deliver for most shots.

### 2.9 Features that come almost free with the tracker

- **Stabilise on the object** (CapCut `object_locked`): apply the inverse of
  the track to the video segment's own transform.
- **Blur or mosaic that follows** (faces, number plates): an effect segment
  that follows a track, with a mask from the box (T1) or from SAM (T3).
- **Auto reframe** (section 3.7) uses the same trackers to follow the subject.
- **Camera tracking** in CapCut is probably a global-motion solve (it moves
  overlays with the camera, not with one object). The 2D version of it is the
  same KLT code over the whole frame. A 3D camera solve is out of scope.

---

## 3. The other features

Each section: what CapCut has, the open options, how to run them, and the
recommendation.

### 3.1 Auto captions with word timestamps

**CapCut:** the Captions tab (auto, manual, SRT/LRC/ASS import), a transcript
editor (`TextEditor`, 22 QML), `speechsdk.dll` (SAMI, ByteDance's speech stack),
34 per-language caption-timing tables.

| Model | Licence | Size | Languages | RTX 3060 | Intel iGPU / CPU | Word times |
|---|---|---|---|---|---|---|
| **Whisper large-v3-turbo** (809 M params) | MIT (code and weights) | ~1.6 GB fp16, ~550 MB q5 | ~99 | ~20–40× real time with whisper.cpp CUDA [est] | ~2–5× real time with whisper.cpp Vulkan [est]; uses ~1.6 GB VRAM on an AMD card [src] | token timestamps + DTW alignment in whisper.cpp; good to ~50–100 ms [est] |
| **Parakeet-TDT-0.6B-v3** | **CC-BY-4.0** [src] | ~2.4 GB fp32, less quantised | 25 European languages, incl. German [src] | very fast (RTFx ~3300 on the leaderboard GPU) [src] | usable on CPU [est] | **native word and segment timestamps** from the duration head [src]; punctuation and capitals [src] |
| Whisper base / small | MIT | 142 MB / 466 MB | ~99 | very fast | CPU real time or better [est] | as above |

**Rust path:** `whisper-rs` (bindings to whisper.cpp; CUDA, Vulkan and CPU
back ends; quantised GGML models). Parakeet runs through `ort` (sherpa-onnx
publishes ONNX exports; sherpa-onnx has a C API, if we want its decoding). FFmpeg
8 also has a `whisper` filter, but our system FFmpeg is 6.1 [meas], and a
filter gives less control.

**Recommendation:** Whisper large-v3-turbo through whisper-rs as the default
(all languages). Offer Parakeet-v3 as a "fast European" option. Small models
(base) as the CPU-only fallback. Run Silero VAD first and transcribe only the
speech regions: faster, and fewer hallucinations in music and silence. Captions
become text segments on a caption lane (the build-out plan, wave 3) with
per-word times kept for karaoke-style highlight animation. **Effort M,
priority 1.**

### 3.2 Translation

**CapCut:** `AiTranslate` module, an audio translation tool in the audio
inspector, `ai_translates` and `multi_language_refs` materials.

| Model | Licence | Notes |
|---|---|---|
| **Opus-MT** (Marian, one model per language pair) | CC-BY-4.0 | ~300 MB per pair; fast on CPU; `candle` has a Marian example; ONNX exports exist |
| **MADLAD-400** (3B / 7B) | Apache-2.0 | 400+ languages; large (3B ≈ 2 GB quantised [est]); GPU recommended |
| Small instruction LLM (e.g. Qwen3-4B) | Apache-2.0 | understands subtitle context and line length limits; llama.cpp; ~2.5 GB q4 [est] |
| NLLB-200 | **CC-BY-NC-4.0** | **excluded** |

**Recommendation:** Opus-MT for the common pairs (small, CPU). Optional
LLM-based translation for quality. Translate whole sentences, then
redistribute the times. **Effort M, priority 2.** Audio translation (dubbing)
= captions + translation + TTS; that is a chain, not a new model.

### 3.3 Text-to-speech

**CapCut:** `pc_tts` strings, the TTS web tool, SAMI.

| Model | Licence | Size | Notes |
|---|---|---|---|
| **Piper** | code now **GPL-3.0** (OHF-Voice/piper1-gpl; the MIT repo was archived on 2025-10-06) [src]; each voice has its own licence | 20–60 MB per voice | real time on a Raspberry Pi [src]; German voices exist; GPL is no problem for us; check each voice's licence before listing it |
| **Kokoro-82M** | Apache-2.0 [src] | ~330 MB fp32, ~90 MB q8 [est] | 8 languages, **no German** [src]; very good English |
| **Chatterbox (Multilingual / Turbo)** | MIT [src] | ~0.5 B params [est] | 23 languages incl. German, zero-shot voice cloning [src]; needs a GPU to be fast |
| F5-TTS, XTTS-v2 | weights **NC** / CPML | — | **excluded** |

**Rust path:** all three run through `ort` (ONNX exports exist for Piper and
Kokoro). Piper and Kokoro need a phonemiser; espeak-ng is GPL-3.0, which is
fine for us.

**Recommendation:** Piper (CPU, many languages) plus Kokoro for English, then
Chatterbox for cloning as a GPU option. Business risk of voice cloning: abuse
complaints against the app; add a consent checkbox. **Effort M, priority 2.**

### 3.4 Background removal and person segmentation (video)

**CapCut:** "Hintergrund entfernen" with **automatic** (Pro), **custom**
(brush or click) and chroma key (capture `31-…`); `CCAiCutOutClipFilter`;
model `tt_matting` (278 references in the effect corpus). The draft stores a
**pre-rendered matte video** (`matting.mask_video_path`), not live inference.

| Model | Licence | Size | RTX 3060 | iGPU | Temporal stability | Scope |
|---|---|---|---|---|---|---|
| **RVM** (Robust Video Matting, MobileNetV3) | **GPL-3.0** (code) | ~15 MB | real time at 1080p and more [est from paper: 1080p on a GTX 1080 Ti at >100 fps] | ~10–20 fps at 720p [est] | **good** (recurrent) | people only |
| **BiRefNet** (lite / general / matting) | **MIT** [src] | Swin-T lite ~45 M; Swin-L ~220 M params | general: ~10 fps fp32, ~17 fps fp16 at 1024² on an RTX 4090 [src] → ~4–7 fps on a 3060 [est]; lite ~3× faster [est] | <1 fps (general) [est] | per frame → flicker | any object, best edges |
| MODNet | Apache-2.0 (repo) | ~25 MB | real time | real time at low res | medium | portraits |
| **SAM 2.1** (from 2.3) | Apache-2.0 | see 2.3 | ~20–30 fps [est] | 2–5 fps [est] | good | any object, from a click |
| MatAnyone | **S-Lab licence (NC)** [src] | — | — | — | very good | **excluded** |
| RMBG-2.0 (BRIA) | **CC-BY-NC-4.0** [src] | — | — | — | — | **excluded** |

**Licence note on RVM:** GPL-3.0 code is compatible with chukcut. It would
block a closed paid add-on that links it. For a GPL paid build it is fine.

**Recommendation:** RVM for "remove background" on people (fast, stable,
iGPU-capable). BiRefNet-lite for still images and objects. SAM 2.1 for
"custom removal" by click (shares T3). Like CapCut, **bake the matte** to a
cache file (an 8-bit grey video, e.g. FFV1 or lossless HEVC) and composite it
in the preview. Real-time inference during playback is a later optimisation.
**Effort M, priority 2.**

**Built (2026-10-04).** RVM MobileNetV3, fp32 ONNX, from the authors' release
`v1.0.0` (`https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx`,
14 975 696 bytes, SHA-256 `88d45312…cbbd2828`; repository licence GPL-3.0,
which covers the weights). Mattes are baked per source frame into
`~/.cache/chukcut/mattes/` as greyscale PNGs at 960 px (not a matte video:
frames land as the bake reaches them, so the preview shows them at once), and
the quad shader multiplies them into the clip's alpha. 15.6 ms per 540×960
frame on an RTX 3060 (CUDA 13), 99 ms on the CPU. Decision 0025.

**Built (2026-10-04, agent/ml2): objects and "Select object".**

- **BiRefNet lite** for "Objects": onnx-community's transformers.js export
  (`huggingface.co/onnx-community/BiRefNet_lite-ONNX`, commit `de15b22`,
  `onnx/model.onnx`, 224 005 088 bytes, SHA-256 `56000243…0f03333`, in the
  registry; MIT). Fixed `[1, 3, 1024, 1024]` input, ImageNet-normalised;
  the output is **logits** (no sigmoid in the graph). Measured on an RTX
  3060 (driver 610, CUDA 13 bundle): **426 ms per 960×540 frame**, of which
  ~32 ms is our resize in and out — so ~2.3 fps, slower than the estimate
  above. The fp16 file (115 MB) runs at 385 ms; not used (no CPU kernels for
  some nodes, fp16 overflow risk). **On the CPU it is unusable for video:**
  12–25 s per frame on four threads and 11 GB peak memory (6 GB with ORT's
  memory arena off). So it is GPU-only (`cpu_ok: false`): the bake refuses
  on the CPU in words.
- **MobileSAM** for "Select object" (Apache-2.0; ONNX by Acly,
  `huggingface.co/Acly/MobileSAM` commit `0d3b403`, MIT): the encoder takes
  HWC RGB 0..255 with the long side at 1024 and normalises and pads in the
  graph; the decoder is Segment Anything's single-mask export. 960×540,
  encoder + decoder: **71.6 ms on CUDA**, **727 ms on the CPU** (4 threads,
  load ~13); the decoder alone ~45–60 ms on the CPU, so more clicks on one
  frame are cheap (the worker keeps the last embedding). On a synthetic disc
  one click gave IoU 0.993, a box prompt 0.993.
- **Propagation.** SAM 2.1's video predictor needs its memory encoder and
  memory attention in ONNX; the published community exports cover only the
  image encoder and the prompt decoder (`onnx-community/sam2.1-hiera-tiny-ONNX`
  has `vision_encoder` and `prompt_encoder_mask_decoder`, nothing else), so
  the memory path was not feasible in this step. Instead: segment the
  clicked frame, track the mask's box with VitTrack forwards and backwards
  (with its whole-frame re-detection), and prompt SAM on every frame with
  the tracked box grown by 10 % plus the clicked points carried along with
  the box (`matting/object.rs`); the box's size stays within 0.7–1.4× of
  the clicked object's (VitTrack's box grows over busy backgrounds), the mask
  is cleared outside the prompt box, and carried clicks stay 15 % inside it.
  With a correct box SAM is near perfect here (IoU 0.999 on the moving
  square, box or box + point, single-mask decoder); the error is all in the
  box. Good for rigid and slowly deforming objects; a thin object crossing a similar background can lose parts, and
  occlusion drops the mask until VitTrack finds the object again.

### 3.5 Scene / shot detection

**CapCut:** toolbar "Szenen aufteilen" (free); macOS bundle resource
`SceneEditDetection`.

- **FFmpeg `scdet`** is in our system build [meas]. It is a classic score (SAD
  on frames). It is good for hard cuts and bad for dissolves.
- **TransNetV2:** MIT, ~30 MB, fast on a GPU and acceptable on a CPU at its
  48×27 input [est]. It also detects gradual transitions. Runs with `ort`.

**Recommendation:** `scdet` now (S). TransNetV2 when the ML runtime exists
(S). Output: markers, or "split at cuts". Auto reframe and smoothing also use
the shot cuts. **Priority 1** because it is cheap and other features need it.

### 3.6 Beat detection

**CapCut:** `beats` material, `markBeat` shortcut, `BeatsModel`.

- **Beat This!** (CPJKU, ISMIR 2024): MIT [src]. Small model 2.1 M params /
  ~10 MB; full model 20.3 M / ~80 MB [src]. Beats and downbeats, plain peak
  picking. ONNX exports exist. A Rust crate `beat-this` exists on crates.io
  [src] (not reviewed).
- madmom: code BSD, but its models are **CC-BY-NC-SA** → excluded.
- Classic spectral-flux onset detection: no model, weaker on beats.

**Recommendation:** Beat This! small through `ort` on the CPU. Output: beat
markers on the audio segment, and "snap to beat". **Effort S, priority 2.**

### 3.7 Auto reframe (subject-aware crop for 9:16)

**CapCut:** "Auto-Formatänderung" (Pro) in the video Basic tab;
`smart_crops` material, `SmartCropCoordinateUtils`; **771 MB of SmartCrop
models** in the user cache; a separate SmartCut web tool.

There is no single model for this. Google's **AutoFlip** (MediaPipe,
Apache-2.0) describes the method well: per shot, detect the important things
(faces, people, salient objects), choose a crop that holds them, and smooth the
crop path (stationary, panning or tracking per shot).

| Part | Option | Licence |
|---|---|---|
| Faces | YuNet (OpenCV zoo, tiny); MediaPipe BlazeFace | MIT; Apache-2.0 |
| People / objects | RT-DETR, D-FINE, RF-DETR | Apache-2.0 |
| | YOLOv8 / YOLO11 | **AGPL-3.0** — legally compatible with GPL-3.0, but the combined work becomes AGPL, and Ultralytics sells commercial licences. Avoid. |
| Shot cuts | 3.5 | |
| Path | offline smoothing with constraints (max pan speed, hold if motion is small) | own code |

**Recommendation:** detector at 2–5 fps of analysis (not every frame), the
tracker (section 2) between detections, offline path optimisation per shot.
The result is a crop/transform with keyframes that the user can edit.
**Effort M, priority 2.**

### 3.8 Silence and filler-word removal

**CapCut:** transcript-based editing (`TextEditor`, `Views/TranscriptEdit`).

- **Silence:** Silero VAD (MIT, ~2 MB, `ort` CPU, far faster than real time)
  [est], plus FFmpeg `silencedetect` [meas: in our build] as the no-model
  fallback. Output: a list of ranges → "remove silences" makes ripple deletes
  through `EditCommand`s, with a preview of what will go. **Effort S,
  priority 1.**
- **Filler words** ("um", "äh"): standard Whisper often omits them. That is
  the problem. CrisperWhisper transcribes them verbatim, but its licence is
  **CC-BY-NC** → excluded. Workarounds: an initial prompt that contains
  fillers (helps, not reliable); Parakeet keeps some fillers [est]; a small
  keyword detector trained by us is a research project. **Effort M,
  priority 3.**

### 3.9 Noise reduction and voice isolation

**CapCut:** audio inspector: loudness normalise, "Stimme optimieren", denoise
(Pro), **isolate voice** (free); `realtime_denoises`, `vocal_separations`
materials; `CCAudioNoiseFilter`, `CCAudioSamiFilter`.

| Task | Option | Licence | Runtime | Notes |
|---|---|---|---|---|
| Speech denoise | **DeepFilterNet3** | MIT or Apache-2.0 [src] | the Rust crate `deep_filter` (runs on `tract`, CPU) | real time on one CPU core [est]; 48 kHz; already Rust — the best fit in this document |
| Speech denoise | RNNoise | BSD | FFmpeg `arnndn` [meas: in our build] | needs a model file; lower quality |
| Voice / music separation | **HTDemucs (ft)** | MIT | `ort`; STFT/iSTFT must stay outside the ONNX graph | ~80 MB per model, 4 models in "ft" [est]; ~10–30× real time on a 3060 [est]; slow on an iGPU |
| Voice / music separation | BS-RoFormer / Mel-RoFormer | code MIT; **community weights have unclear licences** | `ort` | best quality today; check each checkpoint's licence |

**Recommendation:** DeepFilterNet as the denoise (S, priority 1). HTDemucs for
"isolate voice" and "remove vocals" as a baked render job (M, priority 2).

### 3.10 Video stabilisation

**CapCut:** "Stabilisieren" (Pro); `CCVideoStableFilter`; the draft stores
`stable.matrix_path` — precomputed per-frame transforms.

- **Classic (recommended):** vid.stab (GPL-2+) is in the Ubuntu FFmpeg
  (`vidstabdetect` / `vidstabtransform`) [meas]. Pass 1 writes transforms; we
  apply them in our compositor as a per-frame transform, exactly like CapCut's
  `matrix_path`. FFmpeg `deshake` also exists [meas].
- **Own path later:** the global-motion KLT from section 2 plus path smoothing
  plus crop. This gives a preview inside our compositor and one code base.
- **Gyroflow:** GPL-3.0 and written in Rust. Excellent, but it needs gyro data
  (GoPro, Sony, Insta360, phones with metadata). A good later integration for
  action-camera footage.
- **Learned stabilisers:** research quality, slow. Not worth it.

**Effort S (vid.stab) → M (own). Priority 1.**

### 3.11 Frame interpolation (smooth slow motion)

**CapCut:** "Optischer Fluss" (Pro) in video Basic; `CCLensVFIFilter`;
`complement_frame_config`.

| Model | Licence | Size | RTX 3060 at 1080p | iGPU at 1080p |
|---|---|---|---|---|
| **RIFE 4.x** (Practical-RIFE) | MIT | ~10–20 MB | RTX 3050 with TensorRT fp16: **~46 fps** (RIFE 4.6) [src: SVP forum] → ~60 fps on a 3060 [est] | ~2–6 fps with OpenVINO [est] |
| FILM | Apache-2.0 | ~130 MB [est] | ~2–5 fps [est] | too slow |
| FFmpeg `minterpolate` | LGPL | 0 | slow on CPU, artefacts | — |

**Recommendation:** RIFE through `ort` (TensorRT where present, else CUDA,
else OpenVINO). Bake to a cache file when speed < 1×. **Effort M, priority 3.**

**Built (2026-10-04, decision 0028).** RIFE v4 through `ort` in the worker,
the export `huggingface.co/walterlow/RIFE_fp32_timestep` (MIT, commit
`ee09066`, 22 MB): input `[1, 6, H, W]` (both frames, planar RGB 0..1) and a
scalar `timestep`, output `[1, 3, H, W]`; any size (the graph pads to 32).
`yuvraj108c/rife-onnx`'s RIFE 4.9 export (separate `img0`/`img1`/`timestep`
inputs) gave the same answer on a moving square (within 1 px at t = 0.25
and 0.5) but states no licence. Measured on the RTX 3060, fp32, CUDA
provider, release worker: 18 ms at 640×360, 70 ms at 720p, 173 ms at 1080p;
CPU 0.25 s at 360p, 1.2 s at 720p. That is well below the TensorRT fp16
estimate above: TensorRT and fp16 are the open speed-ups. Frames are baked
per (source frame, phase in 64ths) as JPEG into the cache and drawn in place
of the plain blend.

### 3.12 Upscaling and "optimise quality"

**CapCut:** "Qualität optimieren" (Pro, 3 uses per day for free users);
`super_resolution`, `quality_enhance` in `video_algorithm`.

- **Real-ESRGAN:** BSD-3-Clause. `realesr-general-x4v3` is small (~5 MB) and
  fast; `RealESRGAN_x4plus` is ~64 MB and slow (~1–3 fps for 1080p → 4K on a
  3060 [est]). Per-frame models flicker on video; the compact "v3" model
  flickers less [est].
- Real-CUGAN (MIT), for anime.

**Recommendation:** a baked render job with `realesr-general-x4v3`. **Effort
M, priority 3.** Video denoise (CapCut "Bildrauschen reduzieren",
`CCLensVideoDenoiseFilter`) has a classic answer already in our FFmpeg:
`nlmeans_vulkan`, `hqdn3d`, `bm3d` [meas]. Deflicker: FFmpeg `deflicker`.

### 3.13 Face and body landmarks

**CapCut:** effect packages require `tt_face`, `tt_face_extra`,
`tt_fsnew_base_jianying`, `tt_skin_seg`, `tt_skeleton`, `tt_depth_estimation`,
`tt_facefitting*` (counted in `~/git/x/effect-data`); the face model name
`perfect2`; capability flags `faceDetect`, `skeletonDetect`, `handDetect` and
more (`~/git/x/python_renderer_specs/EFFECT_FORMAT_SPEC.md`
in the RE repository).

| Need | Option | Licence |
|---|---|---|
| Face detect | YuNet; MediaPipe BlazeFace | MIT; Apache-2.0 |
| Face mesh (478 points) | MediaPipe Face Landmarker | Apache-2.0 (TFLite; convert to ONNX — unverified for every op) |
| Body / whole-body pose | RTMPose / RTMW (MMPose) | Apache-2.0 |
| | InsightFace models, dlib 68-point (iBUG data) | **non-commercial** → excluded |

**Recommendation:** only when an effect needs it (face stickers, retouch,
"blur faces"). **Effort M, priority 3.**

### 3.14 Colour auto-adjust and colour match

**CapCut:** Adjust → "Automatische Anpassung" (with intensity), colour match,
"protect skin tones".

No model is needed. Auto-adjust: robust percentiles for black and white
points, grey-world or white-patch white balance, a gentle contrast curve.
Colour match: Reinhard mean/standard-deviation transfer in Lab, or the
Monge–Kantorovich linear transform, against a reference frame. **Write the
result into our existing colour materials** (decision 0007: Adjust, curves,
LUT) so the user can see and edit it. That is better than an opaque filter.
**Effort S, priority 2.**

### 3.15 Retouch and beauty

**CapCut:** the whole "Retuschieren" tab (capture `33-…`): face / body / hair,
auto styles, skin (smooth, even, blemish removal, dark circles), face reshape
(slim, width, jaw, cheekbones…), eyes, nose, mouth, brows, make-up looks. It is
built from `tt_face` + `tt_skin_seg` and an algorithm graph
(`algorithmConfig.json` with `face` and `skin_seg` nodes); `CCMakeupFilter`.

Build: landmarks (3.13) + a skin mask + wgpu filters (guided or bilateral
filter for skin smoothing inside the mask, a landmark-driven mesh warp for
reshape). The common face-parsing model (BiSeNet on CelebAMask-HQ) has MIT
code but weights trained on a non-commercial dataset → a legal risk for paid
use. A skin mask from colour plus the face mesh polygon avoids that model.
**Effort L, priority 3.**

### 3.16 AI object removal (inpainting)

**CapCut:** "KI-Entfernen", `ai_in_painting_config`, `pc_inpainting` (59
strings).

LaMa (Apache-2.0) per frame flickers on video. ProPainter (good video
inpainting) has the **S-Lab NC** licence → excluded. No strong permissive video
inpainting model is known to us today. **Priority 3; revisit.**

### 3.17 Generative features (note only, out of scope)

CapCut has AI extend ("KI-Erweiterung", free), AI remix, AI stylise (a whole
inspector tab), eye contact (free), lip sync, relight (`smart_relights`), AI
motion, avatars and digital humans, Seedance video generation, an LLM editing
agent (`clipflow_sdk.dll`, "KI-Bearbeitung", "EditPilot"), AI music, AI cover
design. These are server-side or very large models, and many are paid credits
(`pc_aigc`, `credits_center`). They are out of scope. If they come later, they
belong behind a provider interface (local model or a user's own API key), not
in the engine.

---

## 4. CapCut's ML stack, for reference

From [engine-symbols.md](engine-symbols.md), the macOS stack report
(`~/capcut-libs/CAPCUT_STACK_REPORT.md`) and the effect corpus:

- **Runtimes:** ByteNN (`libbytenn.dylib` on macOS with CoreML + Metal;
  `bytenn_openvinowrapper.dll` on Windows) on top of **OpenVINO**
  (`openvino.dll` + an **NPU** plugin). `nuro.dll` (a ByteDance NN component).
  A small in-house tensor library with CUDA paths (314 `tl_*` exports).
  `NvCVImage` (NVIDIA Video Effects SDK interop).
- **ML framework inside the effect engine:** **Bach** (`BachAlgorithmSystem`,
  `BachAlgorithmModel`, `BachDownloadableResourceFinder`,
  `BachAlgorithmSystemWithDevice`) — models are **downloaded on demand** and
  dispatched **per device**.
- **Model delivery:** effect packages declare the models they need
  (`model_names: {"alg_model": ["tt_face", "tt_skin_seg", …]}`,
  `requirement` flags); `EffectPlatform` (`AlgorithmModelRecord`,
  `LokiRequirementsPeeker`) resolves and downloads them separately.
- **Separate ML libraries:** `lens.dll` 84 MB (video denoise, deflicker, frame
  interpolation), `speechsdk.dll` (SAMI: ASR, TTS), `Tracking.dll`,
  `fastcv.dll`, `metasecml.dll` (anti-abuse; irrelevant to us).
- **Device benchmarking:** `bytebench.dll` chooses codec and ML paths per
  machine.
- **Results are baked:** the draft stores paths to mattes, stabilisation
  matrices, reversed clips and algorithm outputs (`video_algorithm.path`).
- No `.onnx`, `.mnn` or other model files were found on this machine. The
  Windows install lives in the `win10` VM, and its model cache was not
  surveyed for this document. We do not need their models (and the legal
  boundary in `CLAUDE.md` forbids shipping them).

Lessons we copy: download models on demand, select the device per machine,
bake heavy results to disk, store edit-defining results in the document.

---

## 5. Recommended architecture

### 5.1 One inference crate, in a separate worker process

```text
crates/ml/            chukcut-ml: runtime wrappers, model registry, downloader,
                      device probe, tensor pre/post-processing helpers.
                      No UI dependency, no project types.
crates/ml-worker/     chukcut-ml-worker: a small binary. Links chukcut-ml and
                      FFmpeg, decodes its own frames, runs one job at a time,
                      writes results to the cache, reports progress on stdout.
crates/engine/        modules/tracking, modules/captions, modules/matting, …
                      each with commands.rs; they start jobs in the worker and
                      turn results into EditCommands.
```

Why a **separate process**:

1. **The GPU rule.** `CLAUDE.md` says: never open a second Vulkan device,
   because concurrent Vulkan instances crash drivers. whisper.cpp's Vulkan back
   end and ORT's WebGPU EP (Dawn) each open their own Vulkan instance. In
   another process this risk is contained. CUDA and OpenVINO do not use Vulkan,
   but the same isolation helps them too.
2. **Crashes and memory.** A model that runs out of VRAM, or a native crash in
   ORT, kills the worker, not the editor with the user's unsaved work.
   `claude-memguard` and the OS see a separate process with its own limit.
3. **Build and licence hygiene.** The heavy native dependencies (ORT, CUDA
   provider libraries, OpenVINO, whisper.cpp) stay out of the editor binary.
   An "ML pack" can be installed or not.

CapCut works the same way (`VEHelper.exe`, `taskcontainer.exe` as a sandboxed
task host). The price: frames do not cross the process boundary for free. For
analysis jobs the worker decodes the file itself (path + range), so only small
results cross. For real-time ML effects in the preview (a later step), use
shared memory (memfd) or DMA-BUF.

### 5.2 Runtimes: `ort` first, three exceptions

| Runtime | Use it for | Why |
|---|---|---|
| **`ort`** (ONNX Runtime; v2.0.0-rc.13 wraps ORT 1.28 [src]) | most models: trackers, SAM, matting, VAD, beats, TransNetV2, RIFE, ESRGAN, TTS, Parakeet | one format for everything; execution providers for every vendor; falls back per operator to the CPU [src] |
| **`whisper-rs`** (whisper.cpp) | Whisper | quantised models, a mature Vulkan + CUDA back end, built-in DTW word timestamps |
| **`deep_filter`** (tract) | DeepFilterNet | already Rust, CPU real time; no reason to re-wrap |
| **pure Rust / wgpu compute** | classic CV: KLT, NCC, colour statistics, stabilisation paths | no model, no download, runs in the engine itself |

Not chosen as the main runtime:

- **`candle`:** good CUDA support and many transformer examples (Whisper,
  Marian, SAM v1). No Intel or AMD GPU path, so it misses one of our two
  first-class vendors. Keep it as an option for single models.
- **`burn`:** its wgpu back end would run on every GPU in pure Rust, which is
  attractive. Its ONNX import does not cover the operators of models like SAM 2
  yet [est]. Watch it.
- **`tract`:** CPU only. Fine for small audio models.

### 5.3 Execution-provider selection

Probe once in the worker at start-up, cache the answer, show it in Settings →
Hardware (`workspace::hardware` already reports codecs; add ML devices there).

| Vendor | Provider order | Notes |
|---|---|---|
| NVIDIA | TensorRT → CUDA → CPU | ort ships CUDA 13 binaries and expects cuDNN ≥ 9 [src]. TensorRT builds an engine per model and GPU on first use (minutes) — cache the engines keyed by GPU + driver + model hash. |
| Intel | OpenVINO (GPU, then NPU) → CPU | CapCut uses OpenVINO too. The OpenVINO runtime is a separate download (~100–200 MB [est]). |
| AMD | MIGraphX → CPU | ORT removed the ROCm EP in 1.23; AMD now points to MIGraphX [src]. Best effort, as `CLAUDE.md` says for AMD. |
| any | WebGPU EP (Vulkan via Dawn) | experimental in ort and "may cause crashes" [src]. Interesting because it covers every vendor without CUDA or OpenVINO. Test it in the worker; never in the editor process. |

Keep a **tested matrix** per model × provider in the registry. Some models fail
or give wrong results on some providers (SAM 2's memory attention is a known
export risk). If a model is not marked as tested on a provider, it runs on the
next provider in the list.

**Shipping the runtime.** Use ort's dynamic loading. Bundle the CPU build of
ONNX Runtime (tens of MB [est]). Offer per-vendor **runtime packs** on first
use: "NVIDIA" (ORT CUDA/TensorRT provider libraries plus the CUDA and cuDNN
redistributables — large, ~1–2 GB [est]; check NVIDIA's redistribution terms;
or use the system CUDA if present), "Intel" (OpenVINO). This is the single
biggest packaging risk in this document. Measure the real sizes before
promising a download size in the UI.

**Measured (2026-10-04, agent/ml2).** The NVIDIA runtime pack is now a
*bundle* of NVIDIA's PyPI wheels next to ONNX Runtime's CUDA build:
CUDA 13 — ORT 241 MB, cudart 2.5, cuBLAS 439, cuRAND 61, NVRTC 53, cuDNN 9
537 MB: **1.33 GB download**, installed in 3 min 45 s here. CUDA 12 — ORT
424, cudart 3.5, cuBLAS 581, cuRAND 68, NVRTC 90, cuDNN 766: **1.93 GB**.
ORT 1.28's CUDA provider links only cudart, cuBLAS(Lt) and cuRAND
(`readelf -d libonnxruntime_providers_cuda.so`); cuDNN is opened at run
time, and the provider registers without it, then fails on the first
convolution — so the worker checks for `libcudnn.so.9` itself. cuFFT is not
needed. The CUDA 13 wheels put every library in `nvidia/cu13/lib/`, the
CUDA 12 ones in `nvidia/<library>/lib/`. Ubuntu's CUDA 12.0 cudart is too
old for ORT 1.28 (`cudaLibraryGetKernel` is 12.1+); preloading the wheel's
cudart by path keeps it out. Driver 580+ runs CUDA 13, 525+ CUDA 12.

### 5.4 Model registry and downloader

A registry file compiled into the binary (and updatable later):

```toml
[[model]]
id        = "vittrack"
version   = "2023sep"
task      = "track.box"
runtime   = "ort"
licence   = "Apache-2.0"
commercial_ok = true
files     = [{ url = "https://…/object_tracking_vittrack_2023sep.onnx",
               sha256 = "…", bytes = 712345 }]
providers_tested = ["cpu"]
```

- **Download on first use**, with the size and licence shown before the
  download. Only tiny CPU models (VitTrack ~0.7 MB, Silero VAD ~2 MB, Beat
  This! small ~10 MB) are worth bundling.
- URLs pinned to an immutable revision (a Hugging Face commit hash, a GitHub
  release asset). Verify SHA-256, download to a `.part` file, resume with HTTP
  range requests, then rename atomically.
- Location: models are **not** cache (they are large and costly to re-fetch,
  but they can be re-fetched): `~/.local/share/chukcut/models/<id>/<version>/`.
  "Clear cache" does not delete them; Settings has a separate model manager
  with sizes and a delete button.
- An "import model folder" path for offline machines.
- Outgoing requests use a neutral User-Agent (`chukcut/<version>`) and send no
  user data. The project's no-identity rule applies to the downloader too.
- **Legal boundary:** the registry may list third-party URLs; the repository
  never contains weights. This is the same rule as for effect packages.

### 5.5 Jobs, progress and cancellation

Copy the proxy queue's design (`modules/proxy/queue.rs`): `enqueue` does no
work; the worker owns no lock the UI wants; FIFO within a priority.

- Two priorities: **interactive** (tracking the clip the user is looking at)
  before **background** (captions for the whole project, mattes).
- **One GPU job at a time** (12 GB of VRAM is not much for SAM + Whisper + a
  TensorRT engine). CPU jobs run beside it with a thread limit.
- Progress through `shell::Channel`: `{job, fraction, frames_done, eta,
  preview_payload}` — for tracking, the payload is the current box, so the
  player can draw it live.
- Cancellation: a flag checked per frame or per audio chunk. Partial results
  are kept and are useful (a track up to the cancel frame, captions up to that
  point).
- Jobs write results in chunks, so a crash or a restart resumes from the last
  chunk.

### 5.6 Cache versus document

| Result | Where | Why |
|---|---|---|
| Tracks, transcripts with word times, beat markers, scene cuts, silence ranges, reframe paths, colour-match parameters | **project document**, through `EditCommand` | They define the edit. Re-running a model must not move things. They are small. They must survive moving the project to another machine. |
| Mattes, interpolated frames, upscaled frames, separated stems, denoised audio | **cache** (`cache_root()/analysis/…`), keyed by media hash + model id + version + parameters | Large, derived, can be re-built. The document stores the parameters and a "bake state". |
| Raw tracker state, SAM memory features, Whisper segments before editing | cache | speed only |

Export must never silently re-run a model with different results. If a baked
cache file is missing at export time, rebuild it with the **same model
version** the document records, or warn.

---

## 6. Phased roadmap

**Phase 0 — foundations without ML (S, start now).** These need no runtime:
scene cuts with FFmpeg `scdet`, silence ranges with `silencedetect`,
stabilisation with vid.stab + transforms in the compositor, colour auto-adjust
and colour match into the existing colour materials. Value at once, and they
exercise the job, progress and "result → EditCommand" paths.

**Phase 1 — tracking T1 (L).** Track material, follow link, compositor
evaluation, the full UI flow, anchors, re-track, smoothing, KLT tracker. This is
the owner's top feature, and it needs no ML runtime at all.

**Phase 2 — the ML foundation plus the cheap high-value models (M).**
`crates/ml` + `ml-worker`, registry, downloader, device probe, job queue. First
models: VitTrack (tracking T2), Silero VAD, DeepFilterNet, TransNetV2, Beat
This! small — all CPU, all small, all permissive. Then Whisper captions with
word timestamps (the first GPU model).

**Phase 3 — the GPU models (L).** SAM 2.1-tiny (tracking T3, click-to-select,
custom cut-out), RVM background removal, HTDemucs voice isolation, auto
reframe, translation, TTS.

**Phase 4 — quality features (M each).** RIFE slow motion, Real-ESRGAN, face
landmarks → retouch, planar tracking (T4), filler words.

**Not planned:** generative features (3.17).

---

## 7. Top five recommendations

1. **Build motion tracking as "track material + follows link", not as baked
   keyframes**, with anchors and "re-track from here" from day one. Start with a
   pure-Rust KLT tracker (no model), then add VitTrack (0.7 MB, Apache-2.0,
   2 ms/frame on the CPU) for fast motion, then SAM 2.1-tiny with SAMURAI's
   motion logic for occlusion and masks. Skip CoTracker3 (NC licence).
2. **Run all ML in a separate `chukcut-ml-worker` process on `ort`**, with
   whisper-rs and `deep_filter` as the two exceptions. This keeps foreign
   Vulkan instances, native crashes and gigabytes of CUDA libraries out of the
   editor.
3. **Ship the cheap wins first:** Whisper large-v3-turbo captions with word
   timestamps, silence removal (Silero VAD), DeepFilterNet denoise, scene cuts,
   vid.stab stabilisation. All have clean licences, and three of them need no
   model at all.
4. **Treat licences as a gate in the registry** (`commercial_ok`). Excluded
   today: CoTracker3, LiteTracker, NLLB, RMBG-2.0, MatAnyone, ProPainter,
   CrisperWhisper, F5-TTS, XTTS, InsightFace models, madmom models. Use YOLO
   (AGPL) only on purpose.
5. **Store edit-defining results in the document, bake heavy pixels to the
   cache**, and record the model version — the same split CapCut uses
   (`video_trackings` material vs. `mask_video_path`). An export must never
   change because a model was re-run.

---

## 8. Open questions — measure before committing

- Real speed of VitTrack and KLT on our test clips, including decode time at
  640 px (decode is probably the bottleneck).
- Does a community SAM 2.1-tiny video ONNX export run on ORT CUDA **and**
  OpenVINO, with the same masks as PyTorch? Real fps on the RTX 3060 and on an
  Intel iGPU.
- Real download sizes of the NVIDIA and Intel runtime packs; NVIDIA's
  redistribution terms for cuDNN.
- Whisper large-v3-turbo speed with whisper.cpp Vulkan on an Intel iGPU.
- What CapCut's `video_trackings` material contains: create one track in the
  VM, then read `draft_content.json`. Capture the overlay Tracking tab.
- Is the ORT WebGPU EP stable enough to be the single cross-vendor path?

---

## Sources

CapCut (local, static analysis): [engine-symbols.md](engine-symbols.md),
[ui-inventory.md](ui-inventory.md), [draft-format.md](draft-format.md),
[capcut-stack.md](capcut-stack.md), `docs/reference/capcut/README.md` and the
local captures `22-…`, `23-…`, `30-…`, `31-…`, `33-…`;
`~/capcut-libs/CAPCUT_STACK_REPORT.md`; `~/git/x/python_renderer_specs/EFFECT_FORMAT_SPEC.md`;
model names counted in `~/git/x/effect-data/*.json`.

CapCut (web):
- [miracamp] Motion tracking in CapCut — https://www.miracamp.com/learn/capcut/motion-tracking
- [antwort] Motion Tracking in CapCut PC — https://www.antwort.net/a/tutorial/motion-tracking-in-capcut-pc-die-ultimative-anleitung.html
- [filmora] Motion tracking in CapCut — https://filmora.wondershare.de/ai-efficiency/motion-tracking-in-capcut.html
- CapCut desktop AI features — https://www.capcut.com/tools/desktop-ai-power, https://www.capcut.com/resource/capcut-ai

Tracking:
- SAM 2 / 2.1 (sizes, fps, Apache-2.0) — https://github.com/facebookresearch/sam2, https://huggingface.co/zeromodels/sam2_hiera_small
- SAM 2.1-tiny video-path ONNX export notes — https://huggingface.co/jax-image-tools/sam21-tiny-video-onnx
- SAMURAI — https://github.com/yangchris11/samurai, https://arxiv.org/abs/2411.11922
- EfficientTAM — https://github.com/yformer/EfficientTAM
- SAM 3 — https://arxiv.org/abs/2511.16719, https://blog.roboflow.com/what-is-sam3/
- CoTracker3 (CC-BY-NC) — https://github.com/facebookresearch/co-tracker
- [lite] LiteTracker — https://github.com/ImFusionGmbH/lite-tracker, https://arxiv.org/abs/2504.09904
- TAPIR / BootsTAPIR / TAPNext (Apache-2.0) — https://github.com/google-deepmind/tapnet
- Cutie (MIT) — https://github.com/hkchengrex/Cutie ; XMem (MIT) — https://github.com/hkchengrex/XMem
- VitTrack (OpenCV zoo) — https://github.com/opencv/opencv_zoo/tree/main/models/object_tracking_vittrack
- MixFormerV2 (MIT) — https://github.com/MCG-NJU/MixFormerV2, https://arxiv.org/abs/2305.15896

Runtimes:
- ort — https://ort.pyke.io/perf/execution-providers, https://docs.rs/crate/ort/latest, https://github.com/pykeio/ort/releases
- ONNX Runtime 1.23 removes the ROCm EP — https://newreleases.io/project/pypi/onnxruntime/release/1.23.0
- whisper.cpp — https://github.com/ggml-org/whisper.cpp

Speech and audio:
- Parakeet-TDT-0.6B-v3 (CC-BY-4.0) — https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
- Piper moved to GPL-3.0 — https://github.com/OHF-Voice/piper1-gpl, https://www.promptquorum.com/power-local-llm/piper-tts-review
- Kokoro-82M — https://huggingface.co/hexgrad/Kokoro-82M
- Chatterbox (MIT) — https://www.resemble.ai/learn/models/chatterbox
- DeepFilterNet — https://github.com/rikorose/deepfilternet, https://docs.rs/crate/deep_filter
- Beat This! (MIT) — https://github.com/CPJKU/beat_this, https://docs.rs/crate/beat-this/1.0.0

Vision:
- BiRefNet (MIT, speeds) — https://github.com/ZhengPeng7/BiRefNet
- RMBG-2.0 (CC-BY-NC) — https://github.com/Bria-AI/RMBG-2.0
- Robust Video Matting (GPL-3.0) — https://github.com/PeterL1n/RobustVideoMatting
- MatAnyone (S-Lab licence) — https://github.com/pq-yang/MatAnyone
- RIFE speed on RTX 3050 (SVP forum) — https://www.svp-team.com/forum/viewtopic.php?pid=81485 ; Practical-RIFE — https://github.com/hzwer/Practical-RIFE
- Real-ESRGAN — https://github.com/xinntao/Real-ESRGAN
- TransNetV2 — https://github.com/soCzech/TransNetV2
- MediaPipe AutoFlip — https://github.com/google-ai-edge/mediapipe (docs: "AutoFlip: Saliency-aware Video Cropping")
