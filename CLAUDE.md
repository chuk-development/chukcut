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
```

## Things that will bite you

- **`cargo check` with default parallelism gets OOM-killed** on this machine
  while compiling wgpu and the Tauri macro crates. Use `-j 4`.
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
- **Hardware decode is built, on VAAPI, and is deliberately not the default.**
  `src-tauri/src/modules/media/hwdecode.rs` and `dmabuf.rs`; H.264, HEVC, VP9 and
  AV1 all decode on this chip. It is off because it is *slower* than software
  while the frame still has to arrive in system memory — the download out of a
  tiled surface plus the swscale pass cost more than the decode — and only pays
  once `render/` can import the surface as a texture. Measured both ways in
  `docs/research/hardware-decode.md`, which also carries the patch `render/`
  needs. Two things there that will otherwise cost you an afternoon:
  `avcodec_find_decoder` returns a decoder that *cannot* drive the GPU for AV1,
  and a decoder being in the build says nothing about the driver.
- **Preview frames do not go through `invoke()`.** They are served over the
  `chukcut-frame://` protocol. Read `docs/architecture/preview-pipeline.md`
  before touching the preview path; the reasoning there is load-bearing.

## Legal boundary

No ByteDance-authored assets — effects, fonts, templates, icons — are ever
committed to this repo or shipped in a build. The effect runtime loads packages
from a URL provided by the user at runtime. This is not negotiable; it is what
keeps the repo from being taken down.

CapCut's `draft_content.json` informed our schema design. We do not read, write
or import their files.
