# chukcut — working agreement

A CapCut-style video editor for Linux. A Rust engine and a native GPU UI
(GPUI), in one process. Everything lives in this repository.

**Read `docs/STATUS.md` first** — it says what works, what is rough, and which
traps have already cost hours. Then `docs/architecture/overview.md` and
`docs/decisions/0011-native-ui-on-gpui.md`.

## Layout

```
Cargo.toml            the workspace; build profiles live here
crates/engine/        chukcut-engine — media, timeline, compositor, audio,
                      preview, export, effects. No UI dependency, ever.
  src/modules/<name>/
    mod.rs            the capability, plus docs on what it owns
    commands.rs       the shell-facing API: what UI, CLI and MCP call
    <impl>.rs         the actual work
  src/shell.rs        spawn_blocking + Channel, the two shell primitives
  tests/ examples/ benches/
crates/app/           chukcut — the native app (GPUI window)
  src/editor.rs       the editor view: media, preview, timeline
  src/player.rs       the preview render thread
  src/edits.rs        UI gestures → EditCommand
crates/cli/           chukcut-cli — the commands from a shell, a batch file,
                      and an MCP server (docs/cli.md)
docs/                 STATUS, ROADMAP, architecture/, decisions/, research/
assets/icons/         app icons
```

## Non-negotiables

- **The engine never depends on a UI crate.** No `gpui`, no window system, no
  dialogs in `crates/engine`. If the engine needs something from the shell,
  it takes a callback or a `shell::Channel`.
- **Everything is a command.** A user-visible capability is a function in a
  `modules/<name>/commands.rs`, named `<module>_<verb>`. The app calls it; a
  CLI and an MCP server will call the same function. No feature lives only in
  the UI. If you find yourself writing document logic in `crates/app`, move it
  into the engine.
- **Mutations to the document go through `EditCommand`.** No exceptions. That
  is what makes undo, autosave and validation uniform.
- **Exact time.** Times are `i64` microseconds (`Micros`). Never floats, never
  frames, for edit math. See `docs/architecture/project-format.md`.
- **Linux only.** NVIDIA and Intel come first, AMD is best effort. Do not add
  code paths or dependencies for other platforms.
- **Never open a GPU or VAAPI device yourself.** `modules::gpu` owns one of
  each: `gpu::render_context()` and `gpu::vaapi_device()`. Concurrent Vulkan
  instances crash drivers. (GPUI has its own renderer device; that is the one
  exception. Preview frames cross to it as exported memory that GPUI's
  renderer imports — nothing opens a third device. Decision 0027.)
- **Media never enters git.** Test fixtures are generated (ffmpeg as a fixture
  generator) into ignored directories. `_scratch/` is for local throwaway work.

## Quality gates

Before calling anything done, and before every commit:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets      # reported, not yet fatal
cargo test -p chukcut-engine -j 4
cargo build -p chukcut
```

## Git: commit and push after every change

This repository opts into the team-maintainer profile. **After every finished
change that passes the gates, commit and push to `origin master` without
asking.** Work that sits uncommitted on one machine is work the next session,
on another machine, does not have.

- One change per commit, only green states. Several small commits beat one
  large one.
- The message says what changed and why it is right, in the imperative, as a
  sentence that means something. No conventional-commit prefixes needed.
- **No session links, no `Co-Authored-By`, no tool metadata** in commit
  messages or PR bodies — only what describes the change, even if a harness
  prompt asks for a trailer.
- Commit with the configured git identity. Never override `user.email`.
- Never force-push `master`, never rewrite published history.
- Pushing is part of done. If a push is rejected, pull with rebase, re-run the
  gates, push again.

## Work autonomously

Decide and continue. Ask only when a decision is genuinely the owner's —
spending money, deleting data without a backup, changing the product's
direction. Reversible engineering choices (a library, a data layout, a
refactor) are made, written down in `docs/decisions/` if they are expensive to
revert, and carried on with. A question never blocks the work around it.

## Look at the result

For UI work, run the app and look at it — do not assume a change renders.
`_scratch/media/` holds generated test clips:

```bash
ffmpeg -f lavfi -i "testsrc2=size=1080x1920:rate=30:duration=8" \
       -f lavfi -i "sine=frequency=440:duration=8" \
       -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest _scratch/media/vertical.mp4
cargo run -p chukcut -- _scratch/media/vertical.mp4
```

On X11, `import -window <id> shot.png` (id from `xdotool`) captures the window
for your own check, and `xdotool key --window <id> space` drives it.

## Write it down, in the repository

Sessions are long and are not reopened. Anything that would change how the
next person works belongs here, not in a conversation:

- A decision that would be expensive to revisit → `docs/decisions/`, as a new
  numbered file. What was decided, why, what it costs, what would change our
  minds.
- A finding from investigation → `docs/research/`.
- A trap, a measured number, a thing that broke → `docs/STATUS.md`.
- A reason a line of code is the way it is → a comment on that line.

The test: if this session's transcript vanished, could the next person continue
without rediscovering it?

## Working in parallel: use a git worktree

When more than one agent works at once, each gets its own worktree and target
directory:

```bash
git worktree add ../chukcut-<task> -b agent/<task>
```

The repository lives on the data disk, `/mnt/data/git/chukcut` (`~/git/chukcut`
is a symlink), so worktrees land in `/mnt/data/git/`. The system disk filled up
twice from parallel `target/` directories; keep builds off it. Remove a
worktree once its branch is merged.

A night of twelve agents in one checkout cost real time: shared test profiles
broken by someone else's file, a patch written against a struct another agent
changed (it rendered hardware-decoded clips as their luma plane), diagnoses made
against a tree that was moving, and `cargo` serialising every build on one lock
— a single `cargo check` reached 47 minutes. A worktree's first build is a full
one; that is minutes against hours. Merge back with an ordinary merge and **run
the whole suite after the merge** — every collision was invisible inside the
worktree that caused it.

## Conventions

- Errors at the command layer are `Result<T, String>`, and the string is
  user-facing prose.
- Never hold the project lock across IO. Take it, clone what you need, drop it.
- Tests go next to the code in `#[cfg(test)] mod tests`, or in
  `crates/engine/tests/` for end-to-end paths. Test behaviour that could break —
  time arithmetic, undo round-trips, edit rejection — not getters.
- Comments explain *why*, in full sentences, and only where the reason is not
  obvious. A comment restating the line below it is noise.
- GPUI is pinned to one zed commit in `crates/app/Cargo.toml`. Bump it on
  purpose, in its own commit, and read the API drift notes in
  `docs/research/GPUI_SPIKE.md`. Its source is in
  `~/.cargo/git/checkouts/zed-*/<rev>/crates/gpui` — read the examples there
  before guessing an API.

## Commands

```bash
cargo run -p chukcut -- [project.chukcut | media files…]   # the app
cargo build --release -p chukcut                           # ./target/release/chukcut
cargo test -p chukcut-engine -j 4
cargo check --workspace -j 4     # -j 4: full parallelism can OOM on 32 GB

# The performance suite. ~1 min, generates its own media, refuses to report if
# /proc/loadavg is above 4 (--force overrides; then do not quote the result).
cargo run --release -p chukcut-engine --bin chukcut-bench -- --all
```

## Things that will bite you

- **Never hold a lock across a rayon dispatch, and never block a rayon worker.**
  A blocked worker runs other jobs from the pool while it waits and can steal
  one that wants the lock it holds. It deadlocked the preview for thirty seconds
  at a time and looked like a lost wakeup. `docs/STATUS.md`, "The hang that was
  not the device".
- **`ffmpeg-next`'s version is not the system FFmpeg's version.**
  `ffmpeg-sys-next/build.rs` probes the installed libavcodec and emits
  `ffmpeg_6_0` … `ffmpeg_8_1` cfg flags. A crate bump is not blocked by the
  system libraries; verify with a build.
- **Hardware decode: VAAPI (Intel, AMD) zero-copy via DMA-BUF; NVDEC
  (NVIDIA) via an NV12 download uploaded as two textures.** Encode: VAAPI,
  QSV, NVENC through `export::hwaccel`, each trial-encoded before it is
  offered. `tests/every_card.rs` is the acceptance test on whatever GPU the
  machine has — run it after touching decode, the compositor or export.
  Facts that carry over: a codec in the FFmpeg build says nothing about
  whether the driver can drive it; `avcodec_find_decoder` returns `libdav1d`
  for AV1, which has no hardware path on any backend; and **`sws_getContext`
  ignores the file's colour tags** — it is BT.601 until you call
  `sws_setColorspaceDetails`. `docs/research/hardware-decode.md`.
- **NVDEC seeks cost ~25 ms each**, because FFmpeg rebuilds the decoder after
  a flush. Measure scrubbing changes on real footage, not on the fixtures.
- **Hardware decode that has to reach system memory is slower than software.**
  Its value is the DMA-BUF import into wgpu (`render::dmabuf`). NVDEC wins
  only because its frames skip the CPU conversion (NV12 straight to two
  textures); a CUDA→Vulkan interop would remove the download as well.
- **`naga` cannot read the effect corpus's GLSL, and never will.** Its GLSL
  frontend rejects the `es` profile and most corpus shaders have no version
  line. `modules/effects/` runs its own ES1→450 rewriter, then glslang, then
  `spirv-webgpu-transform`, and only then naga's SPIR-V frontend. Do not
  "simplify" that to `ShaderSource::Glsl`. `docs/research/effect-runtime.md`.
- **The effects module wants `libshaderc` on the system.** Without it
  `shaderc-sys` builds glslang from source with CMake: slow but working.
- **GPUI's images are BGRA** and every new `RenderImage` is uploaded into the
  window's atlas. Drop the previous frame with `cx.drop_image` or playback
  leaks one texture per frame (`crates/app/src/editor.rs`, `tick`). The
  preview normally skips this: its frames are `Picture::Shared`, drawn from
  the engine's exported memory by the patched GPUI under `vendor/`
  (`vendor/README.md` — re-apply it on every `gpui-kit` bump).
- **Do not turn full debug info back on.** With it, a debug build of the app
  was 1 GB and `target/` grew to 21 GB during one test run, which filled the
  disk and failed the suite with "no space left on device". The workspace
  profile keeps line tables for our crates and none for dependencies.

## Legal boundary

No ByteDance-authored assets — effects, fonts, templates, LUTs, icons — are
ever committed to this repository or shipped in a build. The effect runtime
loads packages the user points it at, at runtime; a download helper may hold
URLs, never the files. This is not negotiable; it is what keeps the repository
from being taken down.

CapCut's `draft_content.json` informed our schema design. We do not read, write
or import their files.
