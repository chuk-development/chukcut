# Text rendering: how a title becomes pixels

Written while building `src-tauri/src/modules/text/`. Read it before touching
that module, before adding a text feature to the document, and before believing
that any of this can be replaced by a crate.

`docs/research/rust-crate-survey.md` §4 chose the dependencies and this
document records what actually happened when they were used, which decisions
were made on top of them, and what is measured.

## The stack, as built

```
fontique 0.11  → system font enumeration and fallback     (via parley)
parley 0.11    → line breaking, bidi, letter spacing, alignment
  └ harfrust   → shaping                                  (pulled in by parley)
skrifa 0.45    → glyph outlines and colour bitmap strikes
zeno 0.3       → scan conversion, including strokes
kurbo 0.13     → path container and rounded rectangles
ours           → compositing, blur, background boxes, caching, per-glyph output
```

The survey's recommendation held with one substitution. Three notes:

- **The survey named no rasteriser**, because it assumed the glyph atlas would
  live on the GPU. It does not, yet: the compositor already knows how to take an
  RGBA texture from a `SourceProvider`, so a text layer that arrives as one is a
  feature with no compositor change at all. `zeno` is the CPU scan converter,
  it is 3.2 M downloads a quarter, and it is the same rasteriser `swash` used.
  Dormant since 2025-06 and feature-complete, which for a path rasteriser is a
  reasonable place to be.
- **`kurbo::stroke` is not used for the outline, and the survey was wrong to
  suggest it.** Expanding each glyph's outline into a stroke *path* and then
  rasterising that path cost about **0.9 ms per glyph** — a 26-glyph title went
  from 12 ms to 35 ms just by turning the outline on — because round joins at
  every corner of every contour produce a path with several times the segments,
  and both the expansion and the scan conversion pay for them. `zeno` strokes
  *during* scan conversion and never builds the path, and the same outlined
  title fell to 22.9 ms and then to 17.9 once the shadow stopped re-stroking it.
  An outline is still the most expensive thing here — see "Measured" — but it is
  half what it was. kurbo stays for `BezPath` and `RoundedRect`, a fair trade
  for a crate we would otherwise be reimplementing.
- **skrifa 0.43 and 0.45 are both in the tree.** parley pins 0.43; we ask for
  0.45 directly. Nothing crosses between them — we hand skrifa the *bytes* of
  the font blob parley resolved, never a skrifa type — so the duplication costs
  compile time and nothing else. Dropping to parley's version would remove it;
  it is not worth a downgrade until parley catches up.

## The decisions that are ours

### A text layer is rasterised at canvas size, not cropped to the text

`render::layout::fit_size` scales a source to fit the canvas preserving aspect
ratio. Hand it a 400x90 image of the word "Subscribe" on a 1080x1920 canvas and
it will draw that word 1080 pixels wide and 240 tall, because that is what
fitting means. So the text layer is the size of the frame, with the text placed
inside it, and the fit becomes the identity.

The consequences are all good ones: the segment's own `Transform` positions,
scales and rotates a title with exactly the code that positions a clip;
`Crop` works; keyframes work. The cost is a full-frame RGBA buffer per distinct
title, which is what the cache exists to amortise.

### Font size is in *document* pixels, and the raster carries a scale

The preview renders a 1080x1920 project into a 540x960 frame. A 72-pixel title
has to come out 36 device pixels tall there, not fill half the screen. So
`RasterOptions::scale` is device pixels per document pixel — `frame.width /
canvas.width` — and it is threaded into parley's own display scale, so glyphs
are *shaped* at the size they are drawn rather than shaped once and scaled.
Hinting and rounding therefore differ slightly between preview and export,
which is correct: each is right for its own resolution.

### The outline width is what you see, so the stroke is drawn at twice it

A stroke is centred on the path. A user who asks for a 4-pixel outline expects
4 pixels *outside* the glyph, so it is stroked at 8 and the fill is painted back
over the inner half.

### The shadow is cast by fill *and* outline, unioned by maximum coverage

Drawing the fill and the stroke into one non-zero fill does not work: the stroke
boundary runs opposite to the glyph contour it came from and the winding numbers
cancel, which shows up as holes in the shadow of outlined text. Each is
scan-converted separately and combined by taking the larger coverage.

### Compositing is premultiplied, output is straight, and edges are bled

Four layers blend over each other and source-over is only associative in
premultiplied alpha; compositing straight-alpha values directly is how a dark
shadow under white text turns the antialiased edges grey. `SourceFrame`
documents its input as straight alpha and the compositor uses
`BlendState::ALPHA_BLENDING`, so the last step divides the colour back out.

That last step leaves fully transparent pixels with no colour, and the
compositor samples the texture bilinearly — so a transparent black pixel drags
a dark fringe into the glyph edge next to it the moment the layer is scaled.
The final pass copies the colour of the nearest painted neighbour into every
transparent pixel next to one. It is the cheap version of what a "premultiplied
alpha bleed" tool does to game textures, and it is not optional.

### Blending happens on sRGB bytes, not in linear light

Technically the wrong place to antialias, and what every text renderer does,
because glyph edges antialiased in linear light look too thin. It also matches
what the user saw in the colour picker. Note that this is a *different* answer
from the one `rust-crate-survey.md` §5 argues for video, and deliberately so.

## What could not be honoured from `TextMaterial`

Everything in `TextMaterial` is drawn. What is missing is on the document's
side, not the renderer's:

- **No line height and no letter spacing.** `TextRequest` carries both and the
  rasteriser honours them; `TextMaterial` cannot express them, so a project can
  only get the defaults. Adding the two fields to the document is a two-line
  change and a `#[serde(default)]`.
- **`background` is a colour with no geometry.** Padding and corner radius are
  invented here (a fifth of the font size, square corners) because the document
  has nowhere to put them.
- **`stroke_width` is a single outline.** CapCut stacks several. The
  architecture supports it — paint N expanded paths back to front — but the
  document describes one.

## What the renderer itself does not do

- **COLRv1 colour glyphs are not painted.** Bitmap strikes are, which is what
  matters on Linux today: Ubuntu's `NotoColorEmoji.ttf` is CBDT/CBLC, verified
  by reading its table directory. A COLRv1-only emoji font would fall back to
  its outline, or to nothing. skrifa exposes the paint graph; executing it —
  gradients, clips, composite modes — is a day's work and has no user waiting
  for it yet.
- **A monochrome bitmap strike (`EBDT`) draws nothing.** It would have to be
  tinted with the fill colour and no font we have met ships one.
- **Vertical CJK is not implemented.** No Rust stack has the layout half of it;
  see the survey §4, which has the shape of the work and the best reference.
- **Justified alignment is not exposed.** parley supports it; `TextAlign` has
  three variants.

## Per-character animation, which is why the layout is returned

`RasteredText` carries the `TextLayout` that produced it and one ink rectangle
per glyph, in image pixel space. That is the whole interface a future
per-character animation needs: take glyph *i*'s rectangle as a sub-quad of the
texture, give it its own transform, stagger by
`layout.glyphs[i].cluster.start` so the stagger follows characters rather than
glyphs — which is what makes it survive Arabic, where one glyph is often two
characters and a ligature is not two things that can fly in separately.

Nothing in the module interpolates anything. It produces the geometry and
stops.

## Caching

Keyed on a hash of everything that changes the pixels: every `TextMaterial`
field, every typographic extra, and the raster options — including the scale and
the target size, because handing the export the preview's raster is the same
soft-picture bug the media provider's size check exists to prevent.

Held under a byte budget (96 MB, about a dozen 1080p layers) with
least-recently-used eviction. The lock is not held across rasterisation: two
threads racing on the same title do the work twice and the second insert wins,
which is cheaper than making every other title wait.

## Measured

Reproduce with:

```bash
cd src-tauri && cargo run --release --example text_bench
RUST_LOG=debug cargo run --release --example text_bench   # per-stage breakdown
```

**Read the caveat before the numbers.** These were taken on a machine running at
three to four times its core count while several agents built Rust, and the
scheduler is the dominant term: the same case measured 4.7 ms and 15.1 ms
minutes apart. The benchmark therefore reports the **minimum of 25 runs**, which
is the closest thing to an uncontended measurement available, alongside the
median so the spread is visible. Treat them as an upper bound and re-measure on
an idle machine before quoting them.

Title: "How I edit 10 videos a day", 72 px, 26 glyphs. Paragraph: five
pangrams, 40 px, 196 glyphs. Milliseconds, minimum of 25.

| | 1920×1080 | 1080×1920 | tight crop |
|---|---:|---:|---:|
| Short title | **5.8** | 6.5 | 4.3 |
| Short title + 4 px outline | 19.9 | 20.9 | — |
| Short title + outline + shadow + box | 29.3 | 32.7 | — |
| Paragraph, 196 glyphs | 39.1 | 25.9 | 19.6 |
| **Any of them, warm cache** | **0.0005** | 0.0005 | 0.0005 |

Shaping and line breaking — all of parley, harfrust and fontique — is **0.1 ms**
for the title and **0.2 ms** for the paragraph, at every size. It is not where
the time goes and it never was. Everything else is our own rasterisation.

### Where the time goes, and what moved it

Per-stage minima at 1920×1080 for the title with everything on, from the
`tracing::debug!` line `raster.rs` always emits:

| stage | ms |
|---|---:|
| glyph outlines out of skrifa | 0.12 |
| background box | ~1 |
| masks + shadow (shift, blur, blend) | ~20 |
| outline blend | 1.5 |
| fill blend | 0.7 |
| unpremultiply and edge bleed | 1.4 |

Three changes, each measured on the 1080p title:

| | plain | + outline | everything |
|---|---:|---:|---:|
| `kurbo::stroke` expanding each glyph | 12.0 | 35.4 | 103.6 |
| zeno's stroker instead | 6.1 | 22.9 | 57.9 |
| shadow reuses the silhouette mask | 4.3 | 17.9 | **31.3** |

- **zeno's stroker instead of `kurbo::stroke`** — 1.5× on an outlined title,
  1.8× with everything on.
- **The shadow is the silhouette moved, not scan-converted again** — another
  1.9× with everything on. `Coverage::shifted` is a memcpy where a second and
  third stroke pass used to be.
- **`Join::Miter` is not the cheap one.** It seems it should be — a mitred join
  is two lines where a round join is an arc — and it measured **54–62 ms against
  15 ms** for the same outline. Round joins stay.

The remaining cost is the stroke scan conversion itself, about 13 ms for 26
glyphs at 1080p, which is what makes an outlined title four times a plain one.
The idea worth trying next is not a faster stroker: it is dilating the fill
*mask* instead, which is a distance transform over an 8-bit image and is
independent of how many contours the glyphs have.

### Why none of it matters much

A title does not change between frames, so the number that governs playback is
the warm one: **half a microsecond**, a hash and an `Arc` clone. The cold cost
is paid once when the text is edited, and 30 ms of that is not felt in an
editor. It would matter for animated *content* — a live word counter, a
timecode burn-in — and that is when the outline cost above becomes worth
attacking.

## How a title reaches the screen, as built

Nothing in `render/` changed. The compositor already takes a straight-alpha RGBA
texture from a `SourceProvider`, so a text layer that arrives as a canvas-sized
texture is drawn by code that already existed.

```
Text tab / preset            src/modules/text/components/TextPanel.tsx
  └ text_add                 src-tauri/src/modules/text/commands.rs
      ├ edit::default_material     size, colour, outline, shadow — pure
      └ edit::insert_command       lane, instant, one Composite — pure
          └ History::apply         an ordinary undoable edit

Inspector panel              src/modules/inspector/components/TextInspector.tsx
  └ text_set (debounced 140 ms)    replaces the material in the pool

Every frame                  media/provider.rs::text_frame
  └ TextRenderer::rasterize_material   cache hit: ~0.5 µs
      └ upload_rgba → the compositor's ordinary quad
```

Three things about that chain are load-bearing and easy to undo by accident:

- **`text_frame` caches per material *and per size*.** A title does not vary
  with time, so any cached upload at the right size is valid whatever instant
  was asked for — but the size check is what stops an export reusing the
  preview's smaller raster, which is a visibly soft title in the delivered file.
  `tests/text_clip.rs::one_provider_does_not_serve_the_preview_raster_to_the_export`
  is that check, from the outside.
- **The generic `self.cached(...)` lookup at the top of `SourceProvider::frame`
  compares `source_time`, and a title has none.** It misses and falls through to
  `text_frame`, whose own lookup ignores time. That is correct today; if the
  generic check is ever changed to serve text as well, it must not key on the
  instant.
- **The scale is `frame.width / canvas.width`, computed in the provider.** Both
  the preview and the export go through it, which is why they agree. See below.

### The identity between preview and export, and how it is proved

`src-tauri/tests/text_clip.rs`. The preview and the exporter build the same
`MediaSourceProvider` and call the same `Compositor::render_frame`; the only
difference between `preview/server.rs::render_one` and `export/job.rs` is the
size they pass. So:

- Same size — every project up to 1920 on the long edge — must be
  **byte-identical**, and is: 0 of 921,600 pixels differ.
- Smaller preview — a 4K canvas previews at 1920 — must put the title's ink in
  the same place proportionally, within **2 preview pixels**. Glyphs are shaped
  at the size they are drawn rather than shaped once and scaled, deliberately
  (see "Font size is in *document* pixels" above), so hinting and rounding do
  differ between the two and an exact match is not the right assertion.
- Through the real exporter, decoded back out of the H.264 file and scored
  against the preview frame: above 30 dB, with a blank frame and a title-free
  frame as controls.

## What the document still cannot say

"What could not be honoured from `TextMaterial`" above is unchanged by the UI
work: the inspector exposes every field the document has, and the three gaps
listed there — line height, letter spacing, background geometry, stacked
outlines — are still gaps. Building the panel found one more:

- **Vertical alignment inside the canvas is not document state.** The raster is
  always vertically centred and the segment's `Transform` moves it, which is why
  the inspector's quick placements write `transform.position` rather than a text
  field. That is the right factoring — it keyframes for free — but it does mean
  a title cannot be "anchored to the bottom" independently of its transform.

## Changing a title is not undoable

`text_set` writes the material pool directly, because there is no `EditCommand`
variant carrying a `TextMaterial` and `timeline/ops.rs` was owned by other work.
Adding and deleting a title *are* undoable; the words, font and colours are not.
The fix is an `EditCommand::SetTextMaterial { id, before, after }` mirroring
`SetTransition`. Reasoning in `src-tauri/src/modules/text/commands.rs`.

## Traps found while building this

- **`parley` will not tell you a font is missing.** `fontique` silently skips a
  family it cannot find, so a request for one bad name leaves the shaper with an
  empty family list and only script-based fallback. `font::family_stack` always
  appends a generic family, which is what makes a missing font degrade instead
  of depending on the script of the text.
- **`GlyphRun::glyph_start` is private**, so mapping a positioned glyph back to
  the characters it came from means tracking parley's own counter: it restarts
  when the *line item* changes and consecutive glyph runs from one item continue
  it. Keying on the item's cluster range rather than the run index is what makes
  that reliable.
- **A zero-size texture is not allowed** and an empty title is a real thing a
  user makes by clearing the text box. Every dimension is clamped to at least 1.
