# Architecture overview

chukcut is a video editor: a Rust core that owns media, the GPU and the file
system, and a webview frontend that owns everything the user looks at.

## Why this split

The previous attempt at this project put the UI in native Rust (egui). It
produced a timeline that could not export, because every button, panel and drag
interaction had to be drawn by hand and each iteration cost a compile. The
work went into re-implementing widgets instead of into editing video.

That is the real reason, and it is sufficient on its own. The UI is the part of
an editor that has to be rebuilt fifty times before it feels right, so it
belongs in the toolchain with the fastest iteration loop.

An earlier draft of this document also claimed CapCut Desktop does the same
thing. It does not, and the claim has been removed rather than softened. A
full inventory of the Windows build (`docs/research/ui-inventory.md`) found
**2,007 QML files inside `VECreator.dll`** — the editor is entirely native Qt
Quick. Chromium and Lynx are present, but they render the commerce and account
surfaces around the editor, never the timeline. Every CEF `.pak` in the install
is stock Chromium, unpatched.

What does hold:

- **CapCut Web** is a TypeScript UI driving the same engine compiled to WASM
  (`vesdk-lvapi.wasm` plus `libffmpeg.wasm`). So the engine/UI split across a
  language boundary is proven at their scale, even if the desktop client draws
  its own widgets.
- ByteDance can afford to hand-build two thousand QML files. We cannot. That
  asymmetry is the argument, not an appeal to their example.

## The two processes

```
┌─────────────────────────────────────────────────────────┐
│  Webview  (React 19 + TypeScript)                       │
│                                                         │
│  src/modules/<domain>/                                  │
│    components/   what the user sees                     │
│    lib/          typed wrappers around invoke()         │
│    store.ts      Zustand slice                          │
│                                                         │
│  Owns: layout, interaction, local UI state              │
│  Owns nothing else. No fs, no process, no decoder.      │
└───────────────────────┬─────────────────────────────────┘
                        │  invoke() commands
                        │  Channel<T> event streams
                        │  chukcut-frame:// custom protocol
┌───────────────────────▼─────────────────────────────────┐
│  Rust  (src-tauri)                                      │
│                                                         │
│  src-tauri/src/modules/<domain>/                        │
│    mod.rs        the capability                         │
│    commands.rs   the #[tauri::command] surface          │
│                                                         │
│  Owns: document, media, GPU, encoding, disk             │
└─────────────────────────────────────────────────────────┘
```

Module names match on both sides. `timeline` exists twice: once as edit
operations over the document, once as the lane UI that issues them.

## Modules

| Module | Rust responsibility | Frontend responsibility |
|---|---|---|
| `project` | Document model, load/save, validation | Project store, open/save dialogs |
| `timeline` | Edit commands, undo/redo | Lanes, clips, drag/trim/split, ruler |
| `media` | FFmpeg probe/decode, thumbnails, waveforms | Media library, import |
| `render` | wgpu compositor: project + time → frame | — |
| `preview` | Frame server, playback clock | Canvas player, transport controls |
| `export` | Full-res render + encode, progress | Export dialog, presets, progress |
| `effects` | Lua + shader effect runtime | Effect browser, parameter panel |
| `workspace` | Paths, caches, settings, recents | Settings UI, start screen |

## The data flow

There is one source of truth: the `Project` struct in
`modules/project/document.rs`. Everything else is derived from it.

```
    user gesture
         │
         ▼
  EditCommand ──────► History.apply() ──────► Project mutated
                                                    │
                        ┌───────────────────────────┼──────────────────┐
                        ▼                           ▼                  ▼
                  frontend store             render graph         export walk
                  (full document              (project + t          (project +
                   returned by IPC)            → one frame)          every t)
```

The frontend never mutates the document locally and then syncs. It sends a
command, Rust applies it, Rust returns the new document, the store replaces
itself. Blunt, but it makes desync bugs structurally impossible.

## What is deliberately not here

- **No CapCut project import.** We use their format as a design reference for
  our own schema because it is mature, not because we read their files.
- **No bundled effect assets.** The effect runtime loads packages from a URL
  the user provides at runtime. Nothing ByteDance-made ships with the app.
- **No plugin system yet.** Extension points get designed once the core edit
  loop is solid, not before.

## Reading order for a new agent

1. `docs/architecture/project-format.md` — the data model everything derives from
2. `docs/architecture/ipc-contract.md` — how the two halves talk
3. `docs/architecture/timeline-editing.md` — how mutations work
4. `docs/architecture/preview-pipeline.md` — the one genuinely hard problem
5. `docs/ROADMAP.md` — what is built, what is next
