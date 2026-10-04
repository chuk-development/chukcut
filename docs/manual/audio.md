# Audio

Select a clip with sound. A video clip with sound and a compound clip have
an **Audio** tab. An audio clip has **Basic**, **Voice changer** and
**Speed**. The sections below are in that tab, in this order.

Most sections have a checkbox and a reset arrow. The checkbox turns the
section off and keeps its values. The arrow removes it.

## Basic

- **Volume**: −60 dB to +12 dB. It can have keyframes.
- **Fade in**, **Fade out**: in seconds. You can also drag the fade dots on
  the clip. See [Timeline editing](timeline.md#fades).

## Normalize loudness

Sets the clip to a loudness target:

- **Target**: **−14 LUFS · social**, **−16 LUFS · podcast** or **−23 LUFS ·
  broadcast**.
- **Loudness** shows the measured value, the result and the change in dB.
  It says "held back by peaks" when the loudest peaks limit the change.

The export can also set the loudness of the whole mix. See
[Export](export.md#audio).

## Reduce noise

Removes steady noise (fans, hum, street noise) from speech. **Strength**:
**Light**, **Medium** or **Strong**. It uses RNNoise, which is in chukcut
itself. No download.

## Remove silences

**Review pauses…** and **Filler words…** open the **Remove silences**
dialog.

- **Pauses**: **Threshold** (dB), **Shortest pause**, **Padding**, and
  **Detect voice** (also cuts breaths and noise that is not speech; slower).
- **Filler words** ("um", "uh" and similar): **Language** and **Padding**.
  This needs captions with word times, from auto captions.
- **Keep in sync**: captions, music and overlays move with the cuts.
- The list shows each cut with a checkbox. Click a row to go to it. **Cut
  all** and **Keep all** change all checkboxes. The summary says how much
  time the cuts remove.
- **Remove N pauses** (or **Remove N fillers**) makes the cuts. It is one
  undo step.

## Isolate voice

Keeps the speech and removes music and noise, or the reverse. It uses an AI
model (HTDemucs) on your computer.

- **Keep**: **Voice**, or **Background** (the music without the voice).
- **Strength**: **Light**, **Medium** or **Full**.

The clip plays as it was until the new sound is ready. Details and cost:
[AI tools](ai-tools.md#isolate-voice).

## Voice changer

Presets: **None**, **Deep**, **Chipmunk**, **Robot**, **Telephone**,
**Megaphone**, with **Intensity**. An audio clip has this as its own tab.

## Audio effects

- **Equalizer**: **Low**, **Mid**, **Mid frequency**, **High**.
- **Parametric EQ**: five bands, each with frequency, gain and width.
- **Compressor**: **Threshold**, **Ratio**, **Attack**, **Release**,
  **Knee**, **Makeup gain**.
- **Reverb**: **Room size**, **Damping**, **Width**, **Mix**.
- **Echo**: **Time**, **Feedback**, **Mix**, **Tone**.
- **Pitch**: in semitones. **Keep voice character** keeps a voice natural.

The preview plays a cached render of the effects. The export renders the
same sound.

## Beats

Tick **Beats**. chukcut finds the tempo and marks every beat on the clip
(**Tempo** shows the BPM and the number of beats). Then:

- **Cut**: **Every beat**, **Every 2** or **Every 4**.
- **Auto-cut to beat** cuts the video above this music on its beats.
- **Snap cuts to beats** moves the cuts that you have to the nearest beat.

These are also in the clip's right-click menu.

## Speed

Speed changes keep the pitch of the sound, unless **Change audio pitch** is
on. See [Speed and slow motion](speed.md).

## Ducking

Right-click a music clip on an audio lane and choose **Duck under speech**.
chukcut lowers the music where someone speaks, with volume keyframes.
**Duck under speech again** does it again after an edit. **Remove ducking**
removes the keyframes.

## Voiceover

Click **Record voiceover at the playhead** in the timeline toolbar. A 3 s
count-in runs. Then the button shows **Stop recording**, the time and a
level meter. Click it again to stop. The recording goes on an audio lane at
the playhead. The file is saved in a folder "<project> Media" next to the
project file.

## Generated sound

With an ElevenLabs or OpenAI-compatible account (Settings › **Accounts**),
the asset panel's **Audio** tab makes sound:

- **Text to speech**: **Voice**, **Model**, **Stability**, **Similarity**,
  **Speed**. The switch **Captions from the voice's word timing** also makes
  captions.
- **Sound effects** and **Music** (ElevenLabs): **Length**, **Seamless
  loop**, **Instrumental, no vocals**.
- **Generate and add at playhead**, or **Generate** and add later from
  **Generated this session**.

## Music and sound libraries

See [Media and import](media.md#audio).
