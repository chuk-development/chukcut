# Timeline editing

The keys on this page are the keys of the default preset (**chukcut**). You
can change them. See [Keyboard shortcuts](shortcuts.md).

## The toolbar

Left side:

- **Tool**: **Select** (`A`) or **Split** (`B` or `C`). With the split tool, a click
  on a clip cuts it there.
- **Undo** (`Ctrl+Z`), **Redo** (`Ctrl+Shift+Z`).
- **Split** (`S` or `Ctrl+B`): cuts the clip at the playhead.
- **Delete left** (`Q`) and **Delete right** (`W`): delete the part of the
  selected clip before or after the playhead.
- **Delete** (`Delete`): deletes the selection.
- **Marker** (`M`): adds a marker at the playhead, or removes it.
- **Record voiceover at the playhead**: see [Audio](audio.md#voiceover).

Right side:

- **Main track magnet** (`P`): clips on the main lane stay together with no
  gaps.
- **Snapping** (`N`): clips snap to cuts, markers and the playhead.
- **Zoom to fit** (`Shift+Z`), **Zoom out** (`Ctrl+-`), the zoom slider,
  **Zoom in** (`Ctrl+=`).

## Moving around

- Click the ruler to move the playhead. The playhead always lands on a
  frame.
- `Space` plays and pauses. `J`, `K` and `L` shuttle backward, stop and
  forward. Press `L` again for 2x, 4x and 8x.
- `←` and `→` step one frame. `Shift+←` and `Shift+→` step ten frames.
- `Home` and `End` go to the start and the end.
- The mouse wheel scrolls the time. `Ctrl` + wheel zooms. `Shift` + wheel
  scrolls the lanes up and down.

## Lanes

From top to bottom: titles, effect clips, captions, the video lanes, the
main lane, and the audio lanes. Each lane header has:

- **Lock track** (all lanes): a locked lane cannot change.
- **Hide track** (all lanes except audio): the lane is not drawn.
- **Mute track** (video and audio lanes).

The main lane has a **Cover** tile at its left.

## Select

- Click a clip to select it.
- `Ctrl` + click adds a clip to the selection or removes it.
- `Shift` + click selects along a lane.
- Drag a box over empty space to select the clips in the box.
- `Ctrl+A` selects all clips. `Esc` clears the selection.

A move, a trim or a delete of many clips is one undo step.

## Move and trim

- Drag a clip to move it, also to another lane.
- Drag the left or the right edge of a clip to trim it.
- When an edge almost touches a neighbour, chukcut puts it flush against
  the neighbour.

## Cut, copy, paste

`Ctrl+X`, `Ctrl+C`, `Ctrl+V` and `Ctrl+D` (duplicate). A paste lands at the
playhead, on the clip's own lane, or on the next free lane. It never covers
another clip.

## The clip menu

Right-click a clip:

1. **Split**, **Freeze frame**, **Replace media…**, **Delete**,
   **Duplicate**
2. **Copy**, **Cut**, **Paste**
3. **Detach audio**, **Link**, **Unlink**
4. **Reset speed**
5. **Create compound clip**, **Open compound clip**, **Put clips back**
   (see [Timelines and compound clips](compound-clips.md))
6. Analysis: **Detect scenes**, **Split at scene changes**, **Stabilise**,
   **Detect beats**, **Auto-cut to beat**, **Snap cuts to beats**, **Auto
   reframe**, **Reframe project to 9:16** (or to 16:9). **Cancel analysis**
   shows while an analysis runs.
7. **Duck under speech**, **Remove ducking** (sound clips on an audio lane;
   see [Audio](audio.md#ducking))
8. **Select all**

**Freeze frame** cuts the clip at the playhead and puts a 3 s still of that
frame in between. **Replace media…** swaps the file of a clip and keeps its
place, length and settings.

## Linked audio and video

A file with picture and sound gives two linked clips. They move, trim,
split and delete together.

- **Detach audio** makes the sound a separate clip.
- **Unlink** keeps both clips but lets them move alone. **Link** joins the
  selected clips again.

## Fades

Point at a sound clip (or select it) with the select tool. Two round dots
show near its top corners. Drag a dot inwards to set the fade in or fade
out. The **Audio** tab of the inspector has the same values as numbers.

## Keyframes

Clips show their keyframes as small diamonds near the bottom edge. Click a
diamond to select it and move the playhead to it. Drag it to move it.

In the inspector, the properties **Scale**, **Position**, **Rotate** and
**Opacity**, and the crop in **Video › Crop**, have a diamond button: click
it to add a keyframe at the playhead. The arrows beside it jump to the previous or next keyframe.
Right-click a diamond to choose the easing. The **Keyframe easing** section
in **Video › Basic** has a graph to shape the move between two keyframes,
and a list: **Linear**, **Hold**, **Ease in**, **Ease out**, **Ease
in-out**, **Smooth**, **Snap**, **Anticipate**, **Overshoot**, **Overshoot
both**, **Elastic**, **Bounce**, and **Custom** for a curve that you drew.

## Transitions

A transition sits on a cut between two clips, as a badge. Click the badge to
select it. Drag its edge to change its length. See
[Effects and transitions](effects-and-transitions.md#transitions).

## Markers, in and out

- `M` adds or removes a marker at the playhead. Markers are flags on the
  ruler.
- `I` and `O` set the in and out marks. They show as a band on the ruler.
  `Alt+X` clears them.
- `Ctrl+L` loops playback between in and out.
- The export can render only the part from in to out.

## Text on the timeline

Double-click a title or a caption to edit its text on the timeline. `Enter`
keeps the text, `Shift+Enter` adds a line, `Esc` cancels.

## Analysis tools (no AI model)

These are in the clip menu, and in the inspector's **Video › Basic** tab.

- **Stabilise**: measures the camera shake once, then holds the picture
  still, zoomed in to hide the moving edges. **Strength**: **Light**,
  **Medium**, **Strong** or **Tripod**. **Crop**: **Auto**, **5 %**, **10 %**
  or **20 %**. **Analyse again** measures again.
- **Scene detection**: **Detect scenes** finds the cuts in a clip. **Split
  at scene changes** splits the clip there. **Clear marks** removes the
  marks.
- **Auto reframe**: choose a shape (**9:16**, **1:1**, **4:5**, **16:9**)
  and click **Reframe this clip**. chukcut switches the project to that
  shape and follows the subject of each full-frame clip with position
  keyframes.
- **Beats**: see [Audio](audio.md#beats).
