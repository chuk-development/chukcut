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
| 2 | design | design language: `ui/` component kit + tokens (`theme.rs`), restyle title bar, asset panel, player | running (branch agent/design) |
| 2 | colour | grading engine (shader) + LUTs + Adjust/HSL/Curves/Wheels UI in the inspector | running (branch agent/colour) |
| 2 | timeline-2 | multi-select, clipboard, keyframes on clips, transitions on the timeline, text lane, detach/link audio, fade handles | running (branch agent/timeline2) |
| 2 | shell | start screen + recent projects, autosave restore, settings (proxies, cache, hardware), shortcuts sheet, playback/scrub robustness | running (branch agent/shell) |
| 3 | text & titles | text tab (fonts, styles, presets), text inspector (font, size, colour, stroke, shadow, box) | |
| 3 | motion | animation presets (in/out/combo) via keyframes, masks, speed curves | |
| 3 | effects | our own GPU effects (blur, glow, shake, zoom, glitch, RGB split, vignette) via the effect runtime; filters tab | |
| 3 | audio | audio effects (fades, denoise, normalise), voiceover record, beat markers | |
| 3 | captions | SRT import/export, caption lane and styles | |
| 4 | perf | shared GPU device with GPUI (no readback), playback at 4K, proxies on by default for heavy files | |
| 4 | QA | end-to-end tests over the command layer, a test pass over every panel, fixes | |
| 4 | packaging | release build, desktop entry, icon, install script, README | |
