# Export

Click **Export** at the top right, or use **Menu › Export…** (`Ctrl+E`).
The export renders the timeline that is open, also when a compound clip is
open. The export uses the same shaders as the player, so it shows what the
player shows.

## The dialog

The left side shows the cover: the frame at the playhead.

- **Preset**: a list of ready settings.

  | Group | Presets |
  |---|---|
  | Social | TikTok, Instagram Reels, YouTube Shorts, Instagram square, X / Twitter |
  | YouTube | YouTube 1080p, YouTube 4K |
  | Master | Master · ProRes 422 HQ, Master · H.264, Master · HEVC |
  | Audio only | Audio · AAC, Audio · MP3, Audio · WAV |
  | GIF | GIF |
  | Custom | Custom |
  | My presets | the presets that you saved |

  The social presets keep the canvas's shape and set a loudness target of
  −14 LUFS. **+** saves the current settings as a preset. A bin deletes one
  of your presets.
- **Name** and **Export to** (the folder). The dialog warns when a file will
  be replaced.
- **Export as**: **Video**, **Audio only** or **GIF**.
- **Range**: **Whole timeline** or **In to out**. It shows only when you set
  in and out marks.

### Video

- **Resolution**: 480p, 720p, 1080p, 2K, 4K.
- **Bitrate**: **Lower**, **Recommended**, **Higher** or **Custom** (in
  Mbit/s).
- **Codec**: **H.264**, **HEVC**, **AV1** or **ProRes**.
- **Format**: mp4 or mov.
- **Frame rate**: 23.976 to 60 fps.
- **Encoder**: shows which encoder will run, for example "GPU · H.264
  (NVIDIA NVENC)" or "Software · libx264". You cannot choose it here.
  chukcut uses a GPU encoder when one passed its test encode. Settings ›
  **Hardware** shows why an encoder was refused.
- **Bit depth** (HEVC and AV1 only): **8-bit** or **10-bit**. 10-bit gives
  smoother gradients in skies and dark scenes. Some old phones and TVs cannot
  play 10-bit HEVC.
- **Colour space** shows what the file gets: "Rec. 709 SDR" for 720p and
  larger, "Rec. 601 SDR" for 480p. The file is tagged with it, so every player
  shows the colours you saw in the player here.

### GIF

**Resolution**, **Frame rate** (10 to 25 fps) and the size. A GIF has 252
colours and no sound.

### Audio

- **Format**: **AAC (.m4a)**, **MP3** or **WAV**.
- **Bitrate**: 128 to 320 kbps.
- **Loudness**: **Off · keep the mix as edited**, **−14 LUFS · social**,
  **−16 LUFS · podcast** or **−23 LUFS · broadcast**. A limiter keeps the
  peaks safe.
- **Mix now** with **Measure** shows the loudness of the mix as it is.

### The bottom line

**Duration** and **Size: about X MB**. chukcut measures the size on a few
sample frames, so the estimate is close.

## Export or queue

- **Export** starts at once. **Cancel export** stops it. When it is done:
  **Show in folder**, **Play**, **Close**.
- **Add to queue** adds the export to a queue. The queue runs one export
  after the other, also when you close the dialog. **Queue · N** at the top
  opens the queue: **Clear finished**, **Back to settings**, and per item
  **Run earlier**, **Run later**, **Show in folder**, **Remove from the
  list**, **Stop this export**.

The queue is saved. When you quit while an export runs, chukcut asks
"Export running — quit anyway?": **Keep exporting** or **Quit anyway**.
After a restart, the exports that were queued or running are in the queue
again. They do not start on their own: the queue says "N exports from the
last session are waiting" with **Run now**, and the status line says so
too. Adding a new export also runs them. An export that was running when
you quit starts from the beginning. Finished exports stay in the list until
you **Clear finished**. The queue file is
`~/.local/share/chukcut/export-queue.json`.

## Before the export renders

- AI frames that are missing (background mattes, slow-motion frames,
  remade frames) are made first. If they cannot be made, the export stops
  and says why.
- Missing media stops the export. The message names the files.
- When the project uses online media (stock, music library, generated
  sound), the dialog shows a **Licences** box, and the export writes a
  credits file next to the video.
- Captions: see [Text and captions](text-and-captions.md#import-and-export)
  to burn them in or to write an `.srt` next to the video.

## Colour

chukcut writes standard dynamic range (SDR) video:

- **720p and larger**: Rec. 709. **480p and smaller**: Rec. 601. That is
  what players assume when a file says nothing, and chukcut also writes it
  into the file.
- **Limited range** (16–235). This is what every platform expects.
- **HDR clips** (iPhone, Android, GoPro, HLG or PQ) are converted to SDR
  when you edit them. The midtones stay as they were; very bright
  highlights are compressed. The export shows what the player shows.
- chukcut does not export HDR.

From the command line you can choose the matrix (`--color-matrix`), the range
(`--color-range full`) and 10-bit (`--ten-bit`). See
[`docs/cli.md`](../cli.md).

## Hardware encoding

| GPU | Encoder | Codecs |
|---|---|---|
| NVIDIA | NVENC | H.264, HEVC, AV1 (on cards that have it) |
| Intel | VAAPI (and QSV) | H.264, HEVC, AV1 (on chips that have it) |
| AMD | VAAPI | H.264, HEVC, AV1 (on chips that have it) |
| any | software | H.264 (x264), HEVC (x265), ProRes (prores_ks) |

ProRes and GIF always use software. chukcut makes a short test encode with
each hardware encoder before it offers it.

## Save a single frame

Not in the export dialog: use the player's **⋯** button (**Player
options**) › **Save frame as image…**. It writes the frame at the playhead
as a PNG at the canvas size.

## From the command line

`chukcut-cli export` and `chukcut-cli export-queue`. See
[`docs/cli.md`](../cli.md).
