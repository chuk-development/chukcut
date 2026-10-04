# CLI and MCP

`chukcut-cli` edits chukcut projects from a shell, a script or an AI agent.
It calls the same engine functions as the app. A project that the CLI
writes opens in the app, and the reverse. The full reference is
[`docs/cli.md`](../cli.md). This page is a short start.

## Install

```bash
cargo build --release -p chukcut-cli
install -m755 target/release/chukcut-cli ~/.local/bin/
chukcut-cli --help
```

The install script and the release tarball do not install `chukcut-cli`.

## How it works

Each command opens the project file, makes one edit, checks the result, and
saves the file. When the edit fails, the file is not changed. `--dry-run`
shows the result and does not save. `chukcut-cli info PROJECT` lists the
lanes and clips with the names that other commands take.

```bash
chukcut-cli new demo.chukcut --width 1080 --height 1920 --fps 30
chukcut-cli import demo.chukcut clip.mp4 --append
chukcut-cli split demo.chukcut --at 3s
chukcut-cli export demo.chukcut out.mp4
```

The exact options of each command are in [`docs/cli.md`](../cli.md#commands)
and in `chukcut-cli <command> --help`.

## What it can do

Every edit that the app can do, and more. Some examples:

- Timeline: `append`, `place`, `split`, `delete`, `move`, `trim`, `freeze`,
  `marker`, `timeline …`, `compound …`, `template …`
- Look: `grade`, `look`, `curve`, `crop`, `auto-adjust`, `colour-match`,
  `effect …`, `layout pip`, `layout split`, `animate`, `animate-text`
- Speed: `speed-curve`, `frame-blend`, `smooth-slow-mo`
- AI tools: `remove-background`, `select-object`, `remove-object`,
  `enhance-quality`, `retouch`, `follow-face`, `track --tracker vittrack`,
  `isolate-voice`, `captions transcribe`
- AI setup: `ml status --probe`, `ml install gpu`, `ml models`, `ml bench`
- Export: `export`, `export-queue`

Batch files run many commands with one undo history. Every command can
answer in JSON, with exit codes for scripts.

## The MCP server

`chukcut-cli mcp` gives the same operations to an MCP client, for example an
AI agent. The agent can edit a project and look at a frame
(`view_frame` returns a PNG).

For Claude Code:

```bash
claude mcp add chukcut -- chukcut-cli mcp
```

Other MCP clients use the same command: the program `chukcut-cli` with the
argument `mcp`.

## Limits

Export, `render-frame` and `view_frame` need a Vulkan device, like the app.
The AI commands need `chukcut-ml-worker` next to `chukcut-cli` or on your
`PATH`.
