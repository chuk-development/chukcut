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
| `transitions` | the built-in kinds and the library presets |
| `animations` | clip presets (which slot each one fits), text presets, easings |
| `grade` | every grade control name and its resting value |
| `presets` | export presets |
| `hardware` | hardware encoders, and which ones work on this machine |
| `models` | local transcription models, and which ones are downloaded |
| `luts` | `.cube` files in the LUT library |
| `fonts` | font families that the text renderer can draw |

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

### Render

#### `export PROJECT OUTPUT`

Renders the timeline to a video file. The command waits until the file is
complete and shows progress on stderr.

| Option | |
|---|---|
| `--preset` | `youtube_1080p`, `youtube_4k`, `vertical_1080x1920`, `instagram_square`, or `custom` (the project's canvas and frame rate; the default) |
| `--hardware ID` | a hardware encoder from `catalog hardware` (`nvenc_h264`, `vaapi_h265`, …), `auto` for the first one that works, or `software` |
| `--width`, `--height`, `--fps` | override the preset |
| `--codec` | `h264`, `h265`, `vp9`, `av1` |
| `--crf N` or `--bitrate BPS` | quality |
| `--container` | `mp4`, `mov`, `mkv`, `webm`; the file extension follows the container |
| `--audio-codec` | `aac`, `opus`, `none` |
| `--no-audio` | |
| `--loudness LUFS` | bring the mix to this loudness, true peak at -1 dBTP |
| `--from TIME`, `--to TIME` | export only this range |
| `--sidecar srt\|vtt` | also write the captions next to the video |

The software encoder is the default. A hardware encoder is a choice, because
the engine trial-encodes each one before it uses it.

The result gives the path, the size in bytes, the frame count, the time it
took and the encode speed.

```bash
chukcut-cli export reel.chukcut out/reel.mp4 --preset vertical_1080x1920 \
  --hardware auto --loudness -14 --sidecar srt --json
```

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
`transition_add`, `transition_remove`, `track`, `captions_transcribe`,
`captions_import`, `captions_export`, `captions_style`, `captions_list`,
`silence_detect`, `silence_remove`, `normalize`, `denoise`, `loudness`,
`export`, `render_frame`. The arguments are the command's options and
positional arguments without the project. `chukcut-cli mcp` lists each one's
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
`silence_detect`, `loudness`, `catalog`, `view_frame`.

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
  `notifications/progress` during the export, the transcription, the tracking
  and the sound analysis.

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
  a partial file.
- **Undo history lives only in one session** (one command, one batch, one MCP
  connection). The project file does not store it.
- **Cloud features need an account** that you set up in the app. The CLI
  cannot add accounts or keys.
- **Not covered yet:** markers, crop, curves (point lists), picture in
  picture and split-screen layouts, freeze frame, caption translation,
  text-to-speech, stock media search. The engine has commands for them; each
  is a small operation struct in `crates/cli/src/ops/`.

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
  full MCP session over a pipe.
