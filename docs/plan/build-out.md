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
  Each agent creates its own: `git -C /home/user/git/chukcut worktree add
  /home/user/git/chukcut-<name> -b agent/<name> master` and works only there.
- Read `CLAUDE.md`, `docs/reference/capcut/README.md`, this file.
- Screenshots of CapCut: `/home/user/git/chukcut/docs/reference/capcut/*.png`
  (local only, gitignored — never commit them).
- Test visually only on a private display:
  `Xvfb :NN -screen 0 1920x1080x24 &` and
  `DISPLAY=:NN VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json ./target/debug/chukcut …`;
  capture with `DISPLAY=:NN import -window root shot.png`; drive with
  `DISPLAY=:NN xdotool …`. Never touch the owner's display `:1`, never press
  Space (audio goes to the real speakers). Pick a unique NN per agent.
- Build with `memguard-allow 12G cargo build -p chukcut -j 4`; gates:
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
| 4 | perf | readback cost, shared device or DMA-BUF to GPUI, decode-ahead, GPU text animator, export fps | running (agent/perf) |
| 4 | glue | caption split, filler adapter, ripple-all on silence cut, freeze frame, follow-link validation, proxy policy, inline text edit | running (agent/glue) |
| 4 | QA | end-to-end tests over the command layer, a test pass over every panel, fixes | |
| 4 | packaging | release build, desktop entry, icon, install script, README | |
