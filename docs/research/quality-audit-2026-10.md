# Quality audit, October 2026

This audit looks for what must change for quality. It does not propose new
features. It uses four lenses: code health, robustness, UX and performance.
Each finding has evidence (a `file:line` or a measurement), the reason it
matters, a proposed fix and an effort (S: up to one day for one agent, M: two
to four days, L: more). The last section cuts the fixes into agent-sized work
packages.

Branch `agent/research-audit`, from master `bf5f36b`. Date 2026-10-05.

## How the audit was done

- **Code.** Static reading of all four crates (262 k lines of Rust). Three
  read-only sub-audits covered the bake caches, locks and threads, and the
  command layer with `unsafe` and panics. Spot checks confirmed their
  important claims. Scripts for dead code and function length are in
  `_scratch/audit/` of the worktree (not committed).
- **Robustness.** Generated media (VFR, rotation, odd sizes, 10-bit, no
  audio, audio only, 3 hours, truncated, empty, random bytes, 12000×8000 PNG)
  and 13 damaged project files, through the release CLI: `import --append`,
  `info`, `validate`, `render-frame`, `export --from 0 --to 2`.
- **UX.** The release app on private Xvfb displays `:217` (1366×768) and
  `:218` (2560×1440), lavapipe, `CHUKCUT_FILE_DIALOG=builtin`, no session
  bus, `HOME` and `XDG_*` under `_scratch`, ALSA on a null device. Space was
  never pressed.
- **Performance.** Build timings (`cargo build --release --timings -j 3`),
  start-up and project-open times, a 200-clip project, the bench suite.

**Machine load.** Other agents built in parallel. The load average was 4 to
24 during the audit (19.1 at the end of the release build). Treat every time
in this file as an upper bound. Do not quote a time from this file as a
regression baseline.

## Ranked findings

| # | Finding | Lens | Impact | Effort |
|---|---|---|---|---|
| 1 | The export mixes the whole timeline's audio into one buffer | Robustness | High | M |
| 2 | A panic is not contained and is not logged | Robustness | High | M |
| 3 | Background jobs outlive their project and write into the next one | Code health | High | M |
| 4 | About 9 400 lines of the webview preview are dead, and the bench measures them | Code health, Perf | High | M |
| 5 | Open decoders and several caches have no bound | Robustness, Perf | High | M |
| 6 | Locks are held across IO, and the UI thread waits on the ML worker | Code health, UX | Medium-high | M |
| 7 | Five bake caches repeat about 1 100 lines, and the cache limit misses them | Code health | Medium-high | M |
| 8 | A project opens, renders and exports without validation | Robustness | Medium | S-M |
| 9 | Errors in the app are shown truncated, one at a time | UX | Medium | S-M |
| 10 | The first clip can set an odd project frame rate (1 fps) | UX | Medium | S |
| 11 | Accessibility: low contrast tokens, no UI scale, truncated tab labels | UX | Medium | M |
| 12 | Release build time: 16 min, two LTO passes, duplicate GPU stacks | Perf (build) | Medium | S-M |
| 13 | Very large files and functions | Code health | Medium | L |
| 14 | The reachability allowlist hides commands that nothing calls | Code health | Medium | S |
| 15 | Command layer: names, raw error strings, an edit outside `EditCommand` | Code health | Medium | M |
| 16 | Panics on user text (two fixed here) | Robustness | Medium | S |
| 17 | `unsafe`: one safe function can write out of bounds | Code health | Low-medium | S |
| 18 | Thumbnail decodes block the rayon pool | Perf | Low-medium | S |
| 19 | Model downloads cannot resume | Robustness | Low-medium | S-M |
| 20 | Stale documentation and comments (Tauri, webview, `src-tauri`) | Code health | Low | S |
| 21 | Small UX items | UX | Low | S |

### 1. The export mixes the whole timeline's audio into one buffer

**Evidence.**
- `AudioMixer::new` allocates `frames_for(duration) * channels` floats for the
  project's whole duration (`crates/engine/src/modules/export/audio.rs:104`).
  `mix_into` (`export/audio.rs:226`) uses `project.duration()`, not the export
  range.
- Measured: a project with one 3-hour clip (160×90, 1 fps). `chukcut-cli
  export … --from 0 --to 2` reached 6.6 GB RSS at "Mixing audio (0%)". The
  memory guard killed it after 14 s (`memguard.log`, pid 2262347). A 2-second
  export needs about 0.8 MB of audio.
- A clip at a very late timeline position (hand-edited file, start
  9·10¹⁸ µs) makes the export abort: "memory allocation of
  3456000000001152000 bytes failed", SIGABRT, core dump, no user message.
- Each source is also decoded in full before it is mixed.

**Why it matters.** Podcasts, lectures and long recordings are normal input.
At 48 kHz stereo f32, one hour of timeline costs 1.4 GB before any decode.
A 3-hour project cannot export on a 16 GB machine. A short range export pays
for the whole timeline.

**Fix.** Mix in blocks, as the preview's `TimelineMixer` already does
(`modules/audio/mod.rs`, "fill thread"). Feed the encoder block by block for
the export range only. Keep the whole-buffer path only for effects that need
the whole clip (loudness target), and run those on the range. Add a test with
a 1-hour generated silent track that checks the peak memory.

**Effort.** M.

### 2. A panic is not contained and is not logged

**Evidence.**
- No `std::panic::set_hook` exists in the app, the CLI or the engine. The log
  file (`modules/workspace/logging.rs`) receives `tracing` events only. A
  panic goes to stderr, which is lost when the app starts from a launcher.
- Only analysis jobs catch panics (`modules/analysis/jobs.rs:163`). The
  others do not:
  - Player thread (`modules/preview/player.rs:217`): the preview freezes on
    its last frame. The UI reads only the "no GPU" failure
    (`crates/app/src/editor/preview.rs:110`), so it shows no message.
  - Export thread (`modules/export/commands.rs:173-205`): `end_job` and the
    final message do not run. The dialog stays at its last percentage.
  - Export queue worker (`modules/export/queue.rs:619`): the `worker` flag
    stays `true` (`queue.rs:277`, `:404-409`), so the queue never starts
    again until restart.
  - Matting, enhance, tracking, flow, proxy and autosave threads.
  - The MCP server (`crates/cli/src/mcp.rs`): one bad request ends the
    session.

**Why it matters.** A bug in one effect or one file becomes a frozen
preview, a stuck export or a dead queue, and nobody can find the cause.

**Fix.**
1. A panic hook in `lifecycle` that writes the message, the thread name and
   a backtrace to the log file, then calls the previous hook.
2. A `spawn_job` helper that wraps `catch_unwind` and reports a failed
   status. Use it for every job thread.
3. Mark the queue item failed and clear the `worker` flag on a panic.
4. Catch per request in the MCP loop.
5. Show "The preview stopped: <reason>. See the log." in the player.

**Effort.** M.

### 3. Background jobs outlive their project and write into the next one

**Evidence.**
- The process has one `AppState` (`crates/app/src/main.rs:27`).
  `project_close` (`modules/project/commands.rs:638`) cancels no job.
  `Editor::drop` (`crates/app/src/editor/mod.rs:703`) stops only the audio.
- The tracking thread (`modules/tracking/commands.rs:127`) commits into
  whatever project is open when it ends (`commit`, `:269`).
- Auto reframe applies a `ConfigureCommand` to the current project
  (`modules/analysis/commands.rs:769`). If the user opened another project,
  that project's canvas changes.
- Transcription (`crates/app/src/editor/captions/mod.rs:263`) runs on for
  minutes after its editor closed.
- Job records are never removed in the app: flow (`speed/flow/jobs.rs:134`)
  and enhance (`enhance/jobs.rs:134`) remove them only in `wait()`.
  `analysis_forget` is never called. The export queue gets one listener per
  editor that is never removed (`crates/app/src/editor/export/queue.rs:263`,
  `modules/export/queue.rs:551`).

**Why it matters.** A finished job can change the wrong document and add an
undo step the user did not make. This is silent data damage.

**Fix.** Give each opened project a generation number. Every job captures
it, and every commit refuses a different generation. `project_close` cancels
every job registry. Remove job records when their final status is read.
Keep the queue subscription id and unsubscribe in `Drop`.

**Effort.** M.

### 4. About 9 400 lines of the webview preview are dead, and the bench measures them

**Evidence.**
- The app uses only `preview::player` and `preview::clock`
  (`crates/app/src/player.rs:25`, `editor/mod.rs:17`). `clock` uses one
  function of `session` (`sane_fps`).
- These files serve JPEG frames over HTTP to a webview that no longer
  exists: `preview/server.rs` (2 988 lines), `stats.rs` (1 607), `vaapi.rs`
  (941), `encoder.rs` (828), `zerocopy.rs` (801), `vasurface.rs` (668),
  `session.rs` (553), `ladder.rs` (409), `cache.rs` (393), `probe.rs` (266),
  `commands.rs`. Outside the module, only benches, five examples and one test
  helper (`render/nv12.rs:1059`) use them.
- `preview/mod.rs:1-60` still describes `invoke()` and a webview.
- The reachability allowlist calls `preview_start` "the app's live preview
  server" (`crates/cli/tests/reachability.rs:86-92`). The app does not call
  it.
- The bench group "preview" times this JPEG path
  (`crates/engine/benches/bench_preview.rs`). The STATUS section "The
  benchmark suite" tells the reader to `cd src-tauri`, and its baseline
  (`baseline-4da5cff.json`) is from before the GPUI app.
- The path the app uses (shared frames, decision 0027) is not in the suite.
  `examples/player_bench.rs` measures the readback fallback, outside the
  suite.
- On the RTX 3060 the suite reports "Hardware decode: none". Its decode
  group knows only VAAPI, so NVDEC, the decoder this machine uses, has no
  row.
- The export group names every hardware row "VAAPI tier 1/2/3"
  (`benches/bench_export.rs:172-200`), also when the encoder is NVENC. The
  "tier 3: zero-copy" row then logs "a DMA-BUF can only be given to a
  hardware encoder … falling back to a readback", so it measures the
  readback under a zero-copy label.

**Why it matters.** Dead code costs build time, review time and clippy
time, and it looks like a live path. The bench reports a "preview frame"
number that the product does not have. Regressions in the real preview are
not measured.

**Fix.** Delete the server and its JPEG stack. Move `sane_fps` into `clock`.
Remove the "preview" bench group and add a group for `FramePlayer` with the
shared frame and the readback fallback (the core of
`examples/player_bench.rs`). Check whether `turbojpeg` is still needed: the
enhance and flow caches write JPEG with it. Rewrite the STATUS bench section
and take a new baseline on a quiet machine.

**Effort.** M.

### 5. Open decoders and several caches have no bound

**Evidence.**
- `MediaSourceProvider` keeps one decoder and its textures per rendered
  material (`modules/media/provider.rs:290-294`). Only `clear()` (`:435`)
  removes them. The player rebuilds the provider only when the set of
  materials changes (`preview/player.rs:610`). Scrubbing over 100 clips keeps
  100 decoders open; each VAAPI decoder holds its surface pool plus six extra
  frames (`media/hwdecode.rs:86`).
- Measured: the 200-clip project used 621 MB RSS and 144 threads 40 s after
  it opened, with no playback.
- Animated stickers: 32 entries of up to 384 MB each
  (`modules/animated/mod.rs:150`, `animated/frames.rs:18`), so about 12 GB at
  worst. A full cache clears everything (`mod.rs:181`). The inspector then
  decodes the GIF again on the UI thread
  (`crates/app/src/editor/inspector/clip.rs:460`).
- Timeline filmstrips: three strips per material, up to 256 tiles each, and
  no limit over all materials (`crates/app/src/editor/timeline/media_cache.rs:30-68`).
- The LUT cache never evicts (`modules/render/lut.rs:364-386`,
  `fx/tiles.rs:29`).
- The render-side matte, flow and enhance listings never evict
  (`render/background.rs:56-61`). Each Select-object prompt adds a key.

**Why it matters.** Long sessions grow in RAM and VRAM until the driver or
the memory guard stops them. VRAM is small on the laptops that come first
(Intel).

**Fix.** An LRU with a count limit (8 to 16) for decoders and textures in
`MediaSourceProvider`. A byte-budget LRU for stickers, filmstrips and LUTs.
A `still()` for stickers that does not fill the cache. Tests that open N + 1
entries and check that the oldest one goes.

**Effort.** M.

### 6. Locks are held across IO, and the UI thread waits on the ML worker

The project lock itself is clean: every command clones and drops it. The
problems are in other locks.

**Evidence.**
- `ml::worker::client()` holds `SUPERVISOR` while it starts the worker
  process and waits for its hello, up to 10 s
  (`modules/ml/worker.rs:446-488`, `:45`, `:266`). A settings change runs on
  the UI thread (`crates/app/src/editor/settings.rs:197` →
  `workspace_settings_set` → `set_acceleration` → `running()`,
  `worker.rs:186`, `:494`) and waits for that lock. `set_acceleration` also
  reads the settings file on the UI thread (`worker.rs:178`).
- The render listing caches hold their lock across `SourceKey::of` (it reads
  the media file's head and tail) and `read_dir`, every 500 ms while frames
  are missing (`render/background.rs:111-131`, `render/flow.rs:90`,
  `render/enhance.rs:95`). Export workers share the compositor.
- The LUT cache holds its lock across `read_to_string`, the parse and the GPU
  upload (`render/lut.rs:364-386`). Face tracks: `render/faces.rs:46-75`.
- `project_save` passes `&state.history.read()` to `freeze::sweep_unused`,
  which reads project files and deletes files (`project/commands.rs:129`,
  `timeline/freeze.rs:192-217`). Undo waits for it.
- The proxy index writes `index.json` again on every lookup, under its lock
  (`proxy/cache.rs:259-283`, `resolve_all` at `:503`).

**Why it matters.** The UI freezes for seconds. Export and preview threads
wait on each other's disk reads.

**Fix.** Start the worker outside the lock with a "starting" slot. Move
`workspace_settings_set` off the UI thread. Load caches outside the lock,
then insert. Collect the paths under the history lock, then drop it before
the IO. Update the proxy `last_used` in memory and save once per batch.

**Effort.** M.

### 7. Five bake caches repeat about 1 100 lines, and the cache limit misses them

**Evidence.** Mattes, optical flow, enhance (with remove object), face
landmarks and body tracks each have their own copy of:
- a frame-directory store: `matting/cache.rs:141-237`,
  `enhance/cache.rs:59-160`, `speed/flow/mod.rs:131-220` (the JPEG writer is
  the same code twice);
- a job registry with start, status, wait, cancel, ensure and queue-missing:
  `matting/commands.rs:420-640`, `speed/flow/jobs.rs:42-320`,
  `enhance/jobs.rs:43-317`;
- a frame grid, and a render-side listing cache: `render/background.rs:60-135`,
  `render/flow.rs:45-115`, `render/enhance.rs:49-125`;
- a track file: `landmarks/track.rs` and `body/track.rs` differ only in names.

The copies have drifted, and some differences are bugs:
- `trim.rs:274-280` treats every `*.part` file as "being written". It never
  deletes one but counts it, so files from a crash fill the limit for ever.
- No bake starts a trim. Hours of baking can go far past the limit.
- Trim protects the open project's mattes, flow and enhance frames, but not
  its landmark, body or voice files, which cost a model run to make again.
- "Clear cache" stops only proxies (`crates/app/src/editor/settings.rs:233-236`).
  Running bakes create their directories again. Flow has no clear and no
  cache info at all.
- The renderer keys its listing by the path string
  (`render/background.rs:112-121`). A file replaced in place keeps its old
  mattes for the session.
- Voice keys use path, size and mtime; a failed `metadata()` gives the key
  `(0, 0)` (`voice/denoise.rs:50-62`).
- Two clips of one file can analyse faces at the same time. Both run
  `merge_into` on one file without a lock, and each flush rewrites the whole
  file every 60 frames (`landmarks/analyse.rs:18`, `track.rs:158-171`).
- The enhance grid snaps to frames (`enhance/bake.rs:108-117`); the matte grid
  does not (`matting/bake.rs:100-105`).
- An export cannot cancel face or body analysis
  (`landmarks/commands.rs:181`, `body/commands.rs:125`).

**Why it matters.** Each new AI feature copies a fifth or sixth version.
Disk use goes past the user's limit. A fix in one copy does not reach the
others.

**Fix.** One `FrameStore` (key, directory, list, best, atomic write, part
sweep), one `FrameGrid`, one `BakeRegistry<S: BakeSpec>` with a normalised
0..1 progress, one `TrackFile` with a lock per path, and a `DerivedCache`
list that trim, Settings and "Clear cache" loop over. Voice fits only
`DerivedCache` and the atomic write.

**Effort.** M (store, grid, registry, cache list); L with the render-side
listings and a common source key.

### 8. A project opens, renders and exports without validation

**Evidence.**
- `project_open` calls `migrate::load` but not `validate()`
  (`modules/project/commands.rs:86-107`). The migration warnings ("values
  reset to their defaults") go to the log only.
- `validate()` says "valid" for a canvas of 0×1080, a canvas of
  100000×100000, 100 000 fps, a clip that starts at 9·10¹⁸ µs and a source
  range far past the media's end (`_scratch/audit/broken.log`).
- The CLI's `render-frame` and `export` run on files that `validate` rejects:
  overlapping clips, speed 0 and fps 0 all exported (exit 0).
- `import_material` writes `canvas.width`, `canvas.height` and `fps` directly,
  outside `ConfigureCommand` (`project/commands.rs:309-316`). Undo cannot
  restore the canvas.

**Why it matters.** A damaged or hand-edited file gives late, confusing
failures (one abort, see finding 1). The canvas change on import breaks the
rule "mutations go through `EditCommand`".

**Fix.** Add the canvas and fps rules of `configure.rs:65-80` and time-range
bounds to `validate()`. Run it on open; show its warnings and the migration
warnings in the app. Make the CLI refuse to render a document with errors
(or warn with `--force`). Route the import's canvas change through
`ConfigureCommand` in the same undo step.

**Effort.** S-M.

### 9. Errors in the app are shown truncated, one at a time

**Evidence.** Starting the app with a random-bytes file, a truncated MP4 and
a good clip: the title bar showed only
"/mnt/data/git/chukcut-research-audit/_scratch/audit/media/garbage.mp…". The
reason was cut off. The second failure was not shown. The message went away
after the next action (`_scratch/audit/ux/import-errors.png`). The CLI text
for the same files is "cannot open <path>: Invalid data found when
processing input" with the path twice.

**Why it matters.** The user does not learn which file failed or why.

**Fix.** Put the reason first and the file name (not the path) second.
Collect import failures into one notice ("2 files could not be imported")
with a list. Map common FFmpeg errors to prose: "The file is damaged or
incomplete", "This is not a video, image or sound file".

**Effort.** S-M.

### 10. The first clip can set an odd project frame rate

**Evidence.** Importing a 1 fps clip into a new project gave "the canvas took
the clip's shape: 1920x1080 at 1 fps" (`_scratch/audit/robust.log`). A
17×9 clip gave a 1920×960 canvas.

**Why it matters.** Screen recordings, time-lapses and phone VFR files are
common. A 1 fps project makes every later clip stutter and every keyframe
coarse.

**Fix.** Adopt the frame rate only when it is a common rate (23.976 to 60).
Otherwise keep 30 and say so. Round odd canvas shapes to a known aspect.

**Effort.** S.

### 11. Accessibility: low contrast tokens, no UI scale, truncated tab labels

**Evidence.**
- WCAG contrast of the tokens (`crates/app/src/theme.rs`):
  `TEXT_MUTED` on `PANEL` 3.42:1, on `PANEL_RAISED` 3.01:1, on `OVERLAY`
  2.77:1. `TEXT_MUTED` is the 11 px caption, hint and tile-name colour (122
  uses). WCAG AA asks 4.5:1 for small text. The menu hover (`BORDER` on
  `OVERLAY`) is 1.15:1, so the hovered item is almost invisible.
- No setting scales the UI. The rem stays 16 px by design
  (`docs/design/language.md`, "The window's rem stays 16 px"). `Xft.dpi: 144`
  on X11 had no effect.
- At 1366×768 the asset tab rail shows "Me…", "Au…", "Stic…", "Cap…",
  "Effe…", "Tr…", "Filt…", "T…" (`_scratch/audit/ux/editor-1366.png`).
- The window always opens at 1600×960 (`crates/app/src/main.rs:52`). Size,
  position and maximised state are not kept.
- 14 hard-coded colours in views, against the design rule "never write a hex
  value in a view": for example `home.rs:554` uses `0xe5484d`, not `DANGER`
  (`#f2555a`).
- Keyboard: Tab moves focus on the home screen with a visible accent ring,
  and Escape closes the export dialog. Only 21 places in the app use a focus
  handle, so most panels have no keyboard path.

**Why it matters.** Small grey text is hard to read on laptop screens and for
older users. People with 1366×768 laptops cannot read the asset tabs.

**Fix.** Raise `TEXT_MUTED` to about `#868a94` (4.5:1 on `PANEL`) and make
the menu hover one more step. Add a UI scale setting (GPUI window scale or a
rem factor for the kit). Show icons only, with a tooltip, on the rail below
a width. Keep window bounds in settings. Replace the hard-coded colours.

**Effort.** M.

### 12. Release build time: 16 minutes, two LTO passes, duplicate GPU stacks

**Evidence.** `cargo build --release -p chukcut -p chukcut-cli -p
chukcut-ml-worker -j 3 --timings`, cold, load 4 rising to 19:
16 min 19 s wall, 2 135 s of unit time, 3.0 GB peak RSS.

| Unit | Time |
|---|---:|
| `chukcut` (bin, thin LTO) | 482.6 s |
| `chukcut-cli` (bin, thin LTO) | 239.3 s |
| `whisper-rs-sys` build script (whisper.cpp with CMake) | 88.1 s |
| `chukcut-engine` (lib) | 66.2 s |
| `turbojpeg-sys` build script (libjpeg-turbo with CMake, nasm) | 64.5 s |
| `gpui-base` | 49.1 s |
| `zstd-sys` build script | 46.0 s |
| `gpui-pre` | 45.3 s |
| `gpui-component` | 30.6 s |
| `gpui-pre-linux` | 29.6 s |
| `naga` 30 | 28.2 s |
| `x11rb-protocol` | 25.6 s |
| `wgpu-core` 29 | 20.9 s |
| `naga` 29 | 19.1 s |
| `chukcut-ml-worker` | 17.2 s |

- 57 crates are in the tree in two or more versions (`cargo tree -d`).
  The large ones: `wgpu`, `wgpu-core`, `wgpu-hal`, `naga` 29 (GPUI through
  `gpui-pre-wgpu`) and 30 (engine, `vello`): about 220 s of unit time for the
  family. `skrifa` three times (0.40 `cosmic-text`, 0.44 `parley`/`vello`,
  0.45 engine), `read-fonts` twice, `harfrust` twice, `png` twice.
- `image` is used with default features. That pulls `ravif` and `rav1e`, an
  AVIF encoder that nothing calls (about 15 s).
- The app and the CLI each run thin LTO over the whole engine: 722 s
  together.

**Why it matters.** Every agent pays this for every release check. The plan
allows four builds at once, so one release build blocks the machine for
longer than most fixes take.

**Fix.** A `[profile.release-check]` that inherits `release` with `lto =
false` and `codegen-units = 16`, for agents and QA. Keep thin LTO for the
packages. `image = { default-features = false, features = [the formats in
use] }`. Align `wgpu` with GPUI at the next `gpui-kit` bump, in its own
commit. Check if `whisper-rs-sys` can use a system `libwhisper` when present.

**Effort.** S for the profile and `image`; M for the `wgpu` alignment.

### 13. Very large files and functions

**Evidence.** Top 20 files by lines (tests included, test share in brackets):

| Lines | File |
|---:|---|
| 4 800 | `engine/src/modules/render/compositor.rs` (tests from 2 740) |
| 4 476 | `engine/src/modules/timeline/ops.rs` (tests from 2 277) |
| 4 259 | `app/src/editor/timeline.rs` (tests from 4 179) |
| 2 988 | `engine/src/modules/preview/server.rs` (dead, finding 4) |
| 2 504 | `engine/src/modules/project/document.rs` |
| 2 379 | `app/src/editor/inspector/masks.rs` |
| 2 250 | `engine/src/modules/export/job.rs` |
| 2 093 | `engine/src/modules/inspector/edit.rs` |
| 2 031 | `engine/src/modules/transitions/library/gl_table.rs` (data) |
| 1 736 | `ml-worker/src/main.rs` (no tests) |
| 1 665 | `engine/src/modules/export/encoder.rs` |
| 1 616 | `engine/src/modules/fx/render_tests.rs` (tests) |
| 1 607 | `engine/src/modules/preview/stats.rs` (dead) |
| 1 589 | `engine/src/modules/media/provider.rs` |
| 1 500 | `app/src/editor/export/dialog.rs` |
| 1 458 | `cli/src/ops/look.rs` |
| 1 437 | `app/src/editor/inspector/animation.rs` |
| 1 420 | `engine/src/modules/media/decoder.rs` |
| 1 415 | `engine/src/modules/export/presets.rs` |
| 1 388 | `engine/src/modules/effects/graph.rs` |

Longest functions: `Editor::render_lanes` 701 lines
(`app/src/editor/timeline.rs:2851`), `ops.rs` `apply` 594
(`timeline/ops.rs:545`), `Compositor::render_to_texture` 460
(`render/compositor.rs:1028`), export `render_settings` 404
(`app/src/editor/export/dialog.rs:713`), ML worker `handle` 382
(`ml-worker/src/main.rs:367`), `Compositor::with_config` 380, `render_clip`
376, `Project::validate` 331. In `app/src/editor/timeline.rs`, one `impl
Editor` block runs from line 502 to 4 092.

**Why it matters.** Parallel agents collide in these files (CLAUDE.md
describes such a night). Long functions hide state that the tests do not
reach.

**Fix.** Split `timeline.rs` into paint (lanes, clips, ruler), hit testing
and gestures. Split `ops.rs` `apply` into one function per command kind.
Split `render_to_texture` by pass. Give the ML worker's `handle` one function
per request. Do these as separate packages, one file each, with no behaviour
change.

**Effort.** L in total; M per file.

### 14. The reachability allowlist hides commands that nothing calls

**Evidence.**
- 22 allowlisted command functions have no caller in the app or the engine,
  although the reason often says "the app": `audiofx_render`,
  `audio_set_volume`, `audio_status`, `cancel_all_thumbnails` ("the app's
  quit path"), `effects_describe`, `enhance_running` and `speed_flow_running`
  ("the app's status line"), `export_queue_add`, `export_queue_wait_idle`,
  `inspector_set_grade_control`, `inspector_set_wheel`, `keymap_conflicts`,
  `preview_pause`, `preview_stop`, `preview_viewport`, `project_get`,
  `proxy_generation`, `proxy_queue_status`, `proxy_watch`,
  `workspace_recent_clear`, `workspace_recent_list`.
- The app goes around the command layer in places: it calls
  `media::waveform` and `media::thumbnail_strip`
  (`crates/app/src/editor/timeline/media_cache.rs:284`), not `media_waveform`
  and `media_thumbnails`; it calls `workspace_recent_entries`, not
  `workspace_recent_list` (`crates/app/src/editor/home.rs:123`).
- 16 public engine functions have no reference at all, for example
  `render::compositor::clear_nested` (`compositor.rs:808`),
  `proxy::cache::resolve_all` (`proxy/cache.rs:504`),
  `export::encoder::audio_frame_size` (`export/encoder.rs:390`),
  `workspace::paths::preview_dir` (`workspace/paths.rs:97`). 53 more are used
  only by tests.

**Why it matters.** "Everything is a command" is a core rule. Duplicate
commands drift, and dead ones look like features in the CLI's reasons.

**Fix.** Extend `reachability.rs`: an entry whose reason names the app must
have a caller in `crates/app/src`. Delete the 22 dead commands or wire them.
Make the app call the command functions. Delete the 16 dead functions.

**Effort.** S.

### 15. Command layer: names, raw error strings, an edit outside EditCommand

**Evidence.**
- 435 `pub fn` in 37 `commands.rs`; 307 return `Result<_, String>`.
- 33 do not follow `<module>_<verb>`. The real ones:
  `project::import_material` (`project/commands.rs:177`),
  `speech::transcribe_chunked` (`:115`), `template::load` (`:141`),
  `export::plan_for`, `estimate_sampled_for`, `cancel_all_exports`
  (`:244`, `:279`, `:521`), `compositing::key_probe_project`, `pick_from`,
  `media::cancel_all_thumbnails`, `proxy::cancel_all_proxies`,
  `ml::gpu_vendors`, `voice::isolation_missing` (a second copy of
  `voice_isolation_missing`, `voice/commands.rs:233-244`).
- Error prose: the save path passes the IO error without the file name
  (`project/commands.rs:18-26`: "Permission denied (os error 13)");
  `ml/commands.rs:447` shows `{other:?}` to the user; seven sites say
  "unknown segment {uuid}" (`speed/commands.rs:55`, `:175`, `:366`, `:441`,
  `fx`, `animated`, `compositing`).
- Commands that do IO but cannot report failure:
  `project_recovery_discard`, `workspace_settings_apply`,
  `template_open_project`, `export_queue_restore`.
- The import's canvas change outside `EditCommand` (finding 8).

**Fix.** Rename with the module prefix (keep the CLI names). Add the path to
every IO error. Replace debug output and ids in messages with prose ("the
clip is no longer on the timeline"). Return `Result` from the four IO
commands.

**Effort.** M.

### 16. Panics on user text (two fixed on this branch)

**Evidence and status.**
- **Fixed:** `captions/karaoke.rs` compared only the total byte length of the
  caption and its lower case. One growing and one shrinking capital ("İ" and
  "ẞ") kept the total. The word range then split a character. Slicing with
  it panics, on the UI thread, in a normal split (`captions/edit.rs:663`).
  Now the check is per character. Test:
  `a_growing_and_a_shrinking_capital_do_not_shift_the_ranges`.
- **Fixed:** the MCP resource URI decoder sliced `&s[i+1..i+3]`; "%aé" ended
  the MCP session. It now reads the bytes. Test:
  `a_percent_before_a_multibyte_character_is_kept_as_text`.
- **Open:** `crates/cli/src/ops/audiofx.rs:403`
  `Duration::from_secs_f32` panics on `--count-in inf`.
  `captions/srt.rs:179` overflows `i64` for absurd hour values (panic in
  debug, wrong times in release). `preview/commands.rs:57-69` uses a
  `std::sync::Mutex` across `from_project`, so one panic poisons it and
  every later call panics (this code goes with finding 4).

**Fix.** Validate finite, bounded values at the CLI edge; use
`checked_mul` in the SRT parser.

**Effort.** S.

### 17. unsafe: one safe function can write out of bounds

**Evidence.** 117 `unsafe` sites, almost all with a `SAFETY` comment.
Exceptions:
- `export/encoder.rs:1120` `palette_mut` is a safe function that builds a
  1 024-byte `from_raw_parts_mut` over `data[1]` without a check that the
  frame is PAL8. A YUV frame there writes out of bounds. Today the one caller
  passes PAL8.
- The bytemuck `Pod` impls (12) have "no padding" comments but no
  `size_of` asserts. `render/compositor.rs:103` says "every field a f32
  array", but the struct has `u32` fields.
- `unsafe impl Send for VideoDecoder` (`media/decoder.rs:222-260`) depends on
  `ffmpeg-next` internals; a crate bump can break it without a compile error.

**Fix.** Make `palette_mut` check the pixel format (or mark it `unsafe`).
Add `const _: () = assert!(size_of::<T>() == N)` for each `Pod` type. Put a
test next to the `Send` impl that pins the assumption, and a line in the
ffmpeg bump checklist.

**Effort.** S.

### 18. Thumbnail decodes block the rayon pool

**Evidence.** `thumbnail_stream` runs whole FFmpeg decode chunks on rayon
workers (`media/thumbnails.rs:241-245`, `render_chunk` at `:298`). CLAUDE.md:
"never block a rayon worker". The app loads two filmstrips at a time
(`media_cache.rs:37`).

**Fix.** A dedicated small thread pool for thumbnails and filmstrips.

**Effort.** S.

### 19. Model downloads cannot resume

**Evidence.** `ml/download.rs:288` fetches each pack in one GET with no
`Range` header. The TensorRT add-on is 3.0 GB. The read timeout is 600 s
(`cloud/http.rs:20`).

**Why it matters.** A dropped connection at 2.9 GB starts again from zero.

**Fix.** Keep the `.part` file and its hash state, and resume with `Range`
when the server sends `Accept-Ranges`. Verify the checksum at the end as now.

**Effort.** S-M.

### 20. Stale documentation and comments

**Evidence.** 30 mentions of Tauri, webview or `src-tauri` in
`docs/STATUS.md` and 81 in `.rs` files (`preview/server.rs` 14,
`media/thumbnails.rs` 7, `shell.rs` 3, `inspector/commands.rs` 3). The
engine's `Cargo.toml:65` says `cpal` lets "the device live in Tauri-managed
state". STATUS.md is 4 466 lines; its bench section is from the webview era.

**Fix.** Remove with finding 4. Move the old STATUS sections into
`docs/history/` and keep STATUS to the present state.

**Effort.** S.

### 21. Small UX items

- Details rows are fixed text: "Colour space: Rec.709 SDR" and "Proxy:
  Automatic, for footage too heavy to play"
  (`crates/app/src/editor/inspector/details.rs:146-153`), whatever the
  settings say.
- "Kept in place (1 files)" — **fixed** on this branch (test
  `one_file_is_singular`).
- On a first start the template posters stayed empty for more than 4 s
  (lavapipe), with no placeholder.
- The timeline zooms out to 35 minutes on a 13-minute project; clip borders
  disappear at that zoom.
- The batch numbers operations from 0 in errors ("operation 3" is the
  fourth). People count from 1.
- The CLI exports with libx264 unless `--hardware` is given; the app picks
  NVENC.
- The app's audio thread used 90 % of a core while paused with the null ALSA
  device. A real device may block; check that the output stream stops while
  paused.

## Robustness results through the CLI

`_scratch/audit/robust.log` and `broken.log`. Times include a cold Vulkan
start and are under load.

| Input | Import | render-frame | export 0–2 s |
|---|---|---|---|
| VFR MKV (30 then 12 fps) | ok, 30 fps project | ok, 4.3 s | ok |
| Rotation 90° MP4 | ok | ok, 1080×1920 | ok |
| 1001×777 4:4:4 | ok, 1392×1080 canvas | ok | ok |
| 17×9 | ok, 1920×960 canvas | ok | ok |
| 2×2 FFV1 | ok | ok | ok |
| HEVC 10-bit 1080p | ok | ok | ok |
| H.264 10-bit | ok | ok | ok |
| No audio | ok | ok | ok |
| MP3, 22 kHz mono | ok | ok | ok |
| 3 h, 1 fps | ok, **1 fps project** | ok | **killed at 6.6 GB** (finding 1) |
| Truncated MP4 | refused, raw FFmpeg text | — | — |
| Empty file, random bytes | refused, raw FFmpeg text | — | — |
| PNG 12000×8000 | ok | ok | ok |
| Truncated project JSON | refused with line and column | | |
| Future schema version | refused, clear prose | | |
| Overlap, speed 0, fps 0 | `validate` refuses | **renders** | **exports** |
| Canvas 0 wide | `validate` passes | renders | refused ("resolution must be larger than zero") |
| Canvas 100000² | `validate` passes | refused (texture limit) | refused |
| Clip at 9·10¹⁸ µs | `validate` passes | renders | **abort, core dump** |

No crash or hang came from odd media. Not tested: disk full (needs a small
loop-mounted file system), no GPU at all, no network for ML downloads (the
cached models were not moved away), real VAAPI hardware.

## Performance measurements

All under load 10 to 24 from other builds. Upper bounds only.

| What | Result |
|---|---|
| App start to mapped window, first run, empty settings | 1.02 s |
| App start with 3 media files | 1.02 s |
| App start with a 200-clip project (800 s, 40 sources) | 0.53 s to window; player still black at 2.5 s |
| After opening the 200-clip project, idle 40 s | 621 MB RSS, 144 threads |
| 70 zoom and scroll events on the 200-clip timeline (lavapipe) | 1.9 s main-thread CPU, about 27 ms per event |
| CLI `info` on the 200-clip project | 0.05–0.09 s, 56 MB |
| CLI `batch` that builds the 200-clip project | 0.28 s |
| CLI `render-frame`, one frame | 0.5–4.3 s (Vulkan start each call) |
| CLI export, 2 s of 1080×1920, libx264 | 8.3 s (7.2 fps) |

The bench suite result is in the appendix below. Missing measurements that need a
tool first: frame times of the GPUI window (scroll and zoom smoothness),
4K HEVC playback through NVDEC with the shared frame path, and playback with
a stacked effect chain. The bench has none of these (finding 4).

### Appendix: the bench suite under load

`cargo run --release -p chukcut-engine --bin chukcut-bench -- --all` refused:
"one-minute load average is 12.52, above the 4.00 threshold". With
`--force`: load 12.52 before and 22.45 after, 189 s, 87 rows, 29 skipped,
23 with a spread above 1.5. Indicative only; the full output is in
`_scratch/audit/bench.log` and `bench.json` of the worktree.

| Row | Median |
|---|---:|
| decode h264 1080p software, sequential | 39.9 ms |
| composite 10 layers, grade + LUT, GPU only | 7.46 ms |
| composite 10 layers, transform + crop + keyframes, GPU only | 2.79 ms |
| "preview-frame" 1920×1080 whole frame (dead JPEG path) | 21.3 ms |
| export software (libx264) | 14.1 fps |
| export "VAAPI tier 2" (really NVENC, GPU NV12) | 67.6 fps |
| export "VAAPI tier 3" (really NVENC, readback fallback) | 62.1 fps |
| project save, 500 segments | 0.90 ms |
| apply one edit on a 500-segment document | 4.9 µs |

## Work packages

Each package fits one agent and one worktree. The order is the proposed
order. Packages in the same row touch different files and can run at the same
time.

| Order | Package | Findings | Owns | Effort |
|---|---|---|---|---|
| 1 | **export-audio-stream**: mix the export in blocks over its range; memory test with a 1-hour track | 1 | `modules/export/audio.rs`, `export/job.rs` | M |
| 1 | **panic-containment**: log hook, `spawn_job` with `catch_unwind`, queue recovery, MCP per request, player message | 2, 16 (poisoned mutex) | `lifecycle.rs`, job spawn sites, `cli/src/mcp.rs`, `app/src/editor/preview.rs` | M |
| 1 | **document-guard**: rules in `validate()`, validate on open, show warnings, CLI refuses invalid input, import canvas through `ConfigureCommand`, common-fps adoption | 8, 10 | `modules/project/`, `cli/src/ops/` (render, export) | S-M |
| 2 | **job-generation**: project generation id, cancel on close, refuse stale commits, forget records, unsubscribe | 3 | job registries, `project/commands.rs`, app editor drop | M |
| 2 | **bounded-caches**: decoder LRU, sticker, filmstrip and LUT budgets, thumbnails off rayon | 5, 18 | `media/provider.rs`, `animated/`, `render/lut.rs`, `media/thumbnails.rs`, app `media_cache.rs` | M |
| 3 | **preview-legacy-removal**: delete the JPEG server, new player bench group (shared frame and readback), NVDEC decode rows, true encoder names in the export rows, STATUS bench rewrite, stale docs | 4, 20 | `modules/preview/` except player and clock, `benches/`, `examples/`, docs | M |
| 3 | **locks-off-ui**: ML supervisor starting slot, settings off UI thread, caches load outside locks, history lock in save, proxy index save | 6 | `ml/worker.rs`, `render/background.rs`, `render/flow.rs`, `render/enhance.rs`, `render/lut.rs`, `proxy/cache.rs`, `timeline/freeze.rs` | M |
| 4 | **bake-store**: `FrameStore`, `FrameGrid`, `BakeRegistry`, `TrackFile`, `DerivedCache` wired to trim and Clear cache, part sweep, trim after bakes | 7 | `matting/`, `enhance/`, `speed/flow/`, `landmarks/`, `body/`, `workspace/trim.rs` | M-L |
| 4 | **errors-and-a11y**: import error notice, FFmpeg error prose, contrast tokens, UI scale, rail labels, window bounds, hard-coded colours, Details rows | 9, 11, 21 | `crates/app/src/` (title bar, theme, assets rail, main.rs, details.rs), `media` error mapping | M |
| 5 | **build-diet**: `release-check` profile, `image` features, `wgpu` alignment at the next GPUI bump | 12 | `Cargo.toml`, crate manifests | S-M |
| 5 | **command-layer-tidy**: prefixes, error prose with paths, `Result` from IO commands, reachability rule for "the app", delete dead functions | 14, 15 | `modules/*/commands.rs`, `cli/tests/reachability.rs` | M |
| 5 | **hardening**: `palette_mut`, `Pod` size asserts, `Send` pin test, CLI finite bounds, SRT `checked_mul`, download resume | 16, 17, 19 | `export/encoder.rs`, `render/`, `media/decoder.rs`, `cli/src/ops/audiofx.rs`, `captions/srt.rs`, `ml/download.rs` | S-M |
| 6 | **split-timeline-view**, **split-ops-apply**, **split-compositor**, **split-ml-worker** (one each, no behaviour change) | 13 | one file each | M each |

Start packages 1 to 3 before the others: they remove data loss, silent
failures and the largest dead weight. Run the split packages last and alone,
because every other package touches those files.

## Fixed on this branch

- `captions/karaoke.rs`: word ranges split a character when one capital
  grows and one shrinks on lower-casing; the UI thread panicked on the slice.
- `cli/src/mcp.rs`: a percent sign before a multi-byte character in a
  resource URI ended the MCP session.
- `app/src/editor/inspector/details.rs`: "Kept in place (1 files)".

Each fix has a test next to it.
