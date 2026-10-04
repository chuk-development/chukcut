# Timelines and compound clips

## Several timelines in one project

A project can hold several timelines. They show as tabs above the lanes,
for example **Timeline 01**.

- **+** (tooltip **New timeline**) adds an empty timeline.
- Click a tab to open that timeline.
- Double-click a tab to rename it.
- Right-click a tab: **Rename…**, **Duplicate**, **Delete**. You cannot
  delete the last timeline.

All timelines use the same media. **Export** renders the timeline that is
open. Deleting a timeline also deletes the compound clips that only that
timeline used. It is one undo step.

## Compound clips

A compound clip holds other clips, like a folder. On the timeline it is one
clip. You can move, trim, grade, speed up and animate it like a video clip.

### Make one

Select the clips, then press `Alt+G`, or right-click and choose **Create
compound clip**.

### Edit inside

- Double-click the compound clip, or right-click it and choose **Open
  compound clip**.
- The tabs change to breadcrumbs, for example **Timeline 01 › Intro
  titles**. Click a crumb to go back to that level. The **<** button
  (**Close compound clip**) goes up one level.
- Opening and closing are steps in the undo history. To undo an edit that
  you made inside a compound clip, open the compound clip first.

### Put the clips back

Select the compound clip and press `Alt+Shift+G`, or right-click and choose
**Put clips back**. The clips go back onto the timeline, at the compound
clip's place. If the compound clip has a speed or a speed curve, the clips
get the same speed. chukcut refuses (and says why) for a curved clip inside a
curved compound clip.

### What works on a compound clip

The inspector shows **Video** (**Basic**, **Mask**), **Audio**, **Speed**,
**Animation**, **Adjust** and **Effects**.

- Grade, effects, masks, blend modes, animations and keyframes.
- Speed and speed curves. The sound inside follows the speed.
- Volume, fades and the audio effects of the **Audio** tab. chukcut mixes
  the sound of the compound clip into one file in the background, then
  applies the effects to that file.
- Scene detection, beat detection, stabilisation and auto reframe look at
  the contents.
- The AI tools of **Video › Remove background**, **Retouch** and **Enhance**
  are not offered for compound clips. Use them on the clips inside.

### Limits

- Nesting stops at 8 levels. A compound clip cannot contain itself.
- An older chukcut version that opens a project with several timelines
  loses all timelines except the open one.

## From the command line

`chukcut-cli timeline …` and `chukcut-cli compound …` do the same. See
[`docs/cli.md`](../cli.md#timelines-and-compound-clips).
