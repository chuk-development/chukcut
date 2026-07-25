# chukcut

A video editor for Linux, Windows and macOS. Rust engine, web UI, Tauri.

Built because the good editors on Linux are either professional tools with a
professional learning curve, or they are CapCut, which does not run here.

## Status

Early. Phase 0 (skeleton, document model, edit commands, undo/redo) is done;
Phase 1 (import → timeline → preview → export) is in progress. See
[`docs/ROADMAP.md`](docs/ROADMAP.md).

## Stack

| Layer | Choice | Why |
|---|---|---|
| Shell | Tauri 2 | Native process owning the machine, webview for the UI |
| UI | React 19 + TypeScript + Vite | The UI is the part that has to iterate fast |
| Styling | Tailwind v4 + shadcn/ui | Design tokens in one file, components we own |
| State | Zustand | UI state only; the document lives in Rust |
| Engine | Rust | Document, editing, compositing, encoding |
| GPU | wgpu | Vulkan / Metal / D3D12 / GL from one codebase |
| Media | FFmpeg (`ffmpeg-next`) | Decode, encode, container handling |
| Lint | Biome | One tool, fast |

## Build

Requires Rust, Node 20+, pnpm, and the Tauri Linux dependencies
(`webkit2gtk-4.1`, `libsoup-3.0`, `librsvg`), plus FFmpeg development headers
matching the pinned `ffmpeg-next` version (6.1).

```bash
pnpm install
pnpm tauri dev
```

## Documentation

- [Architecture overview](docs/architecture/overview.md)
- [Project format](docs/architecture/project-format.md)
- [IPC contract](docs/architecture/ipc-contract.md)
- [Timeline editing](docs/architecture/timeline-editing.md)
- [Preview pipeline](docs/architecture/preview-pipeline.md)
- [Roadmap](docs/ROADMAP.md)
- [Working agreement for contributors and agents](CLAUDE.md)

## License

TBD.
