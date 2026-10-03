# The built-in asset library

Fonts, stickers, music, sound effects and colour looks that a creator can put
into a monetised video. The legal research is `docs/research/open-assets.md`;
this file says what was built from it, where the files go, and what is not
done yet.

## Shape

```
crates/engine/src/modules/library/
  licence.rs    SPDX id -> provenance::Licence; the policy: show, warn, hide
  net.rs        one cached fetch; a stale copy when offline; plain messages
  fonts.rs      Fontsource catalogue, preview tiles, install, registration
  stickers.rs   emoji index (Unicode + Fluent tree), Noto, Iconify, placement
  sounds.rs     curated Incompetech list, full catalogue, CC0 sound packs
  looks.rs      28 procedural looks written as .cube files
  commands.rs   library_* commands for the app, a CLI and MCP
crates/app/src/editor/
  font_picker.rs              the picker the title and caption controls open
  assets/stickers.rs          the Stickers tab
  assets/library_audio.rs     Audio tab: "Music library", "Sound library"
  assets/looks.rs             Filters tab: "Looks"
  assets/library_panel.rs     the panel state and shared pieces
  inspector/text_style.rs     the title's Text tab (font)
```

Every file that can reach the timeline is written with an `asset.json`
sidecar (`cloud::provenance`, kind `library`). Import copies the record into
the project, so the export's licence summary and credits file see library
items as they see stock. Library audio credits under "Sound and music",
library pictures under "Stickers".

## Where files live

| What | Where | Why there |
|---|---|---|
| Catalogues (font list, emoji index, music list, icon sets) | `~/.cache/chukcut/library/catalogues/` | can be fetched again |
| Tracks, sounds, stickers, icons | `~/.cache/chukcut/library/<provider>/<item>/` with `asset.json` | same as stock downloads |
| Panel thumbnails, font previews | `~/.cache/chukcut/library/thumbs/` | no licence record needed |
| Installed fonts | `~/.local/share/chukcut/fonts/<id>/` with `font.json` and `LICENSE.txt` | a project must still draw its title after "clear cache" |
| Looks | `~/.local/share/chukcut/luts/*.cube` (the LUT library) | the inspector's LUT picker lists them too |
| Settings | `~/.config/chukcut/library.json` | the font preview host |

`TextRenderer::shared()` loads every font under the fonts directory when it
is built, so the preview, the export, a CLI and an MCP server draw the same
face.

## Sources

| Asset | Source | Licence | How |
|---|---|---|---|
| Fonts | Fontsource API, jsDelivr | OFL-1.1, Apache-2.0, UFL-1.0 (others hidden) | catalogue cached 7 days; regular, bold, italic TTF on pick; licence text from google/fonts |
| Font previews | Google Fonts CSS2 `text=` subset (about 9 KB), or Fontsource latin file | as the font | switch in the picker; drawn by our renderer into a PNG |
| Emoji | Fluent Emoji 3D and Flat (MIT), Noto Emoji 2D PNG (Apache-2.0) | as stated | Unicode `emoji-test.txt` + Fluent repository tree, joined by name, cached 30 days |
| Icons | Iconify API | per set; NC, GPL, unknown hidden; logos, programming, archived sets hidden; CC BY-SA shown with a warning | search, SVG rasterised with resvg, drawn white unless the set is multicolour |
| Music | Incompetech (Kevin MacLeod) | CC BY 4.0, credit line stored | 50 curated tracks by mood, plus `pieces.json` (about 1,400 tracks) |
| Sound effects | Kenney (8 packs), OpenGameArt rubberduck and SubspaceAudio (8 packs) | CC0 | zip on first open, one directory per sound |
| Looks | our code | GPL-3.0-or-later, free to use in any video | 33-point cubes, installed once per `LOOKS_VERSION` |

Requests carry `User-Agent: chukcut/<version>` and nothing about the user.

## Behaviour without a network

- A catalogue that was fetched once answers from disk; past its time to
  live, a failed refresh falls back to the old copy and the panel says
  "Offline: … from the last time".
- Downloaded tracks, opened sound packs, used stickers, installed fonts and
  the looks need no network.
- With nothing on disk, the panel says "Could not download …: no
  connection. Everything already downloaded still works offline." and shows
  "Try again". Failures stay until the user retries, so an offline machine
  does not ask the network on every frame.

## Not done, and why

- **Animated stickers.** Noto Animated Emoji are Lottie. There is no Lottie
  renderer in the engine: velato needs vello on our wgpu device, and
  dotlottie-rs builds ThorVG from C++. Static stickers only.
- **Font subsets.** Fontsource publishes per-subset files only, and parley
  picks one face per family. An install fetches the `latin` subset (or the
  family's default subset); text in another script falls back to a system
  face.
- **Wikimedia Commons and Musopen** (public-domain classical recordings) are
  not built: the Wikimedia User-Agent policy wants contact details in every
  request, and requests from chukcut carry none. It needs a project identity
  first (owner decision, see the research).
- **Jamendo, ccMixter, Openverse, Twemoji, OpenMoji** are not built; the
  research rates them "care" or optional.
- **No "Licence" button** on a timeline segment yet; the record is in the
  project and in the credits file.
- **Kenney zip links** carry a version hash. When a stored link fails, the
  pack's page is read for the current one; if Kenney changes the page, the
  table in `sounds.rs` needs new links.
- **Sticker duration** is fixed at 3 s and the scale at 40% of the short
  side when added.
