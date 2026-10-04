# Text and captions

## Add a title

Open the asset panel's **Text** tab. The categories are **Basic**,
**Outline**, **Box**, **Glow**, **Retro** and **Templates**.

- Click a style to add a title at the playhead. Or drag it to the timeline.
- When a title is selected, a click restyles that title, and **+** adds a
  new one.
- **Templates** are styles with an animation, for example **Typed note**,
  **Pop headline**, **Neon sign**, **Name tag** and **Breaking**.

Double-click the title on the timeline to type its text there. Or type it in
the inspector.

## Style a title

Select the title. The inspector shows **Text**, **Video**, **Animation**,
**Tracking** and **Effects**. The **Text** tab has:

- **Text**: the words. More than one line is allowed.
- **Font**: the font picker (see below), **Size**, **Style** (**Bold**,
  **Italic**, **Underline**, **Align left**, **Centre**, **Align right**),
  **Letter spacing**, **Line spacing**.
- **Fill**: **Colour**, **Opacity**.
- **Outline**: **Colour**, **Width**.
- **Shadow**: **Colour**, **Offset**, **Blur**.
- **Background** (a box behind the text): **Colour**, **Padding**, **Corner
  radius**.
- **Position**: a 3×3 grid (**Top left** to **Bottom right**). For other
  places, use **Video › Basic › Transform**.

Every change is one undo step.

## Fonts

The font picker has **Installed**, **Popular**, **All**, **Sans**,
**Serif**, **Display**, **Hand** and **Mono**, and **Search fonts**. The
fonts outside **Installed** come from Fontsource. chukcut downloads a font
when you choose it, into `~/.local/share/chukcut/fonts/`.

The switch **Small previews from Google Fonts (sends your IP to Google)**
shows each font's name in that font. It is your choice.

## Animate a title

Open **Animation**. Titles have **In**, **Out**, **Combo** and **Text**.

- **In**, **Out**, **Combo**: see
  [Effects and transitions](effects-and-transitions.md#animations).
- **Text** is the text animator. It moves the title one letter, word or line
  at a time.
  - **Animate**: **Entrance** or **Exit**.
  - Presets: **Typewriter**, **Fade up by word**, **Pop by word**, **Slide
    by line**, **Fade by letter**, **Drop by letter**, **Zoom by letter**,
    **Spin by letter**.
  - **By**: **Letter**, **Word** or **Line**.
  - **Order**: **Forward**, **Backward**, **From centre** or **Random**.
  - **Duration**, **Overlap**, **Strength** and **Easing**.

## Captions

Captions are titles on the caption lane. Open the asset panel's
**Captions** tab. It has **Auto captions**, **Captions**, **Style**,
**Import & export** and **Translate**.

### Auto captions

1. **Transcribe with**:
   - **On this computer**: whisper.cpp. Nothing leaves your computer.
     **Model**: **Tiny (75 MB, fastest)**, **Base (142 MB)**, **Small
     (466 MB)**, **Medium (515 MB, quantised)** or **Large v3 turbo (548 MB,
     best)**. The model downloads once and is checked.
   - **Cloud account**: an OpenAI-compatible server with your own key.
     **Add OpenAI**, **Add Groq** or **Other OpenAI-compatible**, then
     **Test connection**.
2. **Spoken language**: **Detect automatically**, or a language.
3. **Captions**:
   - **Word by word**, with **Words on screen** (1 to 4).
   - Or **Sentences**, with **Characters per line**, **Lines** (1 to 3) and
     **Longest caption** (2 to 7 s).
4. Switches: **Auto emoji: add a fitting emoji to each caption**, and
   **Clear current captions**.
5. Click **Generate captions**. **Cancel** stops it.

**Regroup existing** splits the current captions again with the new
settings, with no new transcription.

Auto captions hear the whole timeline, also the sound inside compound clips.

### Edit captions

The **Captions** category lists every caption. **Add caption** (at the
playhead), **Split at playhead**, **Merge with next**, **Emoji**, **Delete**
and **Delete all**. Double-click a caption on the timeline to edit its text
there.

### Style captions

- **Apply to**: **All captions** or **Selected caption**.
- **Presets**: **Karaoke**, **Yellow**, **Boxed**, **Big**, **Neon**,
  **Minimal**.
- **Font**, **Size**, **Bold**, **Italic**, **Alignment**, **Colour**,
  **Outline** and **Width**, **Background box**, **Drop shadow**.
- **Highlight the spoken word**: karaoke. The word that is spoken lights up
  in the colour that you choose. An SRT file has no word times, so chukcut
  spreads the words over each caption.
- **Position**: **Top**, **Middle** or **Bottom**, or drag the caption on the
  player.

### Import and export

- **Import .srt or .vtt** reads a subtitle file onto the caption lane.
- **Export .srt** and **Export .vtt** write the captions to a file.
- **With the video**: **Burn captions into exported videos** draws them into
  the picture. **Also write an .srt next to every exported video** writes a
  separate file.

### Translate

**Translate** needs a DeepL account or an OpenAI-compatible account (add it
in Settings › **Accounts**). Choose the **Account**, the language (**Into**)
and, for an OpenAI-compatible account, the **Chat model**. Click **Translate
captions**. The translation goes on a new caption lane above the first one.

## From the command line

`chukcut-cli title …`, `chukcut-cli animate-text …` and `chukcut-cli
captions …`. See [`docs/cli.md`](../cli.md).
