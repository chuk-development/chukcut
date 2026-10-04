# Build-out plan (2026-10-03 → Monday 02:00)

The owner's brief: build chukcut out front to back into a real editor —
basic and advanced (colour grading, LUTs, everything editors have), with its
own design language close to CapCut's layout. The lead session manages and
merges; subagents implement. Many small commits. CapCut effect-package
support is *not* a priority.

This file is the lead's memory across context compaction: keep the status
column current.

## How agents work (put this in every agent prompt)

- **Do not use the Agent tool's `isolation: worktree`** — it creates the
  worktree from the session's directory (`~/git/x`), which is the wrong repo.
  Each agent creates its own: `git -C /mnt/data/git/chukcut worktree add
  /mnt/data/git/chukcut-<name> -b agent/<name> master` and works only there.
- Read `CLAUDE.md`, `docs/reference/capcut/README.md`, this file.
- Screenshots of CapCut: `/mnt/data/git/chukcut/docs/reference/capcut/*.png`
  (local only, gitignored — never commit them).
- Test visually only on a private display:
  `Xvfb :NN -screen 0 1920x1080x24 &` and
  `DISPLAY=:NN VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json ./target/debug/chukcut …`;
  capture with `DISPLAY=:NN import -window root shot.png`; drive with
  `DISPLAY=:NN xdotool …`. Never touch the owner's display `:1`, never press
  Space (audio goes to the real speakers). Pick a unique NN per agent.
- GPU tests: run them on the real GPU AND on lavapipe (`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`); results differ (NVIDIA rounds alpha in the blender).
- All Claude processes run in `claude.slice` (70% CPU, throttled above 70% RAM; `~/.config/systemd/user/claude.slice`). At most 4 agents build at once, each with `-j 3`.
- Build with `memguard-allow 12G cargo build -p chukcut -j 3`; gates:
  `cargo fmt --all`, `cargo clippy -p chukcut`, `cargo test -p chukcut`,
  `cargo test -p chukcut-engine -j 4 --lib` (plus integration tests when the
  engine changed).
- Commit on the agent branch after every green step; imperative messages, no
  trailers, no session links. Do not push, do not merge — the lead does.
- No `/tmp`, no `.env` reads, no global input.
- Report at the end: done, missing, branch, last commit, engine changes,
  shared-file edits.

## Waves

| Wave | Agent | Scope (files it owns) | Status |
|---|---|---|---|
| 1 | timeline | `editor/timeline*`, `edits.rs` | merged 6d56c34 |
| 1 | inspector | `editor/inspector/`, engine `modules/inspector` | merged f6a9906 |
| 1 | assets/title/player/export | `editor/assets/`, `title_bar.rs`, `preview.rs`, `editor/export/` | merged 7a35019 |
| 2 | design | design language: `ui/` component kit + tokens (`theme.rs`), restyle title bar, asset panel, player | merged (agent/design) — kit not yet applied to timeline + inspector |
| 2 | colour | grading engine (shader) + LUTs + Adjust/HSL/Curves/Wheels UI in the inspector | merged ba0b00f — Save as preset, auto adjust/colour match, Mask sub-tab still placeholders |
| 2 | timeline-2 | multi-select, clipboard, keyframes on clips, transitions on the timeline, text lane, detach/link audio, fade handles | merged 101f61f — no freeze frame (engine), drop ghost lacks lane (needs `MediaDrag` export), in/out marks not on ruler |
| 2 | shell | start screen + recent projects, autosave restore, settings (proxies, cache, hardware), shortcuts sheet, playback/scrub robustness | merged 331c980 — in/out marks not drawn on the ruler yet (timeline owner); proxy policy + cache limit stored but unused |
| 3 | text & titles | text tab (fonts, styles, presets), text inspector (font, size, colour, stroke, shadow, box) | |
| 3 | motion | in/out/combo presets relative to the clip, easing library, text animator (letter/word/line), punch-in zoom + auto zoom on jump cuts | merged — text animator composites on CPU (slow in export), combo restarts at a split |
| 3 | integrations | provider registry (secrets.toml 0600) + ElevenLabs TTS/SFX, OpenAI-compatible TTS, Pexels/Pixabay/Freesound search, fal.ai jobs with cost estimate, DeepL caption translation, credits file | merged — not verified against live services; fal gets whole files; no job journal |
| 3 | silence | silence + filler-word cutting with a review list, voice cleanup, loudness target on export | merged — ripple only on the clip's own and linked lanes; filler cutting needs an adapter from caption words |
| 3 | effects | short-form pack (glow, shake, light sweep, RGB split, glitch, blur, vignette), film look (grain, halation, bloom), PiP/layouts with rounded corners, gl-transitions + seamless transitions | merged — 17 effects, effect clips, PiP/split layouts, 120 gl-transitions + 5 seamless; colour params swatches only, linear keyframes |
| 3 | audio | audio effects (fades, denoise, normalise), voiceover record, beat markers | |
| 2 | captions | auto captions (OpenAI-compatible API with own base URL/key/model, or local Whisper), word/sentence mode, SRT/VTT, caption lane, styles (font, colour, stroke, box, position, karaoke highlight), emoji | merged c4418c1 — timeline S splits full text into both halves; emoji picker monochrome in GPUI; cloud path tested only against a mock |
| 2 | research | ml-features, open-assets, integrations, resolve-plugins — all merged into docs/research/ |
| 3 | tracking | merged — no re-find after occlusion/leaving frame; motion tracking T1 (Rust KLT, track material + follows link, box select on the player, re-track, smoothing, bake to keyframes) per docs/research/ml-features.md | |
| 3 | design-2 | apply the kit to timeline + inspector; in/out marks on the ruler; drop ghost with lane (export MediaDrag) | merged — inspector/effects.rs still needs the token pass; menus stay 14 px |
| 4 | perf | readback cost, shared device or DMA-BUF to GPUI, decode-ahead, GPU text animator, export fps | merged — NVENC export 52→135 fps, 4K text animator 150→3 ms; shared texture with GPUI needs a patched gpui (docs/research/gpui-shared-texture.md) |
| 4 | glue | caption split, filler adapter, ripple-all on silence cut, freeze frame, follow-link validation, proxy policy, inline text edit | done on agent/glue — the player does not switch to proxies yet (perf owns player.rs; seam `from_project_for_preview` + `proxy_generation()`); Ctrl+X deletes a tracked clip without the bake prompt; freeze length fixed at 3 s, orphaned stills not cleaned up |
| 4 | QA | end-to-end tests over the command layer, a test pass over every panel, fixes, docs/QA.md | done on agent/qa — `tests/full_workflow.rs`; fixed transition autosave, Ctrl+X bake prompt, freeze still names + cleanup, narrow asset rail, title Video tab, Effects tab tokens. Open: no text styling UI, no file access without a portal, canvas choice overridden by first import (docs/QA.md) |
| 4 | packaging | release build, desktop entry, icon, install script, README | merged — tarball 20.8 MB, CI not yet run on GitHub |
| 5 | cli | `crates/cli`: chukcut-cli subcommands + `chukcut-cli mcp` server over the command layer, docs/cli.md | merged — not yet exposed: markers, crop, curves, layouts, freeze, translation, TTS, stock |
| 5 | library | Fontsource fonts + picker, stickers (Fluent/Noto/Iconify), music & SFX pack, 20–30 own LUT looks | merged — no animated stickers, Wikimedia/Musopen skipped pending a project identity |
| 5 | speed | speed curves (time remap) with presets, keyframe easing + graph editor | merged — curved clips muted (no pitch-preserving stretch), no frame blending |
| 5 | analysis | scene detection, stabilisation, beat detection + auto-cut, auto reframe | merged — no face detector (needs ML worker); maps time via legacy source_time_at, wrong on speed-curved clips |
| 6 | titles | full title styling on the kit, colour picker, 20–30 title presets, 10 text templates | merged — new text commands not in CLI/MCP yet |
| 6 | polish | in-app file browser fallback, explicit canvas choice kept, frame-snapped ruler, prune unused materials, analysis on TimeMap, QA lows | done on agent/polish — export resolution/loudness not remembered, size estimate untouched, stale tooltip text left |
| 7 | mlworker | finish VitTrack (T2), person segmentation / background removal through the ML worker | merged — VitTrack with re-find, RVM background removal (people only), CUDA→OpenVINO→CPU; open: CUDA runtime pack (system CUDA 12.0 too old for ORT 1.28), OpenVINO untested, no BiRefNet/SAM, mattes outside the cache limit |
| 7 | demo | release build, showcase project via CLI (`scripts/demo.sh`), `docs/demo.md` feature tour | merged — showcase in `_scratch/demo` of the demo worktree; CLI gaps in docs/QA.md "Showcase pass" |
| 7 | compound | compound clips, nested sequences, several timelines per project (backlog 5) | merged — no nested-render cache, prefetch ignores compound clips, compound volume/speed keys not in audio, analysis/captions only see the open sequence |
| 7 | templates | project templates + shortcut editor (backlog 9, 10) | merged — 11 built-in templates, keymap registry (66 actions, 3 presets); open: user-template media by absolute path, split slot keeps marker, no "Replace media" in clip menu |
| 7 | upkeep | full CLI/MCP coverage, Dependabot bumps, `effects/graph.rs:735` alpha check, GPU tests on both adapters (backlog 13) | merged — reachability test with allowlist; ffmpeg-next 9 + rust-minor bumps; graph.rs alpha fixed; `scripts/gpu-tests.sh`; open: play/pause tooltip, off-grid snapping low |
| 8 | motion2 | frame blending + motion blur, animated stickers (Lottie, animated emoji, GIF/WebP) (backlog 6, 8) | merged — frame blend per clip, motion_blur effect, Lottie/GIF/WebP stickers, Noto animated emoji (CC BY 4.0); open: no optical flow, no blur inside transitions, imported GIF is now a sticker |
| 8 | compound2 | compound clip gaps: nested-render cache, prefetch, audio of compound volume/speed, flatten at speed, timeline prune, real filmstrips | merged — nested-render cache, prefetch + blend inside compounds, compound volume/speed in both mixers, flatten at any speed, timeline prune, real filmstrips; open: scene/beat/reframe inside compounds, compound own audio effects, stale nested frame after LUT file edit |
| 8 | polish2 | template follow-ups (relocatable media, split slot marker, Replace media menu, hide placeholders) + QA lows (play/pause tooltip, snapping, track path, scene button) | merged — template media copied per project, split keeps slot on the left half, Replace media in clip menu, snapping slides flush, CLI paths absolute; open: play/pause tooltip + track path not checked on screen |
| 8 | ml2 | CUDA runtime pack out of the box, SAM click-to-select object masks, BiRefNet object removal, matte cache limits | merged — CUDA bundles by driver (cu12/cu13) with no setup, "AI acceleration" settings, MobileSAM select-object + invert, BiRefNet objects (GPU only), matte cache keyed by provider + in the cache limit; open: no matte-driven grade mask, no SAM2 video propagation |
| 8 | gputex | shared GPU texture between engine and GPUI, readback as fallback (backlog 7) | merged — shared preview frames via exported buffer + patched gpui-pre in `vendor/` (4K UI thread 10–15 ms → 0.05 ms, CPU ~100% → 14–39%); readback fallback; open: Intel/hybrid laptops unchecked, export dialog cover picture black (pre-existing) |
| 8 | stable | flaky tests (templates LUT write race, lavapipe playback count), deterministic pitch-preserving audio on speed curves | merged — atomic writes for looks/tiles/fonts/stickers, per-file autosave queue, autosave off in unit tests, seeded Signalsmith Stretch (vendored), robust lavapipe timing tests |
| 9 | ml3 | AI slow motion (RIFE-class optical flow through the ML worker), matte-driven grade/effect masks (subject / background) | merged — RIFE v4 (MIT) optical-flow frames as third blend mode + "Smooth slow-mo", baked JPEG cache; matte-driven grade/effect masks (subject/background); open: 1080p RIFE ~6 fps (fp16/TensorRT next), one matte per clip |
| 9 | compound3 | scene/beat/reframe inside compound clips, compound clip's own audio effects in the mix, file identity in the nested cache, black export cover picture | merged — scenes/beats/reframe on compound content, compound EQ/comp/reverb/denoise/normalise via a cached mix-down, file identity in the nested digest, last frame shown at timeline end (black export cover fixed), compound inspector; open: stabilisation refuses compounds |
| 9 | ci | CI green on every job, clippy warnings fixed and made fatal, CLI tests in CI, GPU tests on the runner if lavapipe is there | merged — master CI green, clippy fatal (0 warnings), CLI + ML worker tests in CI, engine GPU tests on lavapipe in CI |
| 10 | ml4 | AI object removal (inpainting, LaMa-class) on SAM/painted masks, AI upscaling (Real-ESRGAN-class) | merged — Remove object (SAM click / strokes / boxes; background memory + clean plate + LaMa), Enhance quality 2x/4x (Real-ESRGAN), baked frames cache; open: static logos on moving camera shimmer, no fp16/TensorRT, bakes not queued on project open |
| 10 | colourai | auto adjust, colour match, grade presets, face/body landmarks, beauty/retouch, face-follow, voice isolation | running (agent/colourai) |
| 10 | qa2 | end-to-end QA of every new feature in the release app, fixes, showcase extended with the new features | merged — every new feature renders, undoes, survives save/reopen, export matches preview; 7 bugs fixed; showcase extended (compound, 2nd timeline, RIFE, select-object colour pop, animated GIF + motion blur); open: apply template into an open project, slots inside compounds |
| 10 | polish3 | apply template into an open project, slots inside compounds, queue bakes on project open, stabilisation in compounds, ml status symlink, scrolling bake progress | running (agent/polish3) |
| 10 | mlspeed | fp16 models, optional TensorRT pack with engine cache, IO binding, per-model provider/precision in settings and `ml bench` | running (agent/mlspeed) |

## Backlog for the next waves (lead picks from the top)

1. **[merged]** **Masks, chroma key, blend modes** — shape masks per clip (rect, ellipse, linear, mirror, heart/star, feather, invert, keyframable), green-screen chroma key with spill suppression, the inspector's blend modes (drawn disabled today). CapCut has all three.
2. **[merged]** **Audio tools** — voiceover recording (cpal input), EQ / compressor / reverb per clip, auto-ducking music under speech (uses the speech/VAD work), pitch-preserving time stretch (signalsmith-stretch or similar permissive lib) so speed-curved clips keep their sound.
3. **[merged]** **ML worker process** (docs/research/ml-features.md architecture): `chukcut-ml-worker` on `ort` with CUDA/OpenVINO EPs; first models: YuNet faces (auto-reframe), VitTrack (tracking T2), RVM or BiRefNet-lite person segmentation (local background removal; check licences — GPL is fine for us).
4. **[merged]** **Export presets and queue** (+ full CLI/MCP coverage; open: quit guard while the queue runs, queue not persisted) — TikTok/Reels/Shorts/YouTube presets, a queue, remember last settings, fix the size estimate; batch export from the CLI.
5. **[merged]** **Compound clips / nested sequences**, multi-timeline projects (CapCut "Timeline 01").
6. **[merged]** **Animated stickers** (Lottie via velato on the shared wgpu device) and Noto animated emoji.
7. **[merged]** **Shared GPU texture with GPUI** — patch gpui-pre per docs/research/gpui-shared-texture.md.
8. **[merged]** **Frame blending / motion blur** for slow sections and speed ramps (two-frame cache in the provider).
9. **[merged]** **Keyboard shortcut editor** and presets (CapCut / Premiere layouts).
10. **[merged]** **Project templates** (CapCut-style templates: placeholders for media + preset text/animations).
11. **Intel/VAAPI verification** on the owner's laptop: run `tests/every_card.rs` and the player bench there.
12. **[merged]** **Premultiply in the remaining straight-alpha pipelines** (also `transitions/library/mod.rs`; fx over-draw was latent; open: check `effects/graph.rs:735`'s data-driven blend) — `transitions/render.rs:268` and `fx/render.rs:468` still use `ALPHA_BLENDING`; on NVIDIA an 8-bit sRGB target rounds source alpha to 1/255 before blending (found and fixed for the quad pipeline in d9d86dd: soft mask edges and low opacities were drawn in steps).
13. **[merged]** **Run GPU tests on both adapters** — `test_context()` takes the default adapter. Agents testing on lavapipe missed the NVIDIA alpha rounding; run engine GPU tests once on the real GPU and once with `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`.

**Paused 2026-10-03 by the owner:** all agents and builds stopped because builds filled the SSD (4 GB free). Before restarting: every agent must share one CARGO_TARGET_DIR or delete its target/ after its branch merges; check `df -h /` first.

## Open for the next session (2026-10-03)

- **Moved to the data disk (2026-10-04):** the repository, its worktrees, `~/.cargo` and `~/.rustup` now live on `/mnt/data` (1.8 TB, separate from the system disk); the old paths are symlinks. Worktrees go to `/mnt/data/git/chukcut-<name>`. Each keeps its own `target/`; the lead deletes the worktree after its branch merges. Check `df -h /mnt/data` before launching.
- **ML worker** (backlog 3): partial, uncommitted work in `/mnt/data/git/chukcut-mlworker` (branch agent/mlworker) — review it, commit or redo.
- **Release build is stale:** rebuild `cargo build --release -p chukcut`; the running binary predates the alpha fixes (d9d86dd, agent/alpha merge).
- **CI:** green after the font fix (runs 37142894595, 37144217430 passed on 2026-10-03).
- **Dependabot PRs:** #26 (rust-minor) and #27 (ffmpeg-next 9) are applied on master (7823f49, f283efe) and can be closed; #5, #7, #8, #9, #16, #24, #25 target the removed web/Tauri code and can be closed; #28 (skrifa 0.47) is unreviewed. Closing PRs needs the owner (the session may not write to GitHub).
- **Owner decisions pending:** (1) a project identity (URL + project e-mail, not the owner's) for Wikimedia/Musopen User-Agent — until then those sources stay off; (2) delete the fork `chukfinley/filmcraft` (needs `gh auth refresh -h github.com -s delete_repo`).
- **Follow-up:** check `effects/graph.rs:735` (data-driven blend state) for the NVIDIA alpha rounding.
