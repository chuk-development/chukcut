# Tracking

Tracking makes a title, a caption, a sticker or a photo follow an object or
a face in a video. The follower clip must be on a lane above the video.

The **Tracking** tab is in the inspector of image clips (photos and
stickers) and text clips (titles and captions). Select the clip that must
move, not the video.

## Follow an object

1. Put the follower clip over the video, at the time where the object is
   visible.
2. Select the follower. Open the **Tracking** tab.
3. **Track in**: choose the video. (When no video is under the clip, it says
   "No video under this clip".)
4. **Follow**: **Position**, **Position and scale**, or **Position, scale and
   rotation**.
5. **Tracker**:
   - **Standard**: chukcut's own tracker. It needs no model and runs fast on
     the CPU (90 to 130 frames per second for 720p).
   - **Fast motion (AI)**: the VitTrack model. Use it for fast or small
     objects, and for objects that leave the frame or go behind something.
     It finds the object again when it comes back. It runs on the GPU or the
     CPU. When the AI cannot run, chukcut uses Standard and says so.
6. Click **Select object**. Drag a box around the object on the player.
7. Click **Start tracking**. A progress bar shows "Tracking… N of M frames",
   with **Cancel**.

When the clip follows, the player draws the path of the track. The summary
line says how many frames were tracked and how many are doubtful.

### After tracking

- **Smoothing** (0 to 100): makes the motion calmer.
- **Re-track from here**: adjust the box on the player where the track
  drifted, then track again from the playhead.
- **Bake to keyframes**: turns the motion into ordinary position keyframes.
  The clip then no longer depends on the track.
- **Stop following**: the clip stays where it is now.
- **Remove track**: deletes the track.

The follower stays on the object when you trim, split, move or change the
speed of the tracked video. If you delete the tracked video, the follower
says **Target missing** and stays still. Undo the delete to get the motion
back.

## Follow a face

1. Select the follower clip. Open the **Tracking** tab.
2. **Track in**: choose the video with the face.
3. **Pin to**: **Face**, **Eyes**, **Forehead**, **Nose**, **Mouth** or
   **Chin**.
4. Click **Follow face**. The button says "Finding the face…" while it
   runs.

chukcut finds the face in every frame with two models (YuNet and the
MediaPipe face mesh), and the clip rides with it. The track has 30 %
smoothing. It is one undo step. A face track has no **Tracker** row and no
**Re-track from here**.

Cost: about 6 s for 300 frames of 1080×1920 video, on the GPU or the CPU.
The face points are cached in `~/.cache/chukcut/landmarks/`.

## Follow a body part

1. Select the follower clip. Open the **Tracking** tab.
2. **Track in**: choose the video with the person.
3. **Body part**: **Head**, **Shoulders**, **Chest**, **Hips**, **Whole
   body**, or a hand, an elbow, a knee or a foot. Left and right are the
   person's own: when the person looks at the camera, the left hand is on
   the right of the picture.
4. **Person**: **Person 1** is the first person that chukcut sees in the
   clip, **Person 2** the second.
5. Click **Follow body part**. The button says "Finding people…" while it
   runs.

chukcut finds the people in every frame with two models (YOLOX and
RTMPose), and the clip rides with the part. A hand turns with the forearm.
The track has 40 % smoothing. It is one undo step. When the part is hidden
in a frame, the clip stays where it was. A body track has no **Tracker**
row and no **Re-track from here**.

Cost: about 2 s for 60 frames of 1280×720 video, on the GPU or the CPU. The
models download once (48 MB and 18 MB). The body points are cached in
`~/.cache/chukcut/landmarks/`.

## From the command line

`chukcut-cli track`, `chukcut-cli follow-face` and `chukcut-cli
follow-body` do the same. See
[`docs/cli.md`](../cli.md).
