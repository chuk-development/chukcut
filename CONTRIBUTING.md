# Contributing

Contributions are welcome — bug reports as much as code.

By contributing you agree that your work is licensed under **GPL-3.0-or-later**.
There is no CLA: this project has deliberately given up the option to relicense,
so there is nothing for one to protect.

## Before you start

Read **[`docs/STATUS.md`](docs/STATUS.md)**. It is long because it records what
things cost, and it will save you an afternoon at least once. Then
[`CLAUDE.md`](CLAUDE.md), which is the working agreement — conventions, module
layout, and the traps that have already bitten someone.

For anything larger than a bug fix, open an issue first. A design that lands in
the wrong module is expensive to move afterwards.

## Setting up

```bash
sudo apt install \
  libwebkit2gtk-4.1-dev libsoup-3.0-dev librsvg2-dev \
  libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev \
  libavdevice-dev libswscale-dev libswresample-dev \
  libva-dev libasound2-dev libshaderc-dev nasm cmake

pnpm install
pnpm tauri dev
```

**You do not need a GPU to contribute.** Tests that require a Vulkan adapter or
FFmpeg skip with a printed reason instead of failing, so a machine without them
still runs a green suite — it just runs fewer tests.

## Before you open a pull request

```bash
pnpm biome check --write .        # format and lint the frontend
pnpm build                        # typecheck
pnpm test -- --run                # vitest

cd src-tauri
cargo fmt                         # formatting is enforced in CI
cargo clippy --all-targets
cargo test
```

`cargo check` with default parallelism gets OOM-killed on 32 GB while compiling
wgpu and the Tauri macro crates. Use `-j 4`.

If you touched anything on the preview or export path, run the benchmark suite
and say what happened in the pull request:

```bash
cd src-tauri
cargo run --release --bin chukcut-bench -- --all
```

It generates its own fixtures and refuses to report on a busy machine. Compare
against a baseline with `--compare src-tauri/benches/baseline-<commit>.json`.

## The rules that matter

- **Rust owns the machine, the webview owns the pixels.** No file access, no
  process spawning, no decoding, no GPU work in TypeScript. Capabilities cross
  the boundary as registered Tauri commands.
- **Document mutations go through `EditCommand`.** Every edit must be
  invertible, because undo is built out of the inverses rather than snapshots.
- **Times are `i64` microseconds.** Never floats, never frame numbers.
- **Never open a GPU or VAAPI device.** `modules::gpu` owns one of each and
  hands out references. Concurrent Vulkan instances crash some drivers.
- **Write down what you learned.** A decision that would be expensive to revisit
  goes in `docs/decisions/`; an investigation goes in `docs/research/`; a trap
  or a measured number goes in `docs/STATUS.md`. The test: if your session
  vanished, could the next person continue without rediscovering it?

## Commit messages

Say what changed and why it is right, in the imperative. The existing history is
the style guide — it favours a sentence that means something over a conventional
prefix.

## Reporting bugs

Include the build (`pnpm tauri dev` or a release build), your GPU and driver,
`ffmpeg -version`, and what the app logged. For anything about playback or
export, the log line naming the chosen encoder and frame path is usually the
whole diagnosis.
