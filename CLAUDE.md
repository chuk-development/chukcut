# chukcut — working agreement

A video editor. Rust core, webview UI, Tauri 2. Read
`docs/architecture/overview.md` before writing anything.

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
- **ffmpeg-next must match the system FFmpeg.** System here is 6.1, so the
  crate is pinned to `6.1`. Bumping it without bumping the system libraries
  fails at link time with unresolved symbols.
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
