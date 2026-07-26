# Where this project stands

Written to be read first by anyone — human or agent — picking this up cold.
Sessions are long and are not reopened, so nothing important is allowed to live
only in a conversation. If you learn something that would change how the next
person works, it belongs in this repository, not in a chat log.

Last updated: 2026-07-25.

## What this is

`chukcut` — a CapCut-style video editor. Rust engine, Tauri 2 shell, React
frontend. Everything lives in this repository at `~/git/chukcut`. There is no
other source tree; earlier attempts (`~/git/chukcut-rust`, `~/git/x`) are
reference material only and are described under "Prior work" below.

Run the built app:

```bash
cd ~/git/chukcut
pnpm tauri build --no-bundle          # ~1.5 min incremental
./src-tauri/target/release/chukcut
```

Do **not** judge performance from `pnpm tauri dev`. See "Traps" below.

## What works, verified

Each of these was measured or checked against an independent tool, not assumed.

| Capability | Evidence |
|---|---|
| Import of arbitrary formats | FFmpeg probe; canvas and frame rate adopted from the first clip |
| Frame-accurate decode | `examples/render_smoke.rs` — output matches ffmpeg's own frame at the same timestamp to a max channel delta of 2, no pixel differing by more than 8 |
| GPU compositing | wgpu on Vulkan, Intel Raptor Lake iGPU; transform, crop, opacity, keyframes |
| Preview playback | ~13.7 ms per frame at p50 against a 33.3 ms budget, decode p50 7.2 ms |
| Audio playback | cpal, 48 kHz stereo; the device's played-sample count is the clock master |
| Timeline editing | Magnetic docking, razor at the pointer, undo/redo via invertible commands |
| Export | `examples/export_smoke.rs` — 240 declared **and** 240 decodable frames, exact 4.000 s duration, AAC track at −18.2 dB mean, −1.7 dB peak |
| Hardware preview JPEG (VAAPI) | `mjpeg_vaapi` on the Intel iGPU. A 1080x1920 preview frame encodes in 6.2 ms against 31 ms for the old pure-Rust encoder, and matches the software encoder's picture at 37 dB PSNR. Falls back to libjpeg-turbo on any machine or frame size the device refuses |
| Hardware export (VAAPI) | `h264_vaapi` and `hevc_vaapi` on the Intel iGPU. 240 declared and 240 decodable frames, exact 8.000 s, audio identical to the software export at −17.7 dB mean. Frames match the software encode at 51–53 dB PSNR on luma and 60–62 dB on chroma |
| Hardware decode (VAAPI) | H.264, HEVC, VP9 **and AV1**, each probed by decoding a real embedded frame. `tests/decode.rs` — 34 tests, 22 of them run against both the software and hardware decoders and pass identically, including every seek, VFR and rotation case. The two decoders produce the same picture to a mean channel difference under 2 |
| Zero-copy decode into wgpu | `examples/dmabuf_import.rs` — a decoded VA surface exported as DMA-BUF and imported as two wgpu textures reconstructs the software decode's picture to a mean channel difference of **0.32**, with a deliberately chroma-swapped control at 41.9. Not yet wired into the compositor; see below |

Two of those deserve emphasis because they are the failure modes that usually
go unnoticed: the export is **not** truncated (the classic un-flushed-encoder
bug), and its audio is **not** silent.

## What is known to be rough

- **Seeking backwards during playback stalls briefly.** The ring buffer is
  discarded on every seek, so a backward seek into a region just played costs a
  full decoder seek (130–227 ms measured). Task #9: key the cache to document
  identity rather than to the session id.
- **Speed changes shift pitch.** Both in preview and export. A phase vocoder is
  its own piece of work.
- **The first `play` after launch can lead the sound** by up to one audio
  buffer, because the queue origin is marked before the stream exists. Every
  subsequent play and seek is exact.
- **The export encoder can run on the GPU; most of the export still does not.**
  VAAPI H.264/H.265 works and is measured below. The gain is real but it is
  **1.5×, not the order of magnitude the decision document projected**, because
  the encoder stopped being the bottleneck and nothing else moved. Numbers and
  the reason are under "Hardware export, measured" below.
- **Waveforms and thumbnails are done and stream; nothing here is known rough.**
  Recorded because the next person will otherwise re-derive the two traps:
  a file with a channel *count* and no channel *layout* (plain PCM WAV, and
  therefore most of what gets dropped on an audio lane) decodes to frames whose
  `AVChannelLayout` is `AV_CHANNEL_ORDER_UNSPEC`, and `ffmpeg-next` 6.1 still
  configures swresample through the legacy mask API, so **every such frame is
  rejected with "Input changed"**. `waveform.rs::name_the_layout` relabels the
  frame; the same eight lines are in `audio/decode.rs`, which met it first.
  `tests/media_data.rs` builds the WAV by hand rather than with `ffmpeg`,
  because an `ffmpeg`-written file may carry a mask and then proves nothing.
  Second: a peak-only waveform draws a solid block for anything compressed, so
  the envelope carries signed min/max **and** RMS per bucket.
  Third, and the one that will bite anything else that caches derived data:
  **size plus mtime is not a file identity.** The kernel stamps mtime at clock
  tick granularity, so a file rewritten in place without changing length keeps
  its key and serves stale thumbnails forever — a unit test writes `one` then
  `two` and both writes land in the same tick. `thumbnails.rs::fingerprint` now
  folds in 8 KB from each end of the file as well, which costs nothing against a
  decode and catches every container whose header or trailer moves. Both the
  thumbnail and waveform caches share that one function on purpose, so a single
  edit invalidates everything derived from a file at once.

## Hardware export, measured

Measured 2026-07-25 on the Raptor Lake iGPU, median of three runs, 240 frames
of real phone footage from `~/git/editing/footage`, through
`examples/export_smoke.rs`. `--encode-only` feeds the encoder synthetic frames
instead of composited ones, so the difference between the two rows is
everything upstream of the encoder.

| | 1920×1080 | 1080×1920 |
|---|---|---|
| Whole export, software (`libx264` medium) | 16.1 fps | 14.2 fps |
| Whole export, VAAPI (`h264_vaapi`) | **24.2 fps** | **21.3 fps** |
| Encoder only, software | 16.5 fps | 20.1 fps |
| Encoder only, VAAPI H.264 | **80.6 fps** | **63.4 fps** |
| Encoder only, VAAPI H.265 | 60.8 fps | 51.9 fps |

Run-to-run spread on this machine is wide — software at 1080×1920 was measured
anywhere from 11.3 to 16.9 fps across a session, apparently with thermal state —
so single runs are not worth comparing and a change of less than about 20%
cannot be read off one. Use `--clip=8` or longer and take a median.

Read those two pairs together, because the story is in the gap:

- **The encoder itself got 3–5× faster.** That part of the decision document
  held up.
- **The whole export got 1.5× faster**, at both aspect ratios, and the number is
  suspiciously identical because the same thing limits both.
- What limits it is **everything the encoder is not doing**: decoding the
  sources, compositing, reading the frame back off the GPU, and converting RGBA
  to NV12 on the CPU. At 1080p the hardware export spends about 41 ms per frame
  and the encoder accounts for roughly 12 of them.
- The software comparison also flatters itself: **x264 encodes on eight threads
  in the background**, so a large part of its cost already overlaps with
  decoding and compositing. VAAPI has less to overlap.

So the next win is not a faster encoder. It is, in order: not reading the frame
back to the CPU (`docs/research/zero-copy-encode.md`), doing RGBA→NV12 on the
GPU, and hardware *decode*. The last of those was ranked third in the decision
document and the numbers above argue it should be higher.

## Hardware decode, measured

Measured 2026-07-26, `examples/decode_bench.rs`, median of three runs of 90
sequential frames of real footage, first frame discarded. Full working in
`docs/research/hardware-decode.md`.

| per frame | 1920×1080 H.264 | 1080×1920 H.264 | 1920×1080 VP9 | 1080×1920 VP9 |
|---|---:|---:|---:|---:|
| Software decode → RGBA | 26.9 ms | 31.7 ms | 15.1 ms | 38.4 ms |
| Hardware decode → RGBA | 37.3 ms | 26.1 ms | 18.8 ms | 54.2 ms |
| Hardware decode → DMA-BUF | **1.70 ms** | **2.09 ms** | **1.71 ms** | **2.37 ms** |

The middle row is the surprise and it is the point of the whole exercise:
**hardware decode is slower than software when the frame has to end up in system
memory**, in three of these four cases. A VA surface is `Y_TILED`, so
`av_hwframe_transfer_data` is a detiling pass over the whole frame, and swscale
still has to turn NV12 into RGBA afterwards. Together they cost more than the
decode they were supposed to accelerate — the decode itself is under 2.5 ms.

The bottom row is what the frame costs when nothing copies it. That is 9–16×,
and it is the number the preview and the export are both waiting on. It is
**not reachable yet**: it needs `render/` to import the exported surface as a
texture, and the exact patch for that is at the bottom of
`docs/research/hardware-decode.md`. Until it lands,
`media::provider::DEFAULT_ACCELERATION` is deliberately `Software`, with a
`CHUKCUT_DECODE=software|auto|vaapi` environment override for measuring.

On an integrated GPU the lesson generalises and is worth carrying into the next
piece of work: **"move it to the GPU" is not the optimisation. "Stop copying it"
is.**

**Both of the first two are now done.** See the next section.

## Proxy media, measured

`src-tauri/src/modules/proxy/`, decision 0003. Measured 2026-07-26 on the
Raptor Lake machine. **The machine was at load ~29 while these ran**, so every
absolute figure is roughly twice what a quiet machine gives — a quiet run of the
first row measured 40.9 ms. The ratios are what the numbers are for.

Decode of one frame to RGBA at preview size, single-threaded, which is what
`media::provider` pays (`ffmpeg -threads 1 -i F -vf scale=-2:1080,format=rgba -f
null -`, 240 frames):

| | per frame |
|---|---:|
| 4K H.264 source → 1080-tall RGBA | 84–98 ms |
| 4K HEVC source → 1080-tall RGBA | 116–136 ms |
| **720p all-intra proxy → RGBA** (`libx264 -tune fastdecode`) | **7.9–12.4 ms** |
| 720p all-intra proxy → RGBA (`h264_vaapi`) | 15.3–18.8 ms |

So a proxy is worth **8–11×** against 4K H.264 and **10–15×** against 4K HEVC,
which is the difference between a timeline that plays and one that does not.

Three things there that are not obvious and are worth not rediscovering:

- **How the proxy was encoded changes how expensive it is to decode, by 50%.**
  Same content, same 1280×720, four encoders: `libx264 -crf 23 -g 1` 18.1 ms,
  the same plus `-tune fastdecode` 12.4–15.8 ms, `h264_vaapi -qp 23 -g 1`
  23.0–26.5 ms, and the same plus `-coder cavlc` 18.1–19.5 ms. `fastdecode` and
  `cavlc` are the same trade — CABAC and the deblocking filter off — and both
  are now set. The hardware encoder's *default* output was the most expensive
  proxy of the four.
- **Hardware encoding buys almost nothing here.** The whole 4K→720p transcode
  took 11.33 s in software and 10.40 s on VAAPI: **8%**, because decoding the 4K
  source and scaling it dominate. Same conclusion as the export, one section up.
  The hardware path is still the default because it is what the user waits on,
  and `CHUKCUT_PROXY_ENCODER=software` takes the other side without a rebuild.
- **FFmpeg guesses the muxer from the output filename**, so an atomic write to
  `proxy.mp4.part` fails inside `format::output` with a bare `EINVAL` and no
  hint. The partial file is `.partial-proxy.mp4` — a prefix, not a suffix.

## The export's frame path, measured stage by stage

Measured 2026-07-25/26 on the Raptor Lake iGPU, 1920×1080, `h264_vaapi`, 480
frames of `dont_regret_60fps.mp4` through `examples/export_smoke --clip=8`.
Every run prints a `BREAKDOWN` and an `ENCODESIDE` line; these come from
`render::RenderStats` and `export::WriterStats`, which are always on.

The export now runs at whichever of three tiers the machine reaches, each a
strict improvement on the one below, each falling back automatically:

| | tier 1 `--no-gpu-nv12` | tier 2 `--no-zero-copy` | tier 3 (default) |
|---|---|---|---|
| Colour conversion | swscale, CPU | compute shader | compute shader |
| What crosses the bus | 8 MB RGBA back, 3 MB NV12 up | 3 MB NV12 back, 3 MB up | **nothing** |
| Readback (`readback`/`nv12`) | 5.1 ms | 3.0 ms | 1.5 ms |
| Frame prep (`prepare`) | 14.8 ms | 0.8 ms | **0.05 ms** |
| Encoder submit (`submit`) | 3.9 ms | 3.5 ms | **0.4 ms** |
| Sum of those three | 23.8 ms | 7.4 ms | **2.0 ms** |
| Whole export | 24.0 fps | 41.4 fps | **77.3 fps** |

Two cautions on that table. The whole-export row is the noisiest thing here —
the decode stage (`sources`) was measured anywhere from 12.8 to 76 ms per frame
across a session depending on what else the machine was doing, and it dominates
now. The three *stage* columns are ratios measured inside a single run and are
stable; the fps column is one quiet triple and should not be quoted to three
figures. Take medians, and only compare runs taken back to back.

### The isolated readback, which nobody had measured

`Compositor::render_frame`'s map-and-copy costs **4.4–5.7 ms per 1080p frame**
on a quiet machine, median ≈ 5.1 ms, split roughly evenly:

- **2.1–2.6 ms blocked in `device.poll`**, waiting for the GPU to finish
  compositing and copying. This is a pipeline stall, not bandwidth.
- **2.1–2.9 ms** copying the mapped rows into a `Vec` and stripping the
  256-byte row padding. Pure CPU memory bandwidth, 8 MB a frame.

Against the 39 ms frame the export used to cost, the readback was 13% — so
`zero-copy-encode.md`'s worry that it might be "4 ms of a 15 ms frame" was
close on the absolute number and wrong about the ceiling, because the thing
next to it was much larger than assumed. **The RGBA→NV12 swscale pass was 14.8
ms**, not the ~9 ms the research document estimated for steps ② to ④, and it
was the single largest item in the export after decoding.

### What each step bought

- **RGBA→NV12 on the GPU (tier 2)** is the big one: 1.7× on the whole export,
  and it deletes 14 of the 15 ms of frame preparation outright. It needs no
  DMA-BUF, no `ash`, and no driver cooperation — a compute pass and a storage
  buffer.
- **Zero-copy (tier 3)** is another 1.8×. Most of it is not the readback: it is
  `submit` falling from 3.5 ms to 0.4, because `av_hwframe_transfer_data` was
  most of what "submitting a frame to the encoder" cost.

### Verified by pixels

Against a `libx264` export of the same timeline, decoded at the same timestamps:

| | luma | Cb | Cr |
|---|---|---|---|
| Tier 1, the old path | 52.04 dB | 55.64 | 57.06 |
| Tier 2, GPU NV12 | 52.00 dB | 55.47 | 57.14 |
| Tier 3, zero-copy | 52.00 dB | 52.73 | 51.45 |

And against each other: tier 2 vs tier 1 is 55.1 dB on luma — the compute
shader and swscale disagree by a fraction of a code value. Tier 3 vs tier 2 is
**infinite on luma**: the encoder received bit-identical luma whether the
surface was uploaded or imported. Chroma differs by at most 4 code values on 6
to 10% of samples, which is the VAAPI encoder making marginally different
decisions on a linear imported surface; the DMA-BUF import itself is proven
byte-exact by `hwframes.rs`'s round-trip test.

The controls that say those numbers mean something: a deliberately
chroma-swapped tier-3 output scores 40.9/41.3 dB on U and V, and comparing
frames one apart scores 40.5 dB on luma. So the new paths are not green, not
plane-swapped and not offset by a frame.

**Do not read a quality verdict into these numbers.** x264's CRF and VAAPI's QP
are not the same scale, so "the same quality setting" produces different files:
on the same eight-second timeline the hardware output was 17% larger at
1080×1920 and 5% smaller at 1920×1080. Neither figure means much on its own —
comparing the two properly means a rate-distortion sweep, which nobody has done
here. This is the reason software stays the default: its result is predictable
across machines and the hardware's is not.

The pictures themselves match, though, which is the thing that had to be
checked: decoding both files at the same timestamps gives 51–53 dB PSNR on
luma and 60–62 dB on chroma. For calibration, a deliberately chroma-swapped
control scores 42.6 dB on U and V, and comparing frames one apart scores
34–40 dB on luma. So the hardware path is not green, not chroma-swapped and not
offset by a frame — which are the three ways it usually goes subtly wrong.

## Hardware preview JPEG, measured

Measured 2026-07-25 on the Raptor Lake iGPU. Every preview frame is served to
the webview as a JPEG, so this cost is a direct term in the 33.3 ms playback
budget — and at native resolution it used to be most of it.

Median/best of 21 interleaved runs at load average 3, quality 88, milliseconds
per frame. Full detail and the traps are in
`docs/research/vaapi-jpeg-preview.md`; the benchmark is
`cargo run --release --example preview_jpeg_bench`.

| | jpeg-encoder (was) | libjpeg-turbo | VAAPI (now) |
|---|---|---|---|
| 540x960 | 10.9 / 6.5 | 4.6 / 2.2 | **2.3 / 1.4** |
| 1080x1920 | 31.1 / 26.7 | 10.2 / 8.5 | **6.2 / 4.7** |
| 1920x1080 | 31.4 / 26.2 | 11.2 / 8.8 | **6.1 / 4.7** |

Two independent steps, and it is worth keeping them apart:

- **The crate swap was 3.1x and cost nothing.** `jpeg-encoder` is portable Rust
  with no SIMD; libjpeg-turbo is nothing but SIMD. If the hardware path ever has
  to be turned off, this is what it falls back to and it is still three times the
  old speed.
- **The hardware was a further 1.6x**, for 5.0x end to end.

**Read the 1.6x with the breakdown, because it is the useful part.** Per
1080x1920 frame the hardware path is 1.6 ms of RGBA-to-NV12, 2.2 ms of upload
into a VA surface, and 2.0 ms of actual encoding. The fixed-function encoder is
**a third** of its own path; the rest is carrying pixels to a chip that already
had them — the compositor renders into a GPU texture, and the preview reads
8 MB back to system memory only to send 3 MB straight back.

So, as with the export, the next win is not a faster encoder. It is
`render::nv12` (colour conversion in a compute shader, which also halves the
readback) and then DMA-BUF export, which is `docs/research/zero-copy-encode.md`.

**The reduced playback resolution is gone.** `PreviewSession` briefly carried
two sizes — native for a parked frame, reduced for playback — purely because the
JPEG encode overran the budget at 1080x1920. At 6.2 ms it does not, and the
encode runs on a thread that overlaps compositing besides, so playback renders
at the project's own resolution again. What is left in the frame budget is
compositing, the readback, and decoding.

Two things about the hardware path that are not obvious and cost time to find:

- **swscale cannot do RGBA to NV12 usefully.** It was the first implementation
  and measured 37 ms per 1080x1920 frame — more than the entire frame budget,
  and three times what libjpeg-turbo needs for a whole JPEG. There is no SIMD
  path for that format pair. `preview::vaapi::rgba_to_nv12` replaces it and is
  about twenty times faster.
- **JPEG is full range and every YUV routine defaults to limited.** There is no
  range field in a JPEG file, so decoders assume 0-255, but swscale and friends
  produce 16-235 by default. The result is a valid file with grey blacks that
  reads as the editor having washed out the footage. It scores 27 dB against the
  software encoder instead of 37.

## Known defects, found but not yet fixed

Found by the integration suite. Each is real and reproducible; none has a red
test left behind, so `cargo test` is green despite them.

- **The app opens two GPU devices** — the preview's render loop and the export's
  lazily-initialised compositor. Given that concurrent Vulkan instances were
  observed crashing this driver during testing, these should be collapsed into
  one shared context.

## The document layer's defects, fixed

The five defects this section used to list are fixed, each with a test that
fails without the fix. What is worth carrying forward is *why* they were
invisible, because the same blind spots will produce the next ones.

- **A non-finite float is silent data loss, and only the save path can see
  it.** `serde_json` cannot write a NaN or an infinity — it writes `null` — so
  a project that took one saved successfully and then failed to load forever
  after with "invalid type: null, expected f32". Nothing in memory is wrong,
  which is why every in-memory assertion passed. Non-finite values are now
  refused at the edit-command boundary (`timeline/ops.rs::check_finite`), are
  errors in `validate()`, and `project/migrate.rs::repair_non_finite` opens the
  files that already have them: a `null` at a key is dropped so the field's
  documented default applies — an opacity comes back at 1.0, not 0.0 — and the
  user is told which values were reset. **Every float in the document now
  carries a `serde` default**, which is what makes that repair possible.
- **`schema_version` now gates loading.** `project/migrate.rs` is the only
  place allowed to turn bytes into a `Project`. A newer format is refused with
  a sentence naming both versions and what to do; an older one walks the (still
  empty) `STEPS` ladder. Adding version 2 is a bump and one table entry.
- **`SetSpeed` moves the source range with the speed.** The invariant
  `source_range.duration = target_range.duration × speed` is what `split_at`,
  the mixer and the exporter all read; the command used to change only the
  factor, so a sped-up clip cut in the wrong place. The clip keeps its position
  and length on the timeline and its source range changes, which is the
  direction the rest of the codebase already assumed (`audio/mixer.rs`,
  `export/audio.rs` both derive the source span from the speed). Every command
  that writes a source duration now writes
  `document::source_duration_for(target, speed)` — one function, so undo stays
  byte-exact instead of drifting a microsecond per edit.
- **`validate()` covers what the fuzzer would have found**: negative
  `target_range.start`, source-range sanity, duplicate segment *and* track ids,
  non-finite values, the speed invariant, and keyframe tracks (sorted, unique
  times, no empty track). `edit_fuzz.rs` asserts those invariants
  independently of `validate()` after every one of its thousands of commands,
  because a fuzzer that only asks the checker whether the checker is happy
  tests nothing when the checker stops looking.
- **`RemoveTrack` checks the lane it names is at the index it names.** A stale
  index used to delete a different lane and report success. `AddTrack` now
  refuses an out-of-range index for the same reason rather than clamping: a
  clamped insert produces a delete that cannot undo it.
- **Autosave exists.** `project/autosave.rs` writes the open document to
  `workspace::paths::autosave_file()` after every edit, import, save and undo,
  and `project_get` restores it on the first call after launch. It is not an
  undo step, and it is not the user's file — the path they last saved to is
  remembered in a sibling `autosave.path` so a restored session still knows
  where Ctrl+S goes. Writes go to one background thread through a single-slot
  mailbox, so a burst of edits costs one write, the newest wins, and no edit
  waits on the disk.

The same pass added the four keyframe commands the editing UI sends —
`AddKeyframe`, `RemoveKeyframe`, `MoveKeyframe`, `SetKeyframeEasing`. Two
things about them are load-bearing and not obvious:

- **A `KeyframeTrack` exists exactly while the property is animated.** The last
  keyframe of a property takes the track with it, because both sides read "is
  animated" as "a track exists". That makes undo harder, not easier, which is
  why `AnimatableProperty` is now `Ord` and a segment's tracks are kept in its
  declaration order: re-creating a dropped track has to put it back where it
  was, or undo rewrites the file.
- **A keyframe past the end of its clip is legal.** Trimming a tail produces
  them, the document keeps them so that undoing the trim brings the animation
  back, and refusing them would make the undo of an ordinary delete fail. The
  only positional rule is that a time is not negative; `validate()` reports the
  rest as a warning.

Two things found along the way that were worse than the list said:

- **Trimming was as capable of breaking the speed invariant as `SetSpeed`
  was**, and the fuzzer generates trims eighteen times more often. A trim that
  scales one range and not the other is now refused with a message naming the
  arithmetic, and the frontend already computes it correctly
  (`Timeline.tsx` multiplies the drag delta by the speed), so nothing that
  works today starts failing.
- **`split_at` truncated where it should have rounded**, so cutting a clip at
  an odd speed lost up to a microsecond of source at every cut, compounding
  with each split.

## Text rasterisation, measured

Measured 2026-07-26, `cargo run --release --example text_bench`. The engine is
`src-tauri/src/modules/text/`: parley + fontique + harfrust + skrifa for
shaping, zeno for scan conversion, everything above that ours. Full working, the
traps, and the things it cannot do are in `docs/research/text-rendering.md`.

**The machine was at load 40 on 12 cores while several agents built**, and the
scheduler dominates: the same case measured 4.7 ms and 15.1 ms minutes apart.
The benchmark reports the **minimum of 25 runs**, which is the closest thing to
an uncontended number available here. Treat these as an upper bound and
re-measure on a quiet machine before quoting them.

| Milliseconds, minimum of 25 | 1920×1080 | 1080×1920 |
|---|---:|---:|
| Short title, 26 glyphs | **5.8** | 6.5 |
| Short title + 4 px outline | 19.9 | 20.9 |
| Short title + outline + shadow + background box | 29.3 | 32.7 |
| Paragraph, 196 glyphs | 39.1 | 25.9 |
| **Any of them, warm cache** | **0.0005** | 0.0005 |

Three things worth carrying forward:

- **Shaping is 0.1 ms and is not the problem.** All of parley, harfrust and
  fontique — bidi, ligatures, Arabic joining, CJK line breaking — costs 0.1 ms
  for the title and 0.2 ms for the 196-glyph paragraph, at every size. Every
  other millisecond above is our own rasterisation.
- **The cache is the whole story for playback.** A title does not change between
  frames, so a warm frame costs a hash and an `Arc` clone. The cold cost is paid
  once per edit, and 30 ms of that is not felt in an editor.
- **The outline costs four times the text.** Scan-converting the stroke is what
  an outlined title spends its time on, and both obvious levers were the wrong
  way round: `kurbo::stroke` — which `rust-crate-survey.md` §4 recommends — was
  **1.8× slower** than letting zeno stroke during scan conversion, and
  `Join::Miter` was **4× slower** than `Join::Round` (54–62 ms against 15 ms).
  The next idea is to dilate the fill mask rather than to stroke faster.

## Not built yet

Keyframe editing in the UI, and audio waveforms on the timeline.

**Text rasterisation is built and titles are not** — `modules/text/` turns a
`TextMaterial` into an RGBA layer with system font fallback, real shaping
(ligatures, kerning, CJK, Arabic, Hebrew, colour emoji), outline, drop shadow
and background box, measured above and covered by 30 tests. What is missing is
the twenty lines in `media/provider.rs` that call it: `MaterialSource::Text`
still answers `Ok(None)`, so a text segment composites as nothing. That module
was owned by other work while this landed, so the exact patch is at the bottom
of `docs/research/text-rendering.md` rather than left half-applied. There is
also no UI yet for creating or editing a title.

**Transitions are built except for the compositor** —
`src-tauri/src/modules/transitions/`, and the data model, edit commands,
validation and five WGSL shaders are all in and tested. What is missing is the
thirty lines in `render/compositor.rs` that draw a transition instead of a
single quad; that module was owned by other work while this landed, so the exact
patch is written out at the bottom of `docs/architecture/transitions.md` rather
than left half-applied. Until it is applied a transition is stored, undone,
retimed and validated correctly and renders as nothing — the cut plays as a hard
cut.

Two things in that document are worth reading before touching either
`project/document.rs` or `render/`: a transition is centred on the cut and
**neither clip moves** (overlap would break the no-overlap track invariant and
shift the whole timeline on every duration change), and it hangs off the
**incoming** clip rather than the outgoing one, which is the opposite of CapCut
and reduces the cost of splitting a clip under a transition to one line in
`split_at`.

**Proxy media is built** — `src-tauri/src/modules/proxy/`, decision 0003 — but
two seams outside that module are still open and it does nothing until they are
closed: `media/provider.rs` needs the `from_project_for_preview` constructor
(the exact patch is in the decision document), and `preview/commands.rs` needs
to fold `proxy::ProxyQueue::shared().generation()` into the fingerprint its
provider cache is keyed on, or a proxy finishing will not be picked up until the
next edit.

The effect runtime is partly built — see "The effect runtime (Phase 3),
measured" above. It loads packages, compiles their shaders and runs their Lua,
and it cannot open a package that ships its assets in the binary encoding,
which is nearly all of them.

## Traps that have already cost time

- **`pnpm tauri dev` builds Rust unoptimized.** The preview's JPEG encoder
  needed ~130 ms per frame at `opt-level = 0` and ~7 ms in release, which made
  playback look fundamentally broken when it was merely unoptimized. `Cargo.toml`
  now sets `opt-level = 1` for our crate and `3` for dependencies, but the
  release build is still the one to judge by. Less of a cliff now that the
  encode is on the GPU and libjpeg-turbo is a C library compiled optimized
  either way — but compositing and the colour conversion are still ours.
- **`cargo check`/`build` need `-j 4`.** Full parallelism gets rustc OOM-killed
  on this machine while compiling wgpu and the Tauri macro crates.
- **A frame request must never fail.** WebKitGTK tears down its web process
  under a stream of failed resource loads — an observed crash, not a theory. The
  protocol handler answers with a neighbouring frame (within two frames) or a
  204, never a 404.
- **Never treat a webview frame request as a seek.** Doing so made the decoder
  jump ahead, which forced the next playback frame to seek backwards, which made
  the renderer fall further behind. Single frames took 13.5 seconds. See the
  comment on `PreviewServer::request_frame`.
- **`ffmpeg-next` holds non-atomic `Rc` in both the format and codec contexts.**
  `VideoDecoder` is `Send` only because all handles move together and access is
  serialised by the provider's mutex. That mutex is load-bearing; read the
  safety comment before touching it.
- **`avcodec_find_decoder` does not return the decoder that can drive the GPU.**
  On an ordinary Ubuntu build, `avcodec_find_decoder(AV_CODEC_ID_AV1)` returns
  **`libdav1d`**, which has no hardware support at all. Attaching a VAAPI device
  to it does nothing observable: `get_format` is never offered
  `AV_PIX_FMT_VAAPI`, every frame decodes on the CPU, and the code believes it is
  on the GPU. The native `av1` decoder is the one that works, and FFmpeg's own
  CLI says so out loud — *"Selecting decoder 'av1' because of requested hwaccel
  method vaapi"*. `media/hwdecode.rs::hardware_decoder` walks `av_codec_iterate`
  and picks by `avcodec_get_hw_config` rather than by name. The same shape of
  trap exists for VP9 (`libvpx-vp9` versus `vp9`). Assume it for every codec.
- **A hardware *decoder* being in the FFmpeg build means nothing either**, for
  the same reason as the encoder below. `media/hwdecode.rs::capabilities` probes
  by actually decoding one embedded 320×240 frame per codec — the bitstreams are
  in `media/probe_streams/`, 3.3 KB in total, and are fed to the decoder
  directly so no demuxer or temporary file is involved. Result cached for the
  process.
- **A hardware encoder being in the FFmpeg build means nothing.** `av1_vaapi` is
  in every modern build, and this chip decodes AV1 and cannot encode it —
  `vainfo` lists `VAProfileAV1Profile0` for `VAEntrypointVLD` only. `h264_qsv`
  is in the build too and QSV cannot open a device here at all. So
  `export/hwaccel.rs` answers "does it work" by opening the device and encoding
  a 320×240 frame, not by looking. The result is cached for the process; the
  probe costs about a tenth of a second per encoder.
- **VAAPI rate-control modes are the driver's, not FFmpeg's.** `rc_mode=CQP`
  opens fine on the Intel iHD driver and fails on several AMD ones, and there is
  no way to ask which are supported short of trying. `encoder.rs` walks a ladder
  — constant quality, then VBR, then CBR, then the driver's default — and opens
  with the first rung that works. A bitrate has to come with the fallback rungs,
  because the user's request was a CRF and a driver that refused constant
  quality needs a number.
- **Do not set `bit_rate` on a VAAPI context in CQP mode.** It is treated as a
  target and the QP is ignored, which is how a hardware export comes out at a
  bitrate nobody asked for. The ladder's first rung deliberately leaves it zero.

## The effect runtime (Phase 3), measured

Built 2026-07-26. `src-tauri/src/modules/effects/`, written up in
`docs/research/effect-runtime.md`.

**An effect authored in their format renders.** 45 tests in
`modules/effects/`, including a two-pass chain on the real GPU that samples
`share://input.texture`, ping-pongs through a `.rt` target, and has its uniforms
driven from a Lua script through the `Amaz` API. The fixture is
`modules/effects/fixtures/tint/` and it was written here.

**The shader pipeline works, and it was supposed to be the risk.**
`rust-crate-survey.md` §6b said naga's GLSL frontend rejects every shader in
the corpus and proposed a four-step replacement. That pipeline — our ES1→450
rewriter, then glslang, then `spirv-webgpu-transform`, then naga spv-in —
compiles **24 of 24** real corpus shaders to validated WGSL, including a
`sampler2D` passed as a function parameter and `gl_FragData[]` multi-output
shaders. The survey's estimate of "an afternoon" for the conformance run was
right.

**What actually blocks a downloaded effect is the binary asset container.**
Structural assets (`.xshader`, `.material`, `.rt`, `.scene`, `.mesh`) ship in
two interchangeable encodings and only the YAML twin is read here. That twin is
about **2% of shipped assets** — 4 of 263 `.material`, 4 of 213 `.xshader`. So
an effect authored in the format renders (there is one in
`modules/effects/fixtures/tint/`, and a test that produces pixels from it), and
every package in the surveyed cache is refused at its first `.xshader` with a
message saying exactly why. Decoding that container is the next piece of work
and it has an oracle for every field, which is the strongest position to do it
from.

Traps found building it, both of which produce a *compile error* rather than a
wrong picture — which is the redeeming property of this whole approach:

- **`sample` is an ordinary identifier in GLSL ES 1.0 and a keyword in 450
  core.** A Sobel filter in the corpus declares `vec3 sample;`. There is a
  family of these: `layout`, `buffer`, `shared`, `uint`, `smooth`, `centroid`.
- **A shader may overload a builtin, and renaming it breaks the builtin's
  callers.** `LumiGrain` defines its own `float mix(float, float, float)` and
  calls the builtin `mix(vec3, vec3, float)` in the same file. The reserved-word
  rename therefore has to skip declarations followed by `(`: a function
  overloads, only a variable shadows.

## Prior work, and what it is good for

- `~/git/x/capcut-renderer` — 18k lines of Rust reverse-engineering CapCut's
  effect format (wgpu + mlua + a GLSL-ES compiler). The source for Phase 3.
- `~/git/x/python_renderer_specs/` — effect format, Lua API, shader and render
  graph specifications from that work.
- `~/git/chukcut-rust` — the abandoned egui attempt. Useful only for the CapCut
  screenshots in `screenhsots orignal cpacut/`.
- `~/capcut-libs/` — CapCut Web JavaScript bundles.
- `ssh 10.11.12.79` — Windows machine with CapCut 9.0.0.3858 installed, for
  further reverse engineering. Read-only; drive it with
  `ssh 10.11.12.79 'powershell -NoProfile -Command "..."'`.

## The research

The documents under `docs/research/` were produced by dedicated agents and are
worth reading before making an architectural decision:

- `media-stack-options.md` — GStreamer/GES vs MLT vs building our own. Verdict:
  keep the custom stack; the frameworks give away the timeline we already have
  and withhold the hardware decode and proxy machinery we lack.
- `rust-crate-survey.md` — per-subsystem crate recommendations, with maintenance
  signals. Notably: hardware encode is *not* blocked by our FFmpeg version, and
  naga cannot consume the GLSL ES dialect CapCut ships. Its §2 was right: the
  wrapper took a day and no new dependency.
- `hardware-decode.md` — the VAAPI decode path, what this chip decodes and what
  a decoded frame costs three different ways. Read it before touching
  `media/` or before wiring the compositor to a decoded surface: it carries the
  measurements above, the `avcodec_find_decoder` trap, the evidence that a
  two-plane NV12 DMA-BUF import works on this driver (and therefore that a VAAPI
  VPP pass is *not* needed), and the exact patch `render/` needs.
- `zero-copy-encode.md` — written straight after building the VAAPI encoder,
  about the copy that is still in the path. It corrects an assumption the crate
  survey leaves implicit: the survey researched DMA-BUF *import* for decode, and
  encode needs *export*, which wgpu does not offer at all. Read it before
  starting step 2 of decision 0002.
- `vaapi-jpeg-preview.md` — hardware JPEG for the preview frame server. Read it
  before touching the preview encode path, and especially before using swscale
  for a colour conversion or trusting a YUV routine's default range. Its real
  conclusion is that the encoder is no longer where the time goes.
- `draft-format.md` — a full specification of CapCut's project format, with
  units and coordinate conventions established by evidence.
- `effect-runtime.md` — what happened when the effect format was actually run:
  the shader pipeline's conformance result on real shaders, the two GLSL
  dialect traps, the corrections to the crate survey's §6b, and the exact
  compositor patch the effect chain needs. Read it before touching
  `modules/effects/` or before estimating Phase 3.
- `effect-package-format.md`, `engine-symbols.md`, `ui-inventory.md` — how
  CapCut is built.
