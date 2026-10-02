# 0006 — The menu bar is drawn in the webview, and decided in Rust

> **Webview era.** Written for the Tauri + React shell, which was removed on
> 2026-10-02 (decision 0011). Kept for its reasoning; the code paths it names
> under `src/` and `src-tauri/` no longer exist.

Status: decided 2026-07-27. Replaces the `tauri::menu::Menu` built in
`src-tauri/src/modules/workspace/menu.rs` and covers window decorations,
accelerators and the `--titlebar-*` tokens.

## The decision

**The window has no decorations** (`decorations: false` in `tauri.conf.json`) and
we draw the strip at the top ourselves: the app name, the menus, and
minimise/maximise/close. `src/app/components/TitleBar.tsx`.

**The menus are React**, out of the same shadcn/Radix `dropdown-menu` primitives
as every other menu in the editor, so they take the theme with the rest of it.
`src/modules/workspace/components/MenuBar.tsx`.

**Nothing about the bar is decided on the TypeScript side.** `menu.rs` keeps the
one table — every item, its label, its accelerator, and the single `Gate` that
says when it is clickable — and `workspace_menu_describe` hands the whole bar
over already resolved: titles, labels, keys, enabled flags, and the reason for
any item that can never be enabled. `MenuBar.tsx` renders that answer. Adding an
item in React is therefore not possible, which is the property `Gate` exists to
enforce: an item you can click does something.

**Colour, height, radius, hover and disabled state come from CSS variables** —
the `--titlebar-*` block in `src/styles/globals.css` and the ordinary
`--popover`/`--accent`/`--border` tokens for the dropdowns. No component in the
strip carries a hex value or a pixel measurement of its own.

**Accelerators are advertised by the bar and bound next to what they do.** The
table's `accelerator` field is what gets printed down the right of a menu; the
`keydown` handlers stay in `Timeline.tsx`, `Preview.tsx` and `App.tsx`.

## Why

The bar it replaced was a real GTK widget, and the *desktop* drew it. That is the
whole reason it went. No token in `globals.css` could reach it, so it stayed the
machine's own grey however the app was themed, and on a machine with a different
GTK theme it did not look like part of the application at all. Retheming the
editor is one file by design (`CLAUDE.md`, "tokens from `src/styles/globals.css`
… so retheming is one file"), and the menu bar was the one surface exempt from
that.

Keeping the table in Rust rather than moving it across with the widget is the
part worth defending. The alternative — sending only the enabled flags, or
sending nothing and letting React hold its own list of items — puts the labels
and the gates in two places. The gates are the interesting half: `menu.rs` has
fourteen unit tests over `enablement`, including an exhaustive sweep of all 512
states asserting that nothing which edits a document is ever clickable without
one, and those tests are worth more than the widget ever was. Sending the
structure as well costs one round trip per *distinct* document state — the sync
already deduplicates, because a pointer move over the lanes writes the playhead
many times a second — and buys a codebase where the bar cannot grow a row that
looks live and does nothing.

## What it costs

**Everything a window decoration used to do is now ours.** Four of the five are
straightforward — drag, minimise, maximise, close — and the fifth is a trap:
an undecorated GTK window has no resize border, because the border *was* the
decoration. Without `ResizeEdges.tsx` the window cannot be resized at all, and
nothing says so. See `docs/STATUS.md`, "An undecorated GTK window has no resize
border".

**Five extra window permissions** in `capabilities/default.json`
(`start-dragging`, `start-resize-dragging`, `minimize`, `toggle-maximize`,
`internal-toggle-maximize`, `close`). None is in `core:default`, and a missing
one is not an error — the call is rejected at the boundary and the button does
nothing.

**The bar is empty for the first frame or two**, until the first
`workspace_menu_describe` answers. The strip itself is not: it is rendered
unconditionally, before any document exists and even when the backend never
answered, because with no decorations it is the only way to move or close the
window. A round trip that fails leaves the last bar drawn rather than blanking
it — a stale menu still opens, and an empty menu bar looks like the app has lost
its menus.

**Alt, ←/→ and hover-to-switch are ours to implement.** Radix's dropdown knows
about one menu, not a bar of them: it gives us ↑/↓/Home/End, Enter, Escape and
skipping disabled items, and `MenuBar.tsx` adds the horizontal half. The menus
are also deliberately `modal={false}`, because a modal Radix dropdown marks the
rest of the document `aria-hidden` — including the menubar the user is arrowing
along.

## What would change our minds

- **A second window.** The strip and its state are per-window and nothing about
  this arrangement is shared. A floating preview or scope window would want the
  strip factored out first.
- **macOS.** The system menu bar there is not a strip in the window and cannot be
  drawn by us; a Mac build would keep `tauri::menu` on that platform and feed it
  from the same `describe` output. `menu.rs` was deliberately left as a table for
  exactly this reason.
- **Radix growing a `menubar` primitive we adopt.** `@radix-ui/react-menubar`
  exists and does the horizontal half already. It was not taken because it is a
  dependency and a second menu-styling surface for behaviour that is thirty lines
  here; if the app grows a second menubar, revisit.
