# Media and import

## Import

chukcut reads every format that FFmpeg reads: video, audio and pictures.

- **Menu › Import media…** or `Ctrl+I`.
- The **Import** tile in the asset panel's **Media** tab.
- Drag files from your file manager onto the asset panel.
- Start chukcut with files: `chukcut a.mp4 b.mov` makes a new project with
  them.

An imported file with picture and sound becomes two linked clips on the
timeline: one on a video lane and one on an audio lane. They move, trim,
split and delete together. See [Timeline editing](timeline.md#linked-audio-and-video).

A GIF, an animated WebP or a Lottie `.json` file becomes an animated
sticker. See [Effects and transitions](effects-and-transitions.md#stickers).

## The Media tab

The asset panel's **Media** tab has three categories:

- **Import**: every file that the project has imported. Each tile shows the
  length (`mm:ss`). The badge **Added** shows that the file is on the
  timeline.
- **Project media**: only the files that are on the timeline.
- **AI tools**: cloud processing with your fal.ai account (see below).

To put a file on the timeline:

- Drag its tile onto the timeline, to the lane and time where you want it.
- Or point at the tile and click **+**. chukcut adds the file at the end of
  the first lane of its kind.

A click on a tile only selects it.

## Missing media

A project refers to its files by their paths. When a file is gone (moved,
deleted, or on a drive that is not mounted):

- The clip shows red with an "offline" icon on the timeline and in the
  media list.
- The player shows a dark red picture for it.
- The export refuses to start and names the missing files.

Put the file back at its path, and the clip works again. To use another
file, select the clip and use **Replace media…** in its right-click menu.

Removing a file from the project is an edit that you can undo. The clips
that used it stay, as offline clips. The file on the disk is not touched.

## Audio

The **Audio** tab of the asset panel:

- **Import**: import music and sound effects.
- **Project audio**: the sound files of the project. Rows show "on the
  timeline" when they are used.
- **Music library**: music by mood (**Upbeat**, **Chill**, **Funny**,
  **Cinematic**, **Calm**), by Kevin MacLeod (incompetech.com), CC BY 4.0.
  The export writes the credit. **Play** and **Add**.
- **Sound library**: free sound packs (**Interface**, **Impacts**,
  **Foley**, **Sci-fi**, **Retro synth** and more). **Download pack** gets a
  pack once.
- **Text to speech**, **Sound effects**, **Music**: generated with your own
  ElevenLabs or OpenAI-compatible account. See [Audio](audio.md#generated-sound).

## Stock

The asset panel's **Stock** tab searches stock sites with your own accounts
(add them in Settings › **Accounts**):

- **Videos** and **Photos**: Pexels or Pixabay (**Source**).
- **Sounds**: Freesound. **Show non-commercial** also shows sounds that are
  not free for commercial use.

Type words and press Enter. A click on a picture tile adds it to the
library. **+** adds it at the playhead. Sounds have **Play** and **Add at
playhead**. The attribution link is at the bottom.

chukcut keeps the licence and the author of each downloaded file with the
project. When the export uses online media, the export dialog shows a
**Licences** box, and the export writes a credits file.

## AI tools in the Media tab (cloud, optional)

**Media › AI tools** sends a clip to fal.ai with your own account:
**Remove background**, **Upscale ×2** and **Smooth motion**. The panel shows
**Estimated cost** before you click **Run for $X.XX**. **Result** puts the
new clip **On a lane above the clip**, or uses it to **Replace the clip**.

These cloud tools are different from the local AI tools. The local tools
run on your computer and cost nothing: see [AI tools](ai-tools.md).

## Proxies

For heavy footage (for example 4K HEVC), chukcut can make small proxy files
for the preview. The export always uses the originals. See Settings ›
**Proxies and cache** in [Settings](settings.md#proxies-and-cache).
