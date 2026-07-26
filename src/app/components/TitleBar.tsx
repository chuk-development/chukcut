/**
 * The strip along the top of the window, in place of a window decoration.
 *
 * `decorations: false` in `tauri.conf.json` is what makes the menu bar ours to
 * theme, and this is the bill: the app's name, the menus, somewhere to grab, and
 * the three window buttons. Nothing here has a colour or a measurement of its
 * own — see the `--titlebar-*` block in `src/styles/globals.css`, which is the
 * single place a designer changes any of it.
 *
 * ## Dragging and double-click
 *
 * `data-tauri-drag-region` is Tauri's own mechanism and it is used rather than a
 * hand-rolled pointer handler on purpose: Tauri's injected script starts a real
 * window drag on a single press and toggles maximise on a double one, which is
 * two behaviours a webview cannot reproduce. It is matched against the exact
 * element pressed, not its ancestors, so the buttons and the menus inside the
 * strip are unaffected and only the strip's own empty space drags — which is
 * why the spacer in the middle carries the attribute as well.
 *
 * It needs `core:window:allow-start-dragging` and
 * `core:window:allow-internal-toggle-maximize` in
 * `src-tauri/capabilities/default.json`. Without them the window simply does not
 * move, which is the sort of failure that looks like a CSS bug for an hour.
 */

import { ScissorsLineDashedIcon } from "lucide-react";

import { MenuBar } from "@/modules/workspace/components/MenuBar";
import { WindowControls } from "@/modules/workspace/components/WindowControls";
import type { MenuSectionView } from "@/modules/workspace/types";

interface TitleBarProps {
  /** The bar as Rust resolved it; empty until the first answer lands. */
  sections: MenuSectionView[];
  onSelectMenuItem: (id: string) => void;
}

export function TitleBar({ sections, onSelectMenuItem }: TitleBarProps) {
  return (
    <div
      data-tauri-drag-region
      data-slot="title-bar"
      className="flex h-titlebar shrink-0 items-center gap-1 border-b border-border bg-titlebar pl-2 text-titlebar-foreground"
    >
      <div data-tauri-drag-region className="flex items-center gap-1.5 pr-1">
        <ScissorsLineDashedIcon className="size-4 shrink-0 text-primary" />
        <span data-tauri-drag-region className="text-[12px] font-semibold tracking-tight">
          chukcut
        </span>
      </div>

      <MenuBar sections={sections} onSelect={onSelectMenuItem} />

      {/* The grab area. Everything the strip does not use is somewhere to pick
          the window up by. */}
      <div data-tauri-drag-region className="h-full min-w-0 flex-1" />

      <WindowControls />
    </div>
  );
}
