# CLI and MCP

`chukcut-cli` edits chukcut projects from a shell, a script or an AI agent.
It calls the same engine functions as the app. A project that the CLI
writes opens in the app, and the reverse. The full reference is
[`docs/cli.md`](../cli.md). This page is a short start.

## Install

The install script (`scripts/install.sh`) and the release tarball install
`chukcut-cli` into `~/.local/bin`, next to the editor. To build it by hand:

```bash
cargo build --release -p chukcut-cli
install -m755 target/release/chukcut-cli ~/.local/bin/
chukcut-cli --help
```

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
AI agent. The agent finds every command with `command_list`, runs one with
`command_run`, reads the clips with `doc_inspect` and looks at the picture
with `render_frame` (a PNG in the answer).

For Claude Code:

```bash
claude mcp add chukcut -- chukcut-cli mcp
```

Other MCP clients use the same command: the program `chukcut-cli` with the
argument `mcp`.

### Let the agent work in the open window

Start the app with the control socket, and the server in bridge mode:

```bash
chukcut --control my.chukcut
claude mcp add chukcut-app -- chukcut-cli mcp --bridge
```

The agent's edits then appear in the window at once and go into the app's
undo history: Ctrl+Z takes them back like your own. Nothing is saved until
you save. The agent can also read the playhead and the selection, press
keys, click, and take a screenshot of the window (X11 only).

The control socket is off unless you start the app with `--control`. Only
your user can connect to it.

The design of the MCP server is taken from FilmCraft
(storytold/filmcraft, Apache-2.0). [`docs/cli.md`](../cli.md#the-mcp-server)
has every tool and the control socket's methods.

## Limits

Export, `render-frame` and `render_frame` need a Vulkan device, like the app.
The AI commands need `chukcut-ml-worker` next to `chukcut-cli` or on your
`PATH`.
