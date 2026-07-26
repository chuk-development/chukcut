# chukcut — working agreement

A video editor. Rust core, webview UI, Tauri 2. Everything lives in this
repository; there is no other source tree.

**Read `docs/STATUS.md` first** — it says what works, what is rough, and which
traps have already cost hours. Then `docs/architecture/overview.md`.

## Write it down, in the repository

Sessions are long and are not reopened. Anything that would change how the next
person works belongs here, not in a conversation:

- A decision that would be expensive to revisit → `docs/decisions/`, as a new
  numbered file. Say what was decided, why, what it costs, and what would change
  our minds.
- A finding from investigation → `docs/research/`.
- A trap, a measured number, a thing that broke → `docs/STATUS.md`.
- A reason a line of code is the way it is → a comment on that line.

The test: if this session's transcript vanished, would the next person be able
to continue without rediscovering it? If not, it is not written down yet.

## Working in parallel: use a git worktree

When more than one agent works at once, **each one gets its own git worktree**:

```bash
git worktree add ../chukcut-<task> -b agent/<task>
```

This is not tidiness. A night of twelve concurrent agents in one checkout cost
real time in ways worth naming, because each one looks like a code problem and
is not:

- Two agents' test code broke the shared `lib test` profile, so **six agents
  could not run a single test** until someone fixed files they did not own.
- A compositor patch was written against a struct that a different agent
  changed before it could be applied. It now renders a hardware-decoded clip as
  its luma plane — a convincing greyscale picture — and had to be re-derived.
- A preview test failed in three different ways across three runs while another
  agent was mid-refactor underneath it. Two of the three diagnoses were wrong,
  and the investigation was worthless until the tree stopped moving.
- `cargo` serialises on one build lock per target directory, so twelve agents
  did not build twelve times faster. A single `cargo check` reached 47 minutes.

The trade to understand before reaching for it: **a worktree has its own
`target/`, so the first build in each is a full one.** That is minutes of CPU
against hours of untangling. Take the worktree whenever two agents' file scopes
could plausibly touch, and share a checkout only for genuinely disjoint work —
one agent in `src/`, one in `src-tauri/`, and nothing shared between them.

Merge back with an ordinary branch merge, and **run the whole suite after the
merge**, not only in the worktree. Every collision listed above was invisible
inside the worktree that caused it.

## The rule that matters

**Rust owns the machine, the webview owns the pixels.** No file system access,
no process spawning, no decoding, no GPU work in TypeScript. Every capability
crosses the boundary as a registered `#[tauri::command]`. If you find yourself
wanting to read a file from React, you are adding a command instead.

## Layout

Module names are mirrored on both sides. `foo` in Rust and `foo` in TS are the
same feature seen from two directions.

```
src-tauri/src/modules/<name>/
  mod.rs         the capability, plus docs on what it owns
  commands.rs    the #[tauri::command] surface, nothing else
  <impl>.rs      the actual work

src/modules/<name>/
  components/    React components
  lib/           typed invoke() wrappers — components never call invoke directly
  store.ts       Zustand slice for this module's UI state
```

Shared UI primitives go in `src/components/ui/` (shadcn). Anything
feature-specific belongs to its module, not to `components/`.

## Conventions

**Rust**

- Commands are named `<module>_<verb>`: `media_probe`, `timeline_split`.
- Errors are `Result<T, String>` and the string is user-facing prose.
- Never hold the project lock across IO. Take it, clone what you need, drop it.
- Times are `i64` microseconds. Never floats, never frames. See
  `docs/architecture/project-format.md`.
- Mutations to the document go through `EditCommand`. No exceptions.
- Tests go next to the code in `#[cfg(test)] mod tests`. Test behaviour that
  could plausibly break — time arithmetic, undo round-trips, edit rejection —
  not getters.

**TypeScript**

- `pnpm biome check --write .` before considering anything done.
- Path alias `@/` maps to `src/`.
- Zustand stores hold UI state. The project document is server state: it comes
  from Rust and is replaced wholesale after each edit, never patched locally.
- Tailwind v4, tokens from `src/styles/globals.css`. Use the semantic tokens
  (`bg-panel`, `text-muted-foreground`, `bg-track-video`) rather than raw
  colours, so retheming is one file.

**Both**

- Comments explain *why*, and only where the reason is not obvious from the
  code. A comment restating the line below it is noise.
- Match the surrounding style. This codebase writes prose comments in full
  sentences.

## Commands

```bash
pnpm install              # deps
pnpm tauri dev            # run the app (Vite + Rust, hot reload on both)
pnpm build                # typecheck + bundle frontend
pnpm biome check --write .
cd src-tauri && cargo test
cd src-tauri && cargo check -j 4    # -j 4: full parallelism OOMs on 32 GB

# The performance suite. ~1 min, generates its own media, refuses to report if
# /proc/loadavg is above 4 (pass --force to override, and then do not quote the
# result). --filter <group>, --json <file>, --compare <file>. See docs/STATUS.md.
cd src-tauri && cargo run --release --bin chukcut-bench -- --all
```

## Things that will bite you

- **`cargo check` with default parallelism gets OOM-killed** on this machine
  while compiling wgpu and the Tauri macro crates. Use `-j 4`.
- **Never hold a lock across a rayon dispatch, and never block a rayon worker.**
  A worker blocked inside a parallel iterator runs other jobs from the pool
  while it waits, so it can steal one that wants the lock it holds; and a
  non-rayon thread dispatching under a lock waits for a worker the pool cannot
  free. Both deadlocked the preview for thirty seconds at a time and looked
  like a lost wakeup. `docs/STATUS.md`, "The hang that was not the device".
- **Never open a GPU or VAAPI device.** `modules::gpu` owns one of each for the
  process and hands out references: `gpu::render_context()` for wgpu,
  `gpu::vaapi_device()` for VAAPI. `RenderContext::open` and `VaapiDevice::open`
  are crate-private and called only from there. Concurrent Vulkan instances crash
  this driver, and a VAAPI driver has a finite number of contexts. The evidence,
  and the two bugs that sharing one device exposed in the preview server, are in
  `docs/STATUS.md` under "One GPU device and one VAAPI device".
- **`ffmpeg-next`'s version is not the system FFmpeg's version.** This note
  previously claimed the crate had to match the system libraries. It does not:
  `ffmpeg-sys-next/build.rs` probes the installed libavcodec and emits
  `ffmpeg_6_0` … `ffmpeg_8_1` cfg flags, and the crate supports FFmpeg 3.4
  upward. We are on `6.1` against system 6.1, which is fine, but a bump is not
  blocked by the system libraries. Verify with a build before relying on it.
- **Hardware encode is built, on VAAPI.** The safe wrapper over
  `AVHWFramesContext` lives in `src-tauri/src/modules/export/hwframes.rs` and
  `h264_vaapi`/`hevc_vaapi` work. Two things about it that will otherwise cost
  you an afternoon: an encoder being present in the FFmpeg build says nothing
  about whether the driver can drive it, and VAAPI's rate-control modes are the
  driver's rather than FFmpeg's. Both are in `docs/STATUS.md` under "Traps".
  What is *not* built is zero-copy — the composited frame is still read back to
  the CPU and uploaded again; see `docs/research/zero-copy-encode.md`.
- **Hardware decode is built, on VAAPI, and is now the default** — but only on a
  device that can import the decoded surface as a texture.
  `src-tauri/src/modules/media/hwdecode.rs` and `dmabuf.rs` decode; H.264, HEVC,
  VP9 and AV1 all work on this chip. `render::dmabuf::import_plane` and the
  compositor's two-plane case are what make it worth having: hardware decode
  that still has to reach system memory is *slower* than software, because the
  download out of a tiled surface plus the swscale pass cost more than the
  decode. `media::provider::DEFAULT_ACCELERATION` is `Auto`, gated on
  `RenderContext::can_import_dmabuf()`, with `CHUKCUT_DECODE=software|auto|vaapi`
  still overriding it. Measured every way in
  `docs/research/hardware-decode.md` and `docs/STATUS.md`. Three things there
  that will otherwise cost you an afternoon: `avcodec_find_decoder` returns a
  decoder that *cannot* drive the GPU for AV1, a decoder being in the build says
  nothing about the driver, and **`sws_getContext` ignores the file's colour
  tags** — it is BT.601 until you call `sws_setColorspaceDetails`.
- **Preview frames do not go through `invoke()`.** They are served over the
  `chukcut-frame://` protocol. Read `docs/architecture/preview-pipeline.md`
  before touching the preview path; the reasoning there is load-bearing.
- **`naga` cannot read the effect corpus's GLSL, and never will.** Its GLSL
  frontend accepts only `#version` 440/450/460 and rejects the `es` profile,
  and 224 of 228 corpus shaders have no version line at all; the tracking issue
  was closed as not planned. `modules/effects/` therefore runs its own
  ES1→450 rewriter, then glslang, then `spirv-webgpu-transform` to split the
  combined image samplers WebGPU has no concept of, and only then naga's
  *SPIR-V* frontend. Do not "simplify" that to `ShaderSource::Glsl`. See
  `docs/research/rust-crate-survey.md` §6b and
  `docs/research/effect-runtime.md`.
- **The effects module wants `libshaderc` on the system.** `shaderc-sys` links
  Ubuntu's `libshaderc.so` when it is there and otherwise builds glslang and
  SPIRV-Tools from source with CMake, which is slow but works. If a cold build
  suddenly grows several minutes, that is what happened.

## Legal boundary

No ByteDance-authored assets — effects, fonts, templates, icons — are ever
committed to this repo or shipped in a build. The effect runtime loads packages
from a URL provided by the user at runtime. This is not negotiable; it is what
keeps the repo from being taken down.

CapCut's `draft_content.json` informed our schema design. We do not read, write
or import their files.
