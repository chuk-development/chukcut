# 0032 — Body landmarks are a cached track from RTMPose; a followed body part is a motion track; zipped models unpack one pinned member

Date: 2026-10-04. Status: accepted (agent/body).

## What was decided

**Body pose is top-down: YOLOX finds people, RTMPose reads each one.**
Both models run in the ML worker and both have the Apache-2.0 licence:

- *The detector* is YOLOX-tiny, trained by OpenMMLab on Human-Art
  (`yolox-tiny-human`, 20 MB). Input: the frame in a 416² letterbox, BGR
  0..255, padded with grey 114. Output: boxes with a score. Non-maximum
  suppression is in the graph.
- *The pose model* is RTMPose-m body7, 256×192 (`rtmpose-m`, 54 MB). Input:
  a crop around a person, 1.25 times the box, with a 3:4 shape. Output:
  SimCC rows for COCO's 17 keypoints. The worker finds the peak of each row
  and refines it with a parabola. A bin is half a crop pixel, which is
  2.6 frame pixels on a person 1000 px tall. Without the refinement, a
  sticker on a hand shakes.

The worker follows people the way it follows faces (decision 0030). The
keypoints of a person in one frame give the region for the next frame
(the box of the seen keypoints, 1.25 times larger). The detector runs only
on the first frame, when all people are lost, and when the engine asks
for newcomers (`search`, once a second). Protocol: `detect_people`,
`body_landmarks`.

**The keypoints are cache; a followed body part is an ordinary motion
track.**

- *The track* is one binary file per media file and model version, next
  to the face tracks (`~/.cache/chukcut/landmarks/<digest>-rtmpose-<version>.bdy`).
  Each frame holds its people: an id, a score, 17 × (x, y as 1/65535 of
  the frame, confidence as 1/255). That is 90 bytes per person per frame.
  A background job in `analysis::jobs` (kind `Body`) writes it.
- *Identity.* The engine gives each person an id: the order in which the
  analysis first sees them, the largest first. A box that overlaps (IoU ≥
  0.2) a person seen in the last 2 s keeps that person's id. Thus
  "Person 1" is the same person in each frame, also after a frame where
  the analysis missed them.
- *Follow a body part* writes the pose of the part as a `TrackingMaterial`
  stamped `body` (40 % smoothing), and attaches the overlay with the usual
  follow link, in one undo step. The parts are the head, the shoulders, the
  chest, the hips, the whole body, and the hands, elbows, knees and feet on
  each side. Left and right are the person's own (COCO's convention). A
  limb part turns with its limb (0° when the limb hangs down). The head and
  the torso parts turn with the line from the right side to the left side,
  as the eye line of a face does. A frame where the part is not seen holds
  the last pose, flagged lost.

**Auto reframe uses the person detector when no face is visible.** On a
frame where YuNet finds no face, the reframe job asks for people. The top
fifth of the box of a person (the middle two fifths of its width) becomes
a "face" with 0.8 of the weight of the person's score. The window then
holds the head and the shoulders, with the head room of a face. The job's
message gives the share of frames that used people.

**A model can come inside an archive.** `registry::PACKED_MODELS` names
the archive kind (zip or tar.gz), its size and SHA-256 (in the model's
`ModelSpec`), the one member that is the model, and the SHA-256 of that
member. The download is verified as a whole first, because the directory
of a zip is at its end. Then the engine copies only the named member, to
the model's own path. The size of the member must agree with the registry
before the copy, and the copy stops at that size plus one byte. The
SHA-256 of the copied member must agree with the registry. The engine
deletes the archive in all cases. No other entry is written, thus a path in
the archive (`../`, absolute, a symlink) cannot reach the file system. A
download that becomes larger than its pinned size stops.

## Why

- **Top-down RTMPose** over a bottom-up model (RTMO, OpenPose): it is
  accurate on one to three people, which is what a creator films; a crop
  per person costs 7 ms on the RTX 3060 and 22 ms on the CPU. RTMO's
  single pass would be better for crowds and was not needed.
- **YOLOX, not YOLOv8**: Ultralytics' weights are AGPL-3.0. YOLOX-tiny is
  the detector RTMPose's own pipeline (`rtmlib`) uses, in the same
  archive set.
- **Keypoints as cache, the follower as document**: decision 0019's line,
  as for faces (0030).
- **One member out of a verified archive**: OpenMMLab publishes its ONNX
  files only in zips. Extracting the whole archive would give the archive
  control over file names; one pinned member with two checksums gives it
  none.

## What it costs

- **Training data.** RTMPose's "body7" includes datasets for research use
  only, and Human-Art (the detector's data) has its own terms. OpenMMLab
  publishes both weights under Apache-2.0. This is a business risk to
  weigh before a paid tier, not a licence violation. The research
  (`docs/research/ml-features.md` §3.13) names it.
- **The detector is slow on CUDA** for its size: 24 ms for a 1280×720
  frame (55 ms on the CPU), because the graph's non-maximum suppression
  runs on the CPU. It runs about once a second, thus an analysis is
  decode-bound: 60 frames of 1280×720 in 2.1 s on CUDA, 2.7 s on the CPU.
- **17 keypoints have no fingers and no face.** "Hand" is the wrist moved
  0.3 forearm lengths further. Faces stay with the face mesh.
- **Identity is box overlap only.** Two people who cross can change ids.
  "Person 2" can then follow the wrong person for the rest of the clip; the
  user picks the other person.
- **The zip reader keeps one of two entries with the same name.** The
  member's SHA-256 is the guard: a duplicate with other bytes cannot pass.

## What would change our minds

- Followers on fingers or a held object: RTMW (133 whole-body keypoints,
  Apache-2.0, same family, 4× larger).
- A crowd: RTMO, one pass for all people.
- A detector without CPU fallbacks on CUDA: an export without the in-graph
  NMS, with the suppression in Rust.
