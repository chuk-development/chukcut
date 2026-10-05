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
| 10 | colourai | auto adjust, colour match, grade presets, face/body landmarks, beauty/retouch, face-follow, voice isolation | merged — auto adjust, colour match (Lab + histogram curves), grade presets, MediaPipe face mesh, retouch effect (4 presets), follow face, HTDemucs voice isolation; open: body landmarks (RTMPose), worker RSS 6.8–8 GB on isolation (handed to mlspeed) |
| 10 | qa2 | end-to-end QA of every new feature in the release app, fixes, showcase extended with the new features | merged — every new feature renders, undoes, survives save/reopen, export matches preview; 7 bugs fixed; showcase extended (compound, 2nd timeline, RIFE, select-object colour pop, animated GIF + motion blur); open: apply template into an open project, slots inside compounds |
| 10 | polish3 | apply template into an open project, slots inside compounds, queue bakes on project open, stabilisation in compounds, ml status symlink, scrolling bake progress | merged — template apply into an open project (timeline or compound), slots inside compounds, `modules/prepare` bakes on project open with one title-bar chip, stabilisation on compounds, sequence walk frame-time fix, ml status symlink, pinned bake strips |
| 10 | mlspeed | fp16 models, optional TensorRT pack with engine cache, IO binding, per-model provider/precision in settings and `ml bench` | merged — Fast mode (TensorRT add-on, per-model plans: RIFE TRT fp32 1.5–1.7x, Real-ESRGAN fp16 2.5x, LaMa fp16 2.1x, BiRefNet 2.6x), engine cache, HTDemucs peak 1.5 GB (was 6.8), OOM drop-and-retry; open: one engine per input size |
| 11 | body | zip/tar downloads, RTMPose body landmarks + follow body part, body box for reframe, face/voice queues over every timeline in the prepare chip | merged — zip/tar.gz model downloads, YOLOX-tiny + RTMPose-m (Apache-2.0) body tracks, follow body part (13 parts), people box in auto reframe, face/voice queues over all timelines in the prepare chip; open: fingers, id swaps when people cross, detector NMS on CPU |
| 11 | docs | README feature overview, user manual in docs/manual/ (keyboard table checked against the keymap registry), STATUS top summary | merged — README rewritten, docs/manual/ (17 pages), keymap_doc test, STATUS "At a glance" |
| 11 | ux | install worker + CLI with the app, inspector crop, speed-effect presets, Basic-tab stubs, hardware-decode + ML runtime in settings, persisted export queue + quit guard | merged — worker + CLI installed with the app, GPU "Reduce noise" effect, speed-effect presets, inspector Crop (ratios, handles, rotate/flip), decode + AI runtime pickers, persisted export queue + quit guard, exit hang fixed; open: crop not keyframable, denoise spatial only |
| 11 | qa3 | end-to-end QA of waves 10–11, installed layout, showcase extension | merged — all wave 10–11 features pass render/undo/reopen/export checks, installed layout finds the worker, 6 bugs fixed (MCP/batch argument check, landmark frame count), showcase 27.7 s |
| 11 | release | manual-only `release.yml`: .deb, AppImage x86_64 + aarch64, macOS universal .dmg, Windows .exe zip + installer; Linux jobs must work, macOS/Windows best effort (`continue-on-error`) | merged — `release.yml` (workflow_dispatch only, per-target checkboxes, optional draft release), .deb tested in ubuntu:24.04, x86_64 AppImage tested in Debian/Fedora without FFmpeg, aarch64 type-checked only; macOS/Windows jobs experimental (blocked by ~20 Linux-only engine files, decision 0033) |
| 11 | shutdown | rare segfault at CLI exit after export, unreadable project path message, global slot numbers, short clip ids, D-Bus helper cleanup | merged — export thread drops its job before `Done`; `lifecycle::exit` (export shutdown, worker shutdown, flush, `_exit`) in CLI and app; 0 failures in 180 loaded runs (was 5 segfaults + 1 hang in 90); five QA lows fixed |
| 12 | research-gaps | gap analysis against CapCut, Resolve, Premiere, Kdenlive, Shotcut → `docs/research/gap-analysis-2026-10.md` with proposed waves | merged — top gaps: HDR input, BT.601 export without colour tags, no on-player transform handles, animated/rich captions, text-based editing, reverse, long-to-shorts, local TTS/translation, multicam; waves 12–14 proposed in the report |
| 12 | research-audit | code health, robustness, UX and performance audit → `docs/research/quality-audit-2026-10.md` with work packages | merged — 21 findings, 13 work packages (export audio over range only, panic containment, document validation, job generation, bounded caches, dead preview server, locks off UI, shared bake store, errors + a11y, build diet); 3 small panics fixed |
| 12 | crop2 | crop keyframes, temporal denoise | merged (CI on lavapipe verifies; NVIDIA GPU gates pending the reboot) — crop keyframes (four edge properties), Reduce noise temporal mode |
| 12 | gaps2 | matte masks inside transitions and accumulation, flow under remove object/enhance, TensorRT dynamic shapes, temporal consistency for remove object on a moving camera | merged (fmt, clippy, test build green; CI on lavapipe verifies; NVIDIA GPU gates pending the reboot) — engine lib + compositor, matte_masks, optical_flow, enhance, temporal on GPU and lavapipe; real-model tests with the release worker. RIFE/ESRGAN TensorRT size ranges (fewer engines, same speed), remove-object pan flicker 4.6 → 0.65 |
| 12 | colourio | export matrix + colour tags (correctness), HLG/PQ tone mapping to BT.709, 10-bit decode and HEVC main10 export | merged (CI on lavapipe verifies; NVIDIA GPU gates pending the reboot) — export matrix + tags fixed (worst error 39 → 0–2 code values), PQ/HLG tone map (BT.2390), 10-bit decode + HEVC Main 10 / AV1 10-bit export; needs after the reboot: `cargo test -p chukcut-cli` (delivery failed mid-hang), real-GPU colour/every_card/export/export_presets on the merged tree. Note: `VideoStreamSpec` gained a required `colour` field |
| 12 | robust | export audio mixed over the range in blocks, panic hook + containment (player, export, queue, MCP, jobs), document validation incl. CLI export, project-generation tags on background jobs | merged, decision renumbered to 0035 (CI on lavapipe verifies; NVIDIA GPU gates pending the reboot) — streamed export audio (2 s from 3 h: 4.1 GB → 2.3 MB, bit-identical), panic hook + containment everywhere, document validation + render_check, project generations on jobs, run_export joins its thread; needs after the reboot: full suites one GPU binary at a time, `scripts/loop-test.sh` on tests/export.rs. **Decision 0034 is taken by both colourio and robust — renumber robust to 0035 on merge; both touch export/job.rs, merge colourio first** |
| 13 | canvas | on-player select/move/scale/rotate handles, snap guides, safe areas, player full screen (CPU-only while the GPU is hung) | running (agent/canvas) |
| 13 | captions2 | rich text spans, animated caption styles + presets (CPU-only) | running (agent/captions2) |
| 13 | bakestore | shared derived-cache store for all bake caches, bounded decoder/sticker/filmstrip/LUT caches (CPU-only) | running (agent/bakestore) |
| 13 | diet | remove the dead webview preview server, real player bench rows, build diet (wgpu dup, image features, LTO), dead commands (CPU-only) | running (agent/diet) |

## Backlog for the next waves (lead picks from the top)

1. **[merged]** **Masks, chroma key, blend modes** — shape masks per clip (rect, ellipse, linear, mirror, heart/star, feather, invert, keyframable), green-screen chroma key with spill suppression, the inspector's blend modes (drawn disabled today). CapCut has all three.
2. **[merged]** **Audio tools** — voiceover recording (cpal input), EQ / compressor / reverb per clip, auto-ducking music under speech (uses the speech/VAD work), pitch-preserving time stretch (signalsmith-stretch or similar permissive lib) so speed-curved clips keep their sound.
3. **[merged]** **ML worker process** (docs/research/ml-features.md architecture): `chukcut-ml-worker` on `ort` with CUDA/OpenVINO EPs; first models: YuNet faces (auto-reframe), VitTrack (tracking T2), RVM or BiRefNet-lite person segmentation (local background removal; check licences — GPL is fine for us).
4. **[merged]** **Export presets and queue** (+ full CLI/MCP coverage; quit guard and persisted queue merged with agent/ux) — TikTok/Reels/Shorts/YouTube presets, a queue, remember last settings, fix the size estimate; batch export from the CLI.
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

## Open for the next session (updated 2026-10-04)

- **2026-10-05 06:10 — NVIDIA driver hung machine-wide.** Trigger: six parallel copies of the export test binary (incl. every-encoder tests, likely past the NVENC session limit) killed with SIGTERM mid-GPU-work (agent/robust). gnome-shell, Xvfb, nvidia-smi and exports sat in D state; only a reboot clears it. **Rule from now on: at most one GPU test binary or export at a time per machine; never SIGTERM a process inside GPU work — let it finish.** After the reboot: (1) master has the agent/crop2 merge committed locally but NOT verified or pushed — re-run fmt/clippy/worker build/workspace tests, then push; (2) resume agents robust, gaps2, colourio from their worktrees (`/mnt/data/git/chukcut-{robust,gaps2,colourio}`) — each was told to commit CPU-green states and list what still needs a GPU run; (3) reopen the showcase on :1.

- **Machine:** the repository, its worktrees, `~/.cargo` and `~/.rustup` live on `/mnt/data` (1.8 TB); the old paths are symlinks. Worktrees go to `/mnt/data/git/chukcut-<name>`; the lead removes each after its merge. Check `df -h /mnt/data` before launching.
- **Resource budget:** every Claude process runs in `claude.slice` (`~/.config/systemd/user/claude.slice`: 70% CPU, RAM throttled above 70%, hard cap 80%, swap 4 GB). Builds at `-j 3`, at most 4 agents at once. Before this, parallel builds pushed the user session into memory pressure and systemd-oomd killed terminals with the session in them.
- **Merge routine:** merge one branch at a time, then `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -j 3 -- -D warnings`, rebuild `chukcut-ml-worker`, `cargo test --workspace -j 3 --no-fail-fast`; push only when all pass. Conflicts so far were in shared tables (Cargo.toml patch table, worker protocol version, settings fields, decision numbers) — keep both sides.
- **Waves 7–11 are merged** (incl. qa3, shutdown, release). CI on master is green again since 55fa145. Wave 12 running (two research agents, two gap agents).
- **Showcase:** `/mnt/data/git/chukcut-qa3/_scratch/demo/showcase.chukcut` (built by `scripts/demo.sh`; the qa3 worktree is kept for it). The owner's display shows it in the release build.
- **CI:** clippy is fatal; CLI, ML worker and engine GPU tests (lavapipe) run in CI. Check `gh run list` after each push.
- **Dependabot PRs:** #26 (rust-minor) and #27 (ffmpeg-next 9) are applied on master (7823f49, f283efe) and can be closed; #5, #7, #8, #9, #16, #24, #25 target the removed web/Tauri code and can be closed; #28 (skrifa 0.47) is unreviewed. Closing PRs needs the owner (the session may not write to GitHub).
- **Owner decisions pending:** (1) a project identity (URL + project e-mail, not the owner's) for the Wikimedia/Musopen User-Agent — until then those sources stay off; (2) delete the fork `chukfinley/filmcraft` (needs `gh auth refresh -h github.com -s delete_repo`); (5) remove four AppImage extract dirs (~230 MB each) at `/tmp/appimage_extracted_*`; (6) trigger Actions › Release once and check the ARM, macOS and Windows jobs (the `macos-15-intel` label is unverified); (4) remove five apport core dumps (~550 MB each) left by the crash repro: `sudo rm /var/lib/apport/coredump/core._mnt_data_git_chukcut-qa3_*`; (3) delete the mattes made from the owner's own clip during the ML worker check: `~/.cache/chukcut/mattes/person-376879dd99acffd5-*`.
- **Needs the owner's hardware:** backlog 11, Intel/VAAPI verification on the laptop (`tests/every_card.rs`, player bench, OpenVINO path).
- **Known gaps worth a next wave:** crop keyframes; temporal/ML denoise; RIFE fp16 quality; one TensorRT engine per input size; body fingers (RTMW) and id swaps; static logos on a moving camera in Remove object; matte-limited effects inside transitions; optical flow under Remove object.
