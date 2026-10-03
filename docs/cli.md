# chukcut-cli: the command line and the MCP server

`chukcut-cli` edits chukcut projects without the window. Use it from a shell, a
script, a build pipeline or an AI agent. Every command calls the same engine
functions as the app. A project that the CLI writes is a project that the app
opens, and the reverse.

`chukcut-cli mcp` gives the same operations to an MCP client, for example
Claude Code. DaVinci Resolve 21.1 made its scripting API a Studio-only (paid)
feature. In chukcut, scripting is free and part of the build.

- [Build](#build)
- [How it works](#how-it-works)
- [Commands](#commands)
- [Batch files](#batch-files)
- [The MCP server](#the-mcp-server)
- [Recipes](#recipes)
- [Limits](#limits)

## Build

```bash
cargo build --release -p chukcut-cli
./target/release/chukcut-cli --help
```

The CLI uses the same system packages as the app (see the README). It has
offline transcription (whisper.cpp) by default. `--no-default-features` builds
it without whisper.cpp. `--features cuda` adds the CUDA backend.

Copy the binary to a folder on your `PATH`, for example `~/.local/bin`, to use
it as `chukcut-cli`.

## How it works

### One command, one edit, one save

Each command:

1. opens the project file,
2. applies the operation through the engine's command layer (the same
   functions the app calls; every edit is an `EditCommand` with an undo),
3. validates the document,
4. saves it with a temporary file and a rename, so a crash cannot leave half a
   file.

When the operation fails, or when the result does not validate, the CLI does
not write the file. The file keeps its last good state.

`--dry-run` runs the command and does not save. Use it to see what an edit
does.

### Naming clips, lanes, media and effects

`info` lists every lane and every clip. Each clip has a `ref` and an `id`:

```text
lane 0 "Video 1" (video)
  0:0    9967430b  video      0.000 – 2.000    a.mp4  [graded]
  0:1    4b29c932  video      2.000 – 4.000    a.mp4
lane 2 "Text 1" (text)
  2:0    bd493650  title      0.500 – 2.500    Hello
```

| To name a… | write |
|---|---|
| clip | its full id, a unique prefix of 4 or more characters (`9967430b`), or `lane:index` (`0:1` is the second clip on lane 0, in time order) |
| lane | its index (`0`), its name (`"Video 1"`, any case), or its id |
| imported media | its id, its file name (`a.mp4`), or its path |
| effect on a clip | its index in the clip's stack (`0`), its kind (`glow`), or its id |

A `lane:index` ref changes when clips move. An id never changes. Scripts that
do many edits must keep the ids that the commands return.

Lanes are numbered from the bottom of the stack. A higher lane paints over a
lower lane.

### Times

Times are on the timeline, unless the option says otherwise.

| You write | It means |
|---|---|
| `2.5` or `2.5s` | 2.5 seconds |
| `250ms` | 250 milliseconds |
| `1500000us` | microseconds, the engine's own unit |
| `1:02.5`, `01:02:03.25` | minutes:seconds, hours:minutes:seconds |
| `45f` | 45 frames at the project's frame rate |

In JSON (batch files, MCP), a number is seconds and a string can use all the
forms above.

### Colours

`#rrggbb`, `#rrggbbaa`, `#rgb`, `r,g,b` or `r,g,b,a` with values from 0 to 1,
or a name: `white`, `black`, `red`, `green`, `blue`, `yellow`, `transparent`.

### Positions

Positions are in canvas units. `0,0` is the centre. `1` is the edge: `x 1` is
the right edge, `y 1` is the top edge. Positions stay correct when the canvas
size changes.

### Output

Without `--json`, the CLI prints one line for a person. `info`, `catalog`,
`captions list` and `batch` print a short table.

With `--json`, stdout gets one JSON document and nothing else:

```json
{
  "ok": true,
  "op": "split",
  "message": "split at 2.000 s; 1 new clip(s)",
  "saved": true,
  "data": { "at": 2.0, "created": [ { "id": "4b29c932-…", "ref": "0:1", "start": 2.0, … } ] }
}
```

On a failure:

```json
{ "ok": false, "error": { "kind": "refused", "code": 1, "message": "nothing to split: no clip crosses the playhead" } }
```

Progress goes to stderr: a line that updates on a terminal, one line for each
step in a pipe, and JSON lines (`{"progress": 0.42, "label": "Encoding frame 88 of 210 (31 fps)"}`)
with `--json`.

`-v` and `-vv` show engine diagnostics on stderr. `RUST_LOG` overrides them.

### Exit codes

| Code | Kind | Meaning |
|---|---|---|
| 0 | | Done |
| 1 | `refused` | The engine refused the edit: a collision, a value out of range, nothing to undo. Nothing was saved. |
| 2 | `usage` | The arguments are wrong: an unknown clip, option or name. |
| 3 | `project` | The project file cannot be read, written or created. |
| 4 | `invalid` | The edit made the document inconsistent, or `validate` found errors. Nothing was saved. |
| 5 | `render` | An export or a frame render failed (for example, no GPU). |

### What the CLI does not touch

- **The app's working copy.** The app writes a crash-recovery copy of the open
  project after every edit. The CLI turns this off. A CLI run cannot make the
  app "restore" a document that it did not have.
- **The preview cache.** The app can run at the same time.
- **Proxies.** The CLI does not start proxy encodes.

The CLI reads your settings: the caption panel's transcriber, your cloud
accounts and the LUT library. It uses the same XDG folders as the app
(`~/.config/chukcut`, `~/.cache/chukcut`, `~/.local/share/chukcut`).

## Commands

Every command except `catalog` and `mcp` takes the project file first. The
global options `--json`, `--dry-run` and `-v` go anywhere on the line.

### Project

#### `new PROJECT`

Makes an empty project with one video lane and one audio lane.

| Option | Default | |
|---|---|---|
| `--name` | the file name | |
| `--width`, `--height` | 1080, 1920 | canvas in pixels |
| `--fps` | 30 | |
| `--force` | | replace a file that exists |

The first video that you import into an empty project sets the canvas shape
and the frame rate, as in the app. A 16:9 clip makes a 1920x1080 project. Use
`configure` after the import to change it.

```bash
chukcut-cli new reel.chukcut
```

#### `info PROJECT [--full]`

The summary: canvas, frame rate, duration, each lane with its clips (ref, id,
kind, source, start, end, in, out, speed, volume, transform, effects, grade,
transition, animation, keyframes), the imported media, validation issues and
the undo state. `--full` gives the complete project document instead.

```bash
chukcut-cli info reel.chukcut --json | jq '.data.tracks[0].clips[].id'
```

#### `validate PROJECT`

Lists warnings (for example, missing media) and errors (an inconsistent
document). Exit code 4 when there are errors.

#### `configure PROJECT`

`--name`, `--width`, `--height`, `--fps`, `--background COLOUR`. One undo
step. A new frame rate does not move clips: all times are in microseconds.

```bash
chukcut-cli configure reel.chukcut --width 1080 --height 1920
```

#### `import PROJECT FILE... [--append]`

Adds video, image or audio files to the project's media. With `--append`, it
also puts each file at the end of the first lane of its kind. Import is not an
undo step (the same as in the app). Importing the same file again gives the
same media id.

```bash
chukcut-cli import reel.chukcut take1.mp4 take2.mp4 music.mp3 --append
```

#### `catalog KIND`

Lists what you can use. Does not need a project.

| Kind | Lists |
|---|---|
| `effects` | effect ids and their parameters (ranges, defaults, choices) |
| `audio` | audio effect ids and their parameters (ranges, defaults, units) |
| `transitions` | the built-in kinds and the library presets |
| `animations` | clip presets (which slot each one fits), text presets, easings |
| `grade` | every grade control name and its resting value |
| `presets` | export presets, built in and your own (`presets PROJECT` also shows how each fits a project) |
| `hardware` | hardware encoders, and which ones work on this machine |
| `models` | local transcription models, and which ones are downloaded |
| `luts` | `.cube` files in the LUT library |
| `fonts` | font families that the text renderer can draw |
| `masks` | mask shapes, mask operations and the mask values you can set |
| `blend` | blend modes |
| `title_styles` | title styles for `title style` (id, name, category, sample words) |
| `title_templates` | text templates for `title template` (a style and an animation) |
| `layouts` | split-screen layouts and picture-in-picture corners |
| `speed_presets` | speed-ramp presets for `speed-curve`, with their shapes |
| `accounts` | the cloud accounts that the app has, and what each one can do; never a key |

### Timeline

#### `append PROJECT MATERIAL`

Puts imported media at the end of the first unlocked lane of its kind. A still
image gets 3 seconds.

#### `place PROJECT MATERIAL --at TIME`

Puts imported media on the timeline at a time.

| Option | |
|---|---|
| `--track LANE` | the lane; it must be the right kind and free |
| `--duration TIME` | the length; default is the rest of the media |
| `--from TIME` | where in the media the clip starts reading |

Without `--track`, the CLI uses the first lane of the right kind that has
space. When no lane has space, it adds a lane above the last lane of that kind.

```bash
# A 2-second cutaway from 10 s into broll.mp4, over the main clip at 4 s.
chukcut-cli place reel.chukcut broll.mp4 --at 4 --from 10 --duration 2
```

#### `split PROJECT --at TIME [--clip CLIP]`

Cuts a clip in two at a time. Without `--clip`, it cuts every unlocked clip
under that time (the app's split at the playhead). Linked audio is cut with
its picture. The result lists the new clips.

#### `delete PROJECT CLIP... [--ripple]`

Removes clips. `--ripple` moves the later clips on the lane to the left to
close the gap. When a tracked overlay follows a clip that you delete, the CLI
first bakes the overlay's motion to keyframes, so the overlay keeps its motion.

#### `move PROJECT CLIP --to TIME [--track LANE]`

Moves a clip. The CLI refuses a place where the clip overlaps another clip.

#### `trim PROJECT CLIP`

| Option | |
|---|---|
| `--head TIME` | move the start edge to this timeline time |
| `--tail TIME` | move the end edge to this timeline time |
| `--duration TIME` | give the clip this length; the start stays |
| `--ripple` | later clips on the lane move with the clip's end; a head trim keeps the start in place |

The CLI clamps the edges to the media and to a minimum of one frame, as the
app does when you drag.

```bash
chukcut-cli trim reel.chukcut 0:0 --head 0.4 --tail 3.2 --ripple
```

#### `set PROJECT CLIP`

`--x`, `--y` (position), `--scale`, `--rotation` (degrees, clockwise),
`--opacity` (0 to 1), `--flip-h true|false`, `--flip-v true|false`,
`--volume` (linear gain), `--speed` (playback rate). Transform and volume are
one undo step. Speed keeps the part of the file that the clip shows: speed 2
halves the clip's length and pulls the later clips in.

#### `freeze PROJECT CLIP --at TIME`

Holds the frame of a video clip. The CLI cuts the clip at `--at`, puts a still
of that frame between the two parts, and moves the later clips on the lane and
on its linked lanes to the right. `--duration` is the length of the still
(default 3 s). One undo step. The result gives the still as `still`.

```bash
chukcut-cli freeze reel.chukcut 0:0 --at 2.4 --duration 1.5
```

#### `speed-curve PROJECT CLIP`

Gives a clip a speed ramp, changes it, or removes it. Use one of:

| Option | |
|---|---|
| `--preset NAME` | `montage`, `hero`, `bullet`, `jump_cut`, `flash_in` or `flash_out` (`catalog speed_presets`) |
| `--point TIME=SPEED` | a point of your own; can repeat. TIME is the time in the clip's media from its in point, at normal speed. SPEED is from 0.1 to 10 |
| `--remove` | go back to the clip's constant speed |

The clip's length follows the curve. The later clips on its lanes move with
its end. A clip on a speed curve plays no sound (the engine has no
pitch-correct time stretch). One undo step. For a constant speed, use
`set --speed`.

```bash
chukcut-cli speed-curve reel.chukcut 0:2 --preset hero
chukcut-cli speed-curve reel.chukcut 0:2 --point 0=1 --point 1.2s=0.3 --point 2.5s=1
```

### Markers

A marker is a named time on the ruler. It does not belong to a clip. Name a
marker by its id, a unique prefix of 4 or more characters, or its index in
time order (`0` is the first marker).

#### `marker add PROJECT --at TIME`

`--label TEXT` and `--color` (`blue`, the default, `green`, `yellow`,
`orange`, `red` or `purple`).

#### `marker set PROJECT MARKER`

`--at TIME` moves the marker. `--label` changes the label; an empty label
removes it. `--color` changes the colour. One undo step.

#### `marker remove PROJECT MARKER...`

Removes one or more markers as one undo step.

#### `marker list PROJECT`

Lists the markers in time order: index, id, time, colour, label. It does not
change the project.

```bash
chukcut-cli marker add reel.chukcut --at 12.5 --label "Drop" --color red
chukcut-cli marker set reel.chukcut 0 --at 13
```

### Timelines and compound clips

A project can hold several timelines. One is open at a time, and every
other command (`info`, `split`, `append`, `export` …) works on the open one.
Name a timeline by its id, a unique id prefix of 4 or more characters, its
exact name, or its index in tab order (`0` is the first). A compound clip is
a clip that holds a timeline of its own. Opening one is an edit like any
other: the saved file remembers it, and later commands edit the compound
clip's lanes until `compound close`. `export` always renders the whole
timeline, even while a compound clip is open. Decision 0022.

#### `timeline list PROJECT`

Lists the timelines and the compound clips' sequences: index, id, name,
length, lanes, clips, how many compound clips use it, and which is open. With
`--json`, also the breadcrumb path into an open compound clip. It does not
change the project.

#### `timeline new PROJECT [--name NAME]`

Adds an empty timeline with a video and an audio lane, and opens it.

#### `timeline rename PROJECT TIMELINE NAME`

#### `timeline duplicate PROJECT TIMELINE`

Copies a timeline into a new one after the last. The copy's clips have new
ids, their own link groups, transitions and titles.

#### `timeline delete PROJECT TIMELINE`

The last timeline cannot be deleted. Deleting the open one opens its
neighbour.

#### `timeline switch PROJECT TIMELINE`

Opens another timeline, closing any open compound clip.

#### `compound create PROJECT CLIP... [--name NAME]`

Moves the clips into a new compound clip in their place, linked partners
included. The compound clip goes on the lowest video lane the clips used when
it fits there, otherwise on another video lane or a new one. Prints the new
clip's id.

#### `compound open PROJECT CLIP`

#### `compound close PROJECT [--all]`

Closes the open compound clip, or with `--all` every open level.

#### `compound flatten PROJECT CLIP`

Puts a compound clip's clips back on the timeline in its place, cut at its
edges. The compound clip must play at normal speed.

```bash
chukcut-cli compound create reel.chukcut 0:1 0:2 1:0 --name "Intro"
chukcut-cli compound open reel.chukcut 0:1
chukcut-cli split reel.chukcut --at 1.5
chukcut-cli compound close reel.chukcut
chukcut-cli timeline duplicate reel.chukcut 0
chukcut-cli timeline rename reel.chukcut 1 "Shorts cut"
```

The MCP tools are `timeline_list`, `timeline_new`, `timeline_rename`,
`timeline_delete`, `timeline_duplicate`, `timeline_switch`, `compound_create`,
`compound_open`, `compound_close` and `compound_flatten`.

### Look

#### `grade PROJECT CLIP`

| Option | |
|---|---|
| `--set NAME=VALUE` | one control, can repeat |
| `--lut FILE` | attach a `.cube` LUT; `none` removes it |
| `--lut-intensity N` | 0 to 1 |
| `--reset SECTION` | `basic`, `lut`, `hsl`, `curves`, `wheels` or `all`, before the `--set` values |

Control names: `brightness`, `contrast`, `saturation`, `temperature`,
`exposure`, `tint`, `highlights`, `shadows`, `whites`, `blacks`, `vibrance`,
`sharpen`, `clarity`, `vignette_amount`, `vignette_midpoint`,
`vignette_feather`, `grain`, `fade`, `lut_intensity`,
`hsl_hue:BAND`, `hsl_saturation:BAND`, `hsl_luminance:BAND` (bands: `red`,
`orange`, `yellow`, `green`, `aqua`, `blue`, `purple`, `magenta`), and
`wheel_x:WHEEL`, `wheel_y:WHEEL`, `wheel_luma:WHEEL` (wheels: `lift`, `gamma`,
`gain`, `offset`). `catalog grade` lists them with their resting values.
Values are in document units: exposure in stops, saturation 1 is no change.
All changes in one command are one undo step.

```bash
chukcut-cli grade reel.chukcut 0:0 --set exposure=0.3 --set saturation=1.15 \
  --set hsl_saturation:orange=-0.2 --lut ~/luts/film.cube --lut-intensity 0.6
```

#### `curve PROJECT CLIP --point X,Y...`

Sets one tone curve of the clip's grade. Each `--point` is an input level and
an output level, both from 0 to 1. Give two or more points. The engine joins
the points with a smooth curve that never overshoots. `--channel` is `master`
(the default, all colours), `red`, `green` or `blue`. `--reset` makes the
curve a straight line again. The other grade values stay. One undo step.

```bash
# A soft S-curve on the master channel.
chukcut-cli curve reel.chukcut 0:0 --point 0,0 --point 0.25,0.2 --point 0.75,0.8 --point 1,1
```

In JSON, `points` is a list of `[x, y]` pairs or `"x,y"` strings.

#### `crop PROJECT CLIP`

Crops the clip's picture. `--left`, `--top`, `--right` and `--bottom` are
fractions of the source frame: left and top from 0, right and bottom up
to 1. An edge that you do not give keeps its value. `--clear` removes the
crop. One undo step.

```bash
chukcut-cli crop reel.chukcut 0:0 --left 0.1 --right 0.9
```

#### `layout pip PROJECT CLIP`

Makes the clip a picture in picture: one third of the canvas wide, in a
corner with a margin, with a `frame` effect (round corners, a thin border, a
soft shadow). `--corner` is `top_left`, `top_right`, `bottom_left` or
`bottom_right` (the default). Put the clip on a lane above the main picture.
One undo step.

#### `layout split PROJECT CLIP...`

Puts clips into a split screen. Each clip fills one cell, in the order that
you give them. `--layout`:

| Layout | Clips |
|---|---|
| `two_rows` (default) | 2, one above the other (the 9:16 reaction layout) |
| `two_columns` | 2, side by side |
| `three_rows` | 3 |
| `three_columns` | 3 |
| `grid` | 4, two by two |

```bash
chukcut-cli layout split reel.chukcut 0:0 1:0 --layout two_rows
```

#### `effect add PROJECT KIND`

Adds an effect (`catalog effects`): `gaussian_blur`, `zoom_blur`, `glow`,
`light_sweep`, `shake`, `rgb_split`, `glitch`, `vhs`, `pixelate`, `mirror`,
`kaleidoscope`, `film_grain`, `halation`, `bloom`, `gate_weave`, `letterbox`,
`frame`.

- With `--clip CLIP`, the effect goes on the end of that clip's stack.
- Without `--clip`, it becomes an effect clip on an effect lane. It changes
  everything below it. `--at` (default 0), `--duration` (default 3 s) and
  `--track` place it.

`--set NAME=VALUE` sets parameters. The CLI checks each value against the
catalog: a number in its range, a colour, or a choice by name or index.

```bash
chukcut-cli effect add reel.chukcut glow --clip 0:1 --set intensity=0.8
chukcut-cli effect add reel.chukcut shake --at 5 --duration 0.4
```

#### `effect set PROJECT CLIP EFFECT`

`--set NAME=VALUE` changes parameters (one undo step). `--enabled false`
switches the effect off and keeps its values. `--at TIME` sets each parameter
at that time, which makes a keyframe.

Every changed effect gets a new id (the engine never changes a material in
place). The result gives the new `effect_id`. The index and the kind stay the
same.

#### `effect remove PROJECT CLIP EFFECT`

Removes the effect from the clip. To remove an effect clip, delete the clip.

#### `animate PROJECT CLIP --preset NAME`

Gives a clip an animation preset. `--slot in|out|combo` (default `in`).
`--preset none` clears the slot. `--duration`, `--easing`, `--strength`
change the timing and the size of the motion. `catalog animations` shows which
presets fit which slot.

- In and Out: `fade`, `slide_left`, `slide_right`, `slide_up`, `slide_down`,
  `zoom_in`, `zoom_out`, `pop`, `bounce`, `spin`, `blur`, `wipe_left`,
  `wipe_right`, `wipe_up`, `wipe_down`, `swing`, `shake`, `rise`, `flip`, `whip`
- Combo (a loop): `pulse`, `heartbeat`, `wobble`, `rock`, `float`, `jitter`,
  `rotate`, `flicker`
- Easings: `linear`, `ease_in`, `ease_out`, `ease_in_out`, `smooth`, `snap`,
  `back_in`, `back`, `back_in_out`, `elastic`, `bounce`

#### `animate-text PROJECT CLIP --preset NAME`

Animates a title by letter, word or line. Presets: `typewriter`, `fade`,
`fade_up`, `pop`, `slide_up`, `drop`, `zoom`, `spin`, or `none`. Options:
`--slot in|out`, `--unit letter|word|line`,
`--order forward|backward|centre|random`, `--duration`, `--overlap` (0 to 1),
`--easing`, `--strength`.

#### `zoom PROJECT CLIP`

A punch-in zoom: `--amount` (1.1 is 110 %), `--pivot-x`, `--pivot-y`,
`--duration` (0 is a hard punch), `--easing`. `--clear` removes it. `--auto`
alternates 100 % and the amount across the jump cuts next to the clip. This
hides cuts in a talking-head video.

#### `keyframe PROJECT CLIP --property NAME --at TIME`

Adds a keyframe, or changes the keyframe at that time. Properties:
`position_x`, `position_y`, `scale_x`, `scale_y`, `rotation`, `opacity`,
`volume`. `--value` is required, except with `--remove`. `--easing` is
`hold`, `linear`, `ease_in`, `ease_out` or `ease_in_out`. `--at` is a
timeline time. The engine keeps keyframes relative to the clip start, so they
move with the clip.

```bash
chukcut-cli keyframe reel.chukcut 2:0 --property opacity --at 0 --value 0
chukcut-cli keyframe reel.chukcut 2:0 --property opacity --at 0.5 --value 1 --easing ease_out
```

#### `title add PROJECT TEXT`

Puts a title on the title lane. `--at` (default 0) and `--duration` (default
3 s). When the time is not free, the title moves to the next gap.

Style options (also for `title set`): `--font`, `--size` (canvas pixels),
`--color`, `--bold true|false`, `--italic true|false`,
`--align left|center|right`, `--stroke-width`, `--stroke-color`,
`--background COLOUR|none`, `--shadow true|false`, `--x`, `--y`.

A title with style options is two undo steps: the title, then its style.

```bash
chukcut-cli title add reel.chukcut "Day 1" --at 0.5 --size 140 --color "#ffcc00" --y 0.6
```

#### `title set PROJECT CLIP`

`--text` changes the words. The style options change the look. All changes are
one undo step.

#### `title style PROJECT STYLE`

Adds a title in a style, or gives a title a new style. `catalog title_styles`
lists the styles.

- With `--clip CLIP`, the title gets the style. It keeps its words, its size
  and its position.
- Without `--clip`, the CLI adds a new title. `--at` (default 0) and
  `--track` place it, and `--text` gives its words (default: the style's
  sample). The title goes where the style puts it, for example low and left
  for a lower third.

One undo step.

#### `title template PROJECT TEMPLATE`

The same as `title style`, with a template from `catalog title_templates`. A
template is a style and an animation. With `--clip`, the title gets the
style and the animation of the template.

```bash
chukcut-cli title template reel.chukcut neon-sign --at 1 --text "Day 1"
```

#### `title position PROJECT CLIP --position CELL`

Moves a title to a cell of a 3 x 3 grid and aligns its text to match. Cells:
`top_left`, `top`, `top_right`, `left`, `centre`, `right`, `bottom_left`,
`bottom`, `bottom_right`. One undo step.

#### `title duplicate PROJECT CLIP`

Copies a title: its words, style, transform, keyframes and animation. The copy
gets its own text material, so a change to one title does not change the
other. The copy starts at the end of the original, on the same lane. `--at`
gives another start. When the time is not free, the copy moves to the next
gap. One undo step.

#### `transition add PROJECT CLIP`

Puts a transition on the cut at the start of the clip (the incoming clip).
`--kind`: `dissolve` (default), `dip_to_color`, `wipe`, `slide`, `zoom`,
`blur`, or a library preset from `catalog transitions` (`gl:crosswarp`, or
only `crosswarp` when the name is unique). `--duration` is shortened to what
the two clips allow. A new transition replaces the old one.

#### `transition remove PROJECT CLIP`

#### `track PROJECT CLIP --at TIME --rect CX,CY,W,H`

Tracks a region of a video clip through its frames. `--rect` is the box in the
clip's own frame, as fractions: centre x, centre y, width, height. `--at` is
the timeline time where you draw the box. `--direction forward|backward|both`
(default `both`).

`--overlay CLIP` makes a title, sticker or other clip follow the track.
`--mode position|position_scale|position_scale_rotation` sets how it follows.
The command waits for the analysis to finish. The result is one undo step.

```bash
chukcut-cli track reel.chukcut 0:0 --at 1.2 --rect 0.52,0.4,0.15,0.2 --overlay 2:0
```

#### `mask PROJECT CLIP`

Adds, changes or removes shape masks on a clip. Each change is one undo step.

- `--add SHAPE` adds a mask: `linear`, `mirror`, `ellipse` (or `circle`),
  `rectangle`, `star` or `heart`. The other options then apply to the new mask.
- `--mask REF` names the mask to change: its index (0 is the first) or its id.
  Without it, the command uses the mask it just added, or the only mask.
- `--set NAME=VALUE` sets a value. `x` and `y` are the centre, as a fraction
  of the clip from its middle (0.5 is the right or top edge, +y is up).
  `width`, `height` and `feather` are fractions of the clip's shorter side.
  `rotation` is degrees, clockwise. `roundness` (0..1) rounds the corners of a
  rectangle.
- `--at TIME` sets the values as keyframes at that timeline time.
- `--op add|subtract|intersect` sets how the mask combines with the masks
  before it. `--invert true|false`, `--enabled true|false`, `--shape SHAPE`.
- `--remove` removes the mask. `--clear` removes all masks first.

The result has `compositing.masks`, with each mask's index and id.

```bash
chukcut-cli mask reel.chukcut 1:0 --add circle --set width=0.8 --set feather=0.2
chukcut-cli mask reel.chukcut 1:0 --add rectangle --op subtract --set width=0.1
chukcut-cli mask reel.chukcut 1:0 --mask 0 --set x=-0.3 --at 0 --set x=0.3 --at 2
```

#### `chroma-key PROJECT CLIP`

Keys a colour out of a clip (green screen). `--color #rrggbb` gives the
colour. `--pick X,Y` takes it from the footage under a point of the canvas
(0..1 from the top left) at `--at TIME` (default: the start of the clip).
`--tolerance`, `--softness`, `--spill` and `--shrink` are 0..1.
`--enabled false` switches the key off and keeps its values. `--off` removes
the key.

```bash
chukcut-cli chroma-key reel.chukcut 1:0 --pick 0.05,0.5 --spill 0.7
```

#### `blend PROJECT CLIP [MODE] [--opacity N]`

Sets how a clip blends with the lanes below it: `normal`, `multiply`,
`screen`, `overlay`, `soft_light`, `hard_light`, `darken`, `lighten`,
`color_dodge`, `color_burn`, `difference`, `exclusion`, `add` or `subtract`.
`--opacity` is 0..1.

### Analysis

The analysis commands look at the pictures or the sound of a clip. The
command waits until the analysis is done and shows progress on stderr. Then
the result goes into the project as one undo step.

#### `scenes detect PROJECT CLIP`

Finds the shot changes in a video clip and marks them on the clip.
`--sensitivity` from 0 to 1 (default 0.5: hard cuts, not fast motion).
`--split` also cuts the clip at each change, in the same undo step. The result
lists the changes in timeline seconds.

#### `scenes split PROJECT CLIP`

Cuts the clip at the scene changes that `scenes detect` found. One undo step.

#### `scenes clear PROJECT CLIP`

Removes the scene marks from the clip.

#### `stabilise apply PROJECT CLIP`

Measures the camera shake of a video clip and stabilises it. `--strength` from
0 to 1 (light 0.35, medium 0.6 (the default), strong 0.85, tripod 1).
`--crop` is how much of the picture is cut off to hide the moving edges:
`auto` (the default, the least that hides them) or a fraction up to 0.3. The
engine keeps the measured camera path, so a second run on the same clip is
fast.

#### `stabilise set PROJECT CLIP`

Changes a stabilised clip: `--enabled true|false`, `--strength`, `--crop`.
The clip must have a stabilisation from `stabilise apply`.

#### `stabilise remove PROJECT CLIP`

#### `beats detect PROJECT CLIP`

Finds the beats in the sound of a clip: a music clip, or the linked sound of a
video clip. The marks go on the clip that plays the sound. The result gives
the tempo (`bpm`) and the beat times.

#### `beats clear PROJECT CLIP`

#### `beats cut PROJECT [CLIP...]`

Cuts video clips on the beats that `beats detect` found. `--every N` keeps
every N-th beat (default 1). Without clips, it cuts every clip on the video
lanes. One undo step.

#### `beats snap PROJECT [CLIP...]`

Moves each cut between the clips to the nearest beat. It rolls the cut: the
clip before the cut gets longer by the time that the clip after it loses, so
nothing else moves. `--tolerance TIME` is the farthest a cut can move
(default: half a beat at the slowest tempo, or 250 ms). Without clips, it
uses every clip on the video lanes. One undo step.

```bash
chukcut-cli beats detect reel.chukcut 1:0
chukcut-cli beats cut reel.chukcut 0:0 --every 2
```

#### `reframe PROJECT [CLIP...]`

Finds the subject of each clip and moves a window of the canvas's shape with
it, as position keyframes. `--ratio W:H` (for example `9:16`, `1:1`, `4:5`)
also changes the project to that shape, in the same undo step. Without clips,
the CLI uses each clip that fills its frame on a visible video lane.

```bash
chukcut-cli reframe reel.chukcut --ratio 9:16
```

#### `analysis PROJECT CLIP`

Shows what a clip has from the commands above: the scene changes and the
beats in timeline seconds, the tempo, and the stabilisation. It does not
change the project.

### Cloud

These commands use the cloud accounts that you set up in the app (Settings,
Accounts). `catalog accounts` lists them. The CLI cannot add an account or a
key. Each command takes `--account ID`. Without it, the CLI uses the first
account that can do the job. When there is no such account, the command
fails with exit code 1 and changes nothing.

#### `cloud translate PROJECT --to LANG`

Translates the first caption lane into the language `LANG` (ISO 639-1, for
example `de`). The translation goes on a new caption lane, a little above the
original. `--from LANG` gives the captions' language (default: detected).
`--model` is the chat model for an OpenAI-compatible account. Needs a DeepL
or an OpenAI-compatible account. One undo step.

#### `cloud tts PROJECT TEXT --voice VOICE`

Speaks the text with a cloud voice (ElevenLabs or OpenAI-compatible) and
imports the sound file into the project. `--at TIME` also puts it on an audio
lane at that time. `--model`, `--speed` (1 is normal) and `--instructions`
(OpenAI: tone, accent, emotion in plain words) change the voice. The file goes
to `~/.local/share/chukcut/generated`, with a record of its source and
licence.

```bash
chukcut-cli cloud tts reel.chukcut "Three tips for better sleep" --voice nova --at 0
```

#### `cloud stock-kinds PROJECT`

Lists the kinds of stock (`video`, `photo`, `sound`) that the account's
library has.

#### `cloud stock-search PROJECT QUERY`

Searches a stock library (Pexels, Pixabay or Freesound). `--kind
video|photo|sound` (default `video`), `--page` (from 1), `--per-page`
(default 24), `--non-commercial` (also show sounds that do not allow
commercial use). The result lists each hit's id, title, creator, size,
licence and credit line. The CLI keeps search results for one day. It does not
change the project.

#### `cloud stock-download PROJECT QUERY --id ID`

Downloads one result of a search and imports it into the project. Give the
same query, `--kind` and `--page` as the search; the CLI finds the result by
its `--id`. `--at TIME` also puts the file on the timeline at that time.

```bash
chukcut-cli cloud stock-search reel.chukcut "ocean waves" --kind video --json | jq '.data.hits[].id'
chukcut-cli cloud stock-download reel.chukcut "ocean waves" --kind video --id 1234567 --at 4
```

### Captions

#### `captions transcribe PROJECT`

Transcribes the timeline's speech and puts the captions on the caption lane,
with word times for the karaoke highlight. It uses the transcriber that the
app's caption panel uses, unless you give options:

| Option | |
|---|---|
| `--backend cloud\|local` | an OpenAI-compatible account, or whisper.cpp on this machine |
| `--account ID` | the cloud account (add it in the app: Settings, Accounts) |
| `--model NAME` | the cloud model |
| `--local-model` | `tiny`, `base`, `small`, `medium` or `large_v3_turbo`; downloaded on first use |
| `--language CODE` | ISO 639-1; detected when not given |
| `--words N` | word captions, at most N words on screen; without it, sentence captions |
| `--keep` | keep the captions that are there (default: replace them) |
| `--emoji` | add one emoji to each caption |
| `--preset` | `classic`, `karaoke`, `yellow`, `boxed` or `big` |

```bash
chukcut-cli captions transcribe reel.chukcut --backend local --words 2 --preset karaoke
```

#### `captions import PROJECT FILE`

Reads an `.srt` or `.vtt` file. Same `--keep`, `--emoji`, `--preset`.

#### `captions export PROJECT FILE [--format srt|vtt]`

Writes the captions. The format comes from the file extension when you do not
give it.

#### `captions style PROJECT`

Changes every caption, or one with `--clip`. `--preset` first, then
`--font`, `--size`, `--color`, `--highlight COLOUR|none` (karaoke word
colour), `--bold`, `--italic`, `--align`, `--stroke-width`, `--stroke-color`,
`--background COLOUR|none`, `--position top|middle|bottom|Y`.

#### `captions list PROJECT`

### Sound

#### `silence detect PROJECT CLIP`

Finds the pauses in a clip's sound. It does not change the project. The result
lists each cut in timeline time and in source time, and the total time that
`silence remove` would take out.

| Option | Default | |
|---|---|---|
| `--threshold DB` | from the recording | quieter than this is a pause (dBFS) |
| `--min-silence TIME` | 0.5 s | shorter pauses stay |
| `--padding TIME` | 0.12 s | kept on the speech side of each cut |
| `--voice` | | also cut parts without a voice (breaths, room tone) |

#### `silence remove PROJECT CLIP`

Cuts the pauses from the clip and its linked sound, and closes the gaps. One
undo step. The other lanes (captions, music, titles) move with the cuts, so
captions stay on their words. `--no-sync` stops this. `--fillers` also cuts
filler words ("um", "uh") that it finds in the clip's captions; transcribe
first. `--language` (default `en`) selects the filler word list.

```bash
chukcut-cli silence remove reel.chukcut 0:0 --threshold -38 --min-silence 0.35
```

#### `normalize PROJECT`

Brings speech to a loudness. `--target LUFS` (default -14, from -40 to -5).
`--clip` for one clip; without it, every clip that makes sound. The gain is
capped so that peaks do not clip. `--off` removes the normalisation. For the
whole mix, use `export --loudness`.

#### `denoise PROJECT CLIP`

Reduces background noise (RNNoise). `--strength` from 0 to 1 (default 1).
`--off` turns it off. The first run renders a clean copy of the sound into the
cache.

#### `loudness PROJECT [--clip CLIP]`

Measures EBU R128 loudness: integrated LUFS, loudness range and true peak. For
one clip, or for the whole mix as the export would make it.

#### `audio-effect add PROJECT CLIP KIND [--set name=value]…`

Adds an audio effect to the clip's sound (a video clip's linked audio clip
when it has one): `eq3` (low, mid, mid_freq, high), `eq5` (five bands of
frequency, gain and width), `compressor`, `reverb`, `delay` (an echo),
`pitch` (semitones, keeping or moving the formants), or a voice preset.
`catalog audio` lists every parameter with its range and default. Effects run
in stack order; the clip's speed change comes first.

#### `audio-effect set PROJECT CLIP EFFECT [--set name=value]… [--enabled BOOL] [--index N]`

Changes an effect, named by its index in the clip's audio stack, its kind or
its id: parameters, on or off, or its position in the stack.

#### `audio-effect remove PROJECT CLIP EFFECT` · `audio-effect list PROJECT CLIP`

Takes an effect off; lists the stack with every parameter's value, the pitch
switch and any ducking.

#### `voice PROJECT CLIP PRESET [--intensity 0..1]` · `voice PROJECT CLIP --off`

The voice changer: `deep`, `chipmunk`, `robot`, `telephone` or `megaphone`.
A clip has one voice at a time; a new one replaces the old one in place.

#### `audio-pitch PROJECT CLIP [--follow-speed]`

"Change audio pitch". Every speed change keeps the pitch by default (a
time stretch). With `--follow-speed` the pitch moves with the speed, like a
tape; without it, it is kept again. Speed curves follow the same switch.

#### `duck PROJECT CLIP [--depth dB] [--attack TIME] [--release TIME] [--threshold dBFS]`

Turns a music clip down wherever someone speaks on another lane (default
12 dB, 250ms attack ending where speech starts, 500ms release). Speech is
found by level and by RNNoise's voice detector, so a sound effect does not
duck the music. The result is volume keyframes, multiplied into any fades the
clip has, in one undo step. Ducking again starts from the clip's own volume;
`--off` gives it back.

#### `record PROJECT --duration TIME [--at TIME] [--count-in SECONDS]`

Records a voiceover from the default input device for `--duration` (after an
optional count-in, which is not kept) and puts it on a new audio lane at
`--at`. The file is saved next to the project in `<project name> Media/`.

### Render

#### `export PROJECT OUTPUT`

Renders the timeline to a file. The command waits until the file is
complete and shows progress on stderr.

| Option | |
|---|---|
| `--preset ID` | a preset from `presets` (see below), or `custom` (the project's canvas and frame rate; the default) |
| `--hardware ID` | a hardware encoder from `catalog hardware` (`nvenc_h264`, `vaapi_h265`, …), `auto` for the first one that works, or `software` |
| `--width`, `--height`, `--fps` | override the preset |
| `--codec` | `h264`, `h265`, `vp9`, `av1`, `prores`, `gif` |
| `--crf N` or `--bitrate BPS` | quality |
| `--container` | `mp4`, `mov`, `mkv`, `webm`, `gif`; sound only: `m4a`, `mp3`, `wav`. The file extension follows the container |
| `--audio-codec` | `aac`, `opus`, `mp3`, `pcm`, `none` |
| `--audio-bitrate BPS` | |
| `--no-audio` | |
| `--loudness LUFS` | bring the mix to this loudness, true peak at -1 dBTP; overrides the preset's target |
| `--no-loudness` | keep the mix as edited, also when the preset has a target |
| `--from TIME`, `--to TIME` | export only this range |
| `--sidecar srt\|vtt` | also write the captions next to the video |

The built-in presets:

| Preset | Size | Rate | Picture | Sound | Loudness |
|---|---|---|---|---|---|
| `tiktok` | 1080p, canvas shape | 30 | H.264 CRF 20 | AAC 192k | -14 LUFS |
| `instagram_reels` | 1080p, canvas shape | 30 | H.264 CRF 21 | AAC 192k | -14 LUFS |
| `youtube_shorts` | 1080p, canvas shape | 30 | H.264 CRF 20 | AAC 256k | -14 LUFS |
| `instagram_square` | 1080×1080 | 30 | H.264 CRF 21 | AAC 192k | -14 LUFS |
| `x_twitter` | 1080p, canvas shape | 30 | H.264 8 Mbit/s | AAC 128k | -14 LUFS |
| `youtube_1080p` | 1080p, canvas shape | project | H.264 CRF 20 | AAC 384k | -14 LUFS |
| `youtube_4k` | 2160p, canvas shape | project | H.265 CRF 22 | AAC 384k | -14 LUFS |
| `master_prores` | canvas | project | ProRes 422 HQ 10-bit, .mov | PCM 16-bit | as edited |
| `master_h264` | canvas | project | H.264 CRF 14 | AAC 320k | as edited |
| `master_hevc` | canvas | project | H.265 CRF 16 | AAC 320k | as edited |
| `audio_aac` | — | — | — | AAC 256k .m4a | -16 LUFS |
| `audio_mp3` | — | — | — | MP3 320k | -16 LUFS |
| `audio_wav` | — | — | — | PCM 16-bit .wav | as edited |
| `gif` | 480p, canvas shape | 15 | GIF, 252 colours | — | — |

"1080p, canvas shape" means: the shorter side is 1080 and the aspect ratio
is the canvas's. A 9:16 canvas gives 1080×1920, a 16:9 canvas 1920×1080. The
composition is never boxed into another shape. When the canvas does not
have the shape that the platform shows full screen, or the timeline is
longer than the platform accepts, the result has a `warnings` list. The old
id `vertical_1080x1920` still works and means `tiktok`.

The software encoder is the default. A hardware encoder is a choice, because
the engine trial-encodes each one before it uses it. ProRes, GIF and sound
only always use the software encoder.

The result gives the path, the size in bytes, the frame count, the time it
took, the encode speed, the output size, the loudness target and the
warnings.

```bash
chukcut-cli export reel.chukcut out/reel.mp4 --preset tiktok \
  --hardware auto --sidecar srt --json
chukcut-cli export reel.chukcut out/voice.mp3 --preset audio_mp3
```

#### `presets PROJECT`

Lists every preset as it fits this project: the size and frame rate it
exports at on this canvas, the container, the loudness target, the warnings
and an instant size estimate. Your own presets come last, with `user: true`.

#### `preset save PROJECT NAME [export options]`

Saves export settings as a preset of your own. Start from a preset and add
overrides; every export option except the output is allowed. The preset
keeps the resolution class, so a "1080p" preset also exports 1080p on a
canvas of another shape. The id is `user_` and the name in lower case.
Saving the same name again replaces the preset. Presets are stored in
`<config>/chukcut/export-presets.json`; the app and the CLI share them.

```bash
chukcut-cli preset save reel.chukcut "Reel HQ" --preset instagram_reels --crf 18
chukcut-cli export reel.chukcut out/reel.mp4 --preset user_reel_hq
```

#### `preset remove PROJECT ID`

Deletes one of your own presets. A built-in preset cannot be deleted.

#### `estimate PROJECT [export options] [--sample]`

Tells what an export will produce and how big it will be, without writing
it. The size of a CRF export depends on the picture. Without `--sample`,
the size of a CRF export is a rough guess from a table
(`estimate.method: "table"`). With `--sample`, the engine encodes three
two-second windows of the timeline (the whole timeline when it is 8 s or
shorter) with the real settings and measures the size
(`method: "sampled"`). A bitrate export and a sound-only export are
calculated, not sampled (`method: "bitrate"`, `"exact"` for WAV). The
estimate includes the audio and the container overhead. In the tests the
sampled and calculated estimates are within 2 % of the real files.

#### `export-queue PROJECT [--presets ID,ID…] [--out-dir DIR] [--jobs FILE]`

Queues several exports and runs them one after another. The command waits
until all files are written. Progress shows the item and its percentage.

- `--presets ID,ID,…` (a list, or the flag repeated) exports this project
  once per preset, to
  `<out-dir>/<project>-<preset>.<ext>`. The other export options (for
  example `--from`/`--to`, `--hardware`) apply to each of these. Without
  `--out-dir`, the files go next to the project. `--preset ID` alone queues
  one export.
- `--jobs FILE` (or `-` for stdin) is a JSON list of jobs. A job is an
  object with `output` and the export options (`preset`, `from`, `to`,
  `crf`, `container`, …). A job can name another project file in `project`
  and a name for the progress lines in `label`.

Every job is checked before the first one starts, so a wrong preset name in
the third job stops the command at once. When an export fails, the others
still run; then the command fails with exit code 5 and names the failed
jobs.

```bash
chukcut-cli export-queue reel.chukcut --presets tiktok,youtube_shorts,audio_mp3 \
  --out-dir out/
echo '[{"output":"out/intro.mp4","preset":"x_twitter","from":0,"to":10},
      {"output":"out/other.mp4","project":"other.chukcut","preset":"youtube_1080p"}]' \
  | chukcut-cli export-queue reel.chukcut --jobs -
```

In the app, the export dialog has the same queue: "Add to queue" puts the
export in the queue and the queue keeps running when the dialog is closed.

#### `render-frame PROJECT --at TIME OUTPUT`

Writes the frame at a time as a PNG at the full canvas size. It uses the export
compositor, so it shows the same pixels as the export.

### Undo and redo

#### `undo PROJECT`, `redo PROJECT`

These are operations for a `batch` and for the MCP server. A one-shot command
starts with an empty history, so `undo` has nothing to undo there. The undo
history is not stored in the project file (the same as in the app).

## Batch files

`batch PROJECT FILE` runs a list of operations against one open project. Use
`-` for stdin. The operations share one undo history, so `undo` and `redo` work
between them. The batch is all or nothing: the CLI saves once, at the end, and
only when every operation succeeded. When operation 3 fails, the error says
`operation 3 (name): …` and the file does not change.

The file is a JSON list, or an object with an `ops` list. Each operation is an
object with `op` and the same arguments as the command, in snake_case. Lists
like `--set` can be an object:

```json
[
  {"op": "new", "width": 1080, "height": 1920},
  {"op": "import", "files": ["take1.mp4", "music.mp3"], "append": true},
  {"op": "silence_remove", "clip": "0:0", "min_silence": "350ms"},
  {"op": "split", "at": "4.5s"},
  {"op": "grade", "clip": "0:1", "set": {"exposure": 0.3, "saturation": 1.1}},
  {"op": "title_add", "text": "Day 1", "at": 0.5, "color": "#ffcc00"},
  {"op": "undo"},
  {"op": "redo"},
  {"op": "transition_add", "clip": "0:1", "kind": "dissolve", "duration": 0.4},
  {"op": "export", "output": "out/reel.mp4", "hardware": "auto"}
]
```

When the first operation is `new`, the batch makes the project.

The operation names are the MCP tool names: `info`, `validate`, `configure`,
`import`, `undo`, `redo`, `append`, `place`, `split`, `delete`, `move`,
`trim`, `clip_set`, `grade`, `effect_add`, `effect_set`, `effect_remove`,
`animate`, `animate_text`, `zoom`, `keyframe`, `title_add`, `title_set`,
`transition_add`, `transition_remove`, `track`, `mask`, `chroma_key`,
`blend`, `captions_transcribe`,
`captions_import`, `captions_export`, `captions_style`, `captions_list`,
`silence_detect`, `silence_remove`, `normalize`, `denoise`, `loudness`,
`marker_add`, `marker_set`, `marker_remove`, `marker_list`, `crop`, `curve`,
`freeze`, `speed_curve`, `layout_pip`, `layout_split`, `title_style`,
`title_template`, `title_position`, `title_duplicate`, `scenes_detect`,
`scenes_split`, `scenes_clear`, `stabilise`, `stabilise_set`,
`stabilise_remove`, `beats_detect`, `beats_clear`, `beats_cut`, `beats_snap`,
`reframe`, `analysis`, `translate_captions`, `tts`, `stock_kinds`,
`stock_search`, `stock_download`, `audio_effect_add`, `audio_effect_set`,
`audio_effect_remove`, `audio_effects`, `voice`, `audio_pitch`, `duck`,
`record`, `export`, `presets`, `preset_save`,
`preset_remove`, `estimate`, `export_queue`, `render_frame`. The arguments
are the command's options and positional arguments without the project.
In `export_queue`, the jobs are a `jobs` list rather than a file. `chukcut-cli mcp` lists each one's
JSON Schema (see below).

## The MCP server

`chukcut-cli mcp` is an MCP server on stdio (newline-delimited JSON-RPC 2.0).
It supports the protocol versions 2025-11-25, 2025-06-18, 2025-03-26 and
2024-11-05. When a client asks for a different version, the server answers
with 2025-11-25.

### Add it to Claude Code

```bash
claude mcp add chukcut -- chukcut-cli mcp
```

Give the full path when `chukcut-cli` is not on your `PATH`:

```bash
claude mcp add chukcut -- /home/you/.local/bin/chukcut-cli mcp
```

`--scope user` makes it available in all your projects. Other MCP clients use
the same command: the program `chukcut-cli` with the argument `mcp`.

### Tools

Each operation above is a tool with the same name and a JSON Schema made from
the same argument struct that the CLI parses. A tool schema cannot be
different from what the tool accepts. Each tool (except `catalog`) also takes
`project`, the absolute path of the `.chukcut` file.

The server adds four tools:

| Tool | |
|---|---|
| `new_project` | makes a project file (`width`, `height`, `fps`, `name`, `force`) |
| `view_frame` | renders the frame at `at` and returns it as a PNG image in the result; writes no file |
| `batch` | runs `ops` (a list as in a batch file) all or nothing |
| `catalog` | lists effects, transitions and so on (`kind`); needs no project |

Tool results have a text block with `{"message": …, "data": …}`. For clients
on 2025-06-18 or later, the same object is also in `structuredContent`. A
refused edit is a tool result with `isError: true` and the engine's message;
an unknown tool or a malformed request is a JSON-RPC error.

Read-only tools have `readOnlyHint`: `info`, `validate`, `captions_list`,
`silence_detect`, `loudness`, `catalog`, `view_frame`, `marker_list`,
`analysis`, `stock_kinds`, `stock_search`, `presets`, `estimate`.

Tools that send data to a service outside this machine have
`openWorldHint`: `captions_transcribe`, `translate_captions`, `tts`,
`stock_kinds`, `stock_search`, `stock_download`.

### Resources

| URI | |
|---|---|
| `chukcut://project?path=/abs/path.chukcut` | the `info` summary, JSON |
| `chukcut://frame?path=/abs/path.chukcut&at=2.5` | the frame at `at`, PNG |

`resources/list` lists the projects that the connection opened.
`resources/templates/list` gives the two templates. Percent-encode the path.

### Sessions, saving and undo

- The server keeps each project open for the life of the connection. `undo`
  and `redo` work across tool calls.
- Each successful edit is saved to the file at once. When an agent stops in
  the middle of a task, the project is consistent.
- When an edit fails, the server discards what it changed in memory. The file
  keeps its last good state.
- When the file changes on disk (for example, you edit it in the app), the
  server reads it again before the next call. The undo history then starts
  again.
- The server does one request at a time. A long export blocks the connection
  until it is done. When the request has a `progressToken`, the server sends
  `notifications/progress` during the export, the transcription, the tracking,
  the sound analysis, the picture analysis (scenes, stabilise, reframe), the
  beat detection and a stock download.

### Example session

What a client sends, one line each:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"me","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"new_project","arguments":{"project":"/home/me/reel.chukcut"}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"import","arguments":{"project":"/home/me/reel.chukcut","files":["/home/me/take1.mp4"],"append":true}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"split","arguments":{"project":"/home/me/reel.chukcut","at":2}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"view_frame","arguments":{"project":"/home/me/reel.chukcut","at":2.5}}}
```

## Recipes

### A talking-head reel in one script

```bash
#!/usr/bin/env bash
set -euo pipefail
p=reel.chukcut
chukcut-cli new "$p" --force
chukcut-cli import "$p" take.mp4 --append
chukcut-cli denoise "$p" 0:0
chukcut-cli captions transcribe "$p" --backend local --words 2 --preset karaoke
chukcut-cli silence remove "$p" 0:0 --fillers
chukcut-cli zoom "$p" 0:0 --auto --amount 1.12
chukcut-cli normalize "$p" --target -14
chukcut-cli export "$p" reel.mp4 --preset vertical_1080x1920 --hardware auto --sidecar srt
```

Transcribe before `silence remove --fillers`: filler words come from the
captions. `silence remove` keeps the captions on their words.

### Keep the ids

```bash
id=$(chukcut-cli --json split reel.chukcut --at 4 --clip 0:0 | jq -r '.data.created[0].id')
chukcut-cli grade reel.chukcut "$id" --set exposure=-0.2
```

### Check an export

```bash
chukcut-cli --json export reel.chukcut out.mp4 | jq '.data | {path, frames, bytes}'
ffprobe -v error -count_frames -show_entries stream=nb_read_frames out.mp4
```

## Limits

- **Export and render need a GPU.** The compositor runs on Vulkan. On a machine
  without a GPU, install Mesa's lavapipe (software Vulkan). Without any
  Vulkan driver, `export`, `render-frame` and `view_frame` fail with exit
  code 5.
- **No cancel.** Ctrl+C stops the process; an export that stops early leaves
  a partial file. The same is true for `export-queue`: the queue lives in
  the process, so the exports not yet run are not run.
- **Undo history lives only in one session** (one command, one batch, one MCP
  connection). The project file does not store it.
- **Cloud features need an account** that you set up in the app. The CLI
  cannot add accounts or keys. Without an account, a cloud command fails with
  exit code 1.
- **The stock commands take a project** (as every operation does), but they
  do not change it, except `cloud stock-download`.

## For developers

- `crates/cli/src/ops/` holds one argument struct per operation. The struct
  is the CLI options (clap), the batch and tool arguments (serde) and the MCP
  schema (schemars). To add an operation, write the struct and its `run`, then
  add it to the `operations!` list in `ops/mod.rs` and to the `Command` enum in
  `main.rs`.
- An operation builds its edit with the engine's command functions
  (`modules/*/commands.rs`) and the gesture builders in
  `modules/timeline/gesture.rs`. It does not change a `Project` directly.
- Tests: `cargo test -p chukcut-cli`. `tests/flow.rs` runs the binary on
  generated media and checks the export with ffprobe. `tests/mcp.rs` drives a
  full MCP session over a pipe. `tests/coverage.rs` checks the later
  operations (markers to cloud) in the saved project file. Its cloud test
  uses an OpenAI-compatible stand-in server on 127.0.0.1, so no request leaves
  the machine.
