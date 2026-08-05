<!--
Say what changed and why it is right. If it fixes an issue, "Fixes #123".
-->

## What this changes

## Why

## How it was verified

<!--
Name the evidence, not the intention. A test that fails without the change, a
benchmark comparison, a file you played in another player.
-->

## Checklist

- [ ] `pnpm biome check --write .`, `pnpm typecheck`, `pnpm test:run`
- [ ] `cargo fmt`, `cargo clippy --all-targets`, `cargo test`
- [ ] Preview or export path touched → benchmark run, numbers in the description
- [ ] Anything the next person would otherwise rediscover is written down in
      `docs/` (decision, research note, or a line in `docs/STATUS.md`)
