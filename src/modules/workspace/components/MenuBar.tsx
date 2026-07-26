/**
 * The menu bar, drawn by us.
 *
 * It used to be a `tauri::menu::Menu` — a GTK widget the desktop drew, in the
 * desktop's colours, which is precisely why it is gone: no token in
 * `src/styles/globals.css` could reach it, so it stayed the machine's grey
 * whatever the app looked like. This is the same bar out of the same shadcn
 * primitives as every other menu in the editor, so it follows the theme along
 * with the rest of it.
 *
 * **It decides nothing.** What the menus are called, what is in them, which keys
 * they advertise and which items are live all arrive from
 * `workspace_menu_describe` — one table, in `src-tauri/.../workspace/menu.rs`,
 * where each item's gate is written next to it. This component renders that
 * answer. Adding an item here is not possible, which is the point: an item that
 * looks live and does nothing cannot be introduced by a change to the view.
 *
 * ## Colours, height, radius
 *
 * All from `--titlebar-*` in `src/styles/globals.css`, and from the ordinary
 * popover tokens for the dropdowns. Nothing below carries a hex value.
 *
 * ## Keyboard
 *
 * Alt opens the first menu (and closes an open one), ←/→ move between menus,
 * ↑/↓ move inside one, Enter activates and Escape closes. The vertical half is
 * Radix's — its menu content is a roving-focus group and gets ↑/↓/Home/End,
 * Enter and Escape right, including skipping disabled items. The horizontal half
 * is ours, because a menu*bar* is not a thing Radix's dropdown knows about: the
 * keydown handler on the wrapper sees ←/→ from the open menu as well, since
 * Radix portals its content through a React portal and React events still
 * bubble through the tree.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { MODIFIER } from "@/modules/workspace/lib/shortcuts";
import type { MenuEntryView, MenuSectionView } from "@/modules/workspace/types";

export interface MenuBarProps {
  /** The bar as Rust resolved it. Empty until the first answer lands. */
  sections: MenuSectionView[];
  /** An item was chosen. The id is `menu.rs`'s, and `runMenuAction` knows them. */
  onSelect: (id: string) => void;
}

/**
 * The accelerator as this machine writes it.
 *
 * Rust spells the modifier `Ctrl` because that is what it is on the two
 * platforms this runs on; macOS writes it ⌘ and every handler in the app
 * already accepts either. `MODIFIER` is the one place that choice is made.
 */
export function displayAccelerator(accelerator: string): string {
  return accelerator.replace("Ctrl", MODIFIER);
}

/**
 * Every entry with something to be identified by.
 *
 * An item has an id. A separator has nothing — it is a rule — so it borrows the
 * id of the item above it, which is stable for exactly as long as the table in
 * `menu.rs` is. Its position in the array would do the same job right up until
 * someone made the bar's contents conditional.
 */
function keyed(section: MenuSectionView): { key: string; entry: MenuEntryView }[] {
  let previous = "top";
  return section.entries.map((entry) => {
    if (entry.kind === "item") {
      previous = entry.id;
      return { key: entry.id, entry };
    }
    return { key: `after:${previous}`, entry };
  });
}

export function MenuBar({ sections, onSelect }: MenuBarProps) {
  /** Which menu is open, by index. `null` is "none". */
  const [openIndex, setOpenIndex] = useState<number | null>(null);
  /**
   * Set while a ←/→ is handing the focus from one menu to the next, so the menu
   * being closed does not pull the focus back to its own trigger on the way out
   * — which makes the whole bar flicker through its titles.
   */
  const moving = useRef(false);

  // A bar that shrinks under an open menu would leave `openIndex` pointing past
  // the end. Nothing renders from it then, but the next ← would wrap to a menu
  // that is not there.
  useEffect(() => {
    if (openIndex !== null && openIndex >= sections.length) setOpenIndex(null);
  }, [openIndex, sections.length]);

  /**
   * Alt, on its own, opens the first menu — and closes it again if it is already
   * open, which is what every desktop menu bar does.
   *
   * On keyup rather than keydown, and only if nothing else was pressed in
   * between: Alt is a modifier first. Alt+Tab, Alt+F4 and every Alt+key an
   * application binds go down as an Alt keydown too, and opening the File menu
   * on the way past would be intolerable.
   */
  useEffect(() => {
    let alone = false;
    const onKeyDown = (event: KeyboardEvent) => {
      alone = event.key === "Alt" && !event.ctrlKey && !event.metaKey && !event.shiftKey;
    };
    const onKeyUp = (event: KeyboardEvent) => {
      const wasAlone = alone;
      alone = false;
      if (event.key !== "Alt" || !wasAlone) return;
      event.preventDefault();
      setOpenIndex((current) => (current === null ? 0 : null));
    };
    // A click while Alt is held is a modified click, not a menu request.
    const onPointerDown = () => {
      alone = false;
    };

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("pointerdown", onPointerDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("pointerdown", onPointerDown);
    };
  }, []);

  const step = useCallback(
    (delta: number) => {
      if (sections.length === 0) return;
      moving.current = true;
      setOpenIndex((current) =>
        current === null ? 0 : (current + delta + sections.length) % sections.length,
      );
    },
    [sections.length],
  );

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent) => {
      // Radix's menu content is a vertically-oriented roving focus group, so it
      // ignores ←/→ and lets them through to here. ↑/↓/Enter/Escape it keeps,
      // which is exactly the division of labour we want.
      if (event.key === "ArrowRight") {
        event.preventDefault();
        step(1);
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        step(-1);
      }
    },
    [step],
  );

  return (
    <div
      role="menubar"
      aria-label="Application menu"
      data-slot="menu-bar"
      onKeyDown={onKeyDown}
      className="flex items-center"
    >
      {sections.map((section, index) => (
        <DropdownMenu
          key={section.title}
          open={openIndex === index}
          onOpenChange={(open) => setOpenIndex(open ? index : null)}
          // Not modal. A modal Radix dropdown marks the whole rest of the
          // document `aria-hidden` while it is open — including this menubar,
          // which would make the other three titles invisible to a screen
          // reader exactly while the user is arrowing between them. It also
          // locks scrolling, which a menu bar has no business doing.
          modal={false}
        >
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              role="menuitem"
              // Once a menu is open, sliding across the titles opens each in
              // turn without a second click. Every menu bar does this and it is
              // unlearnable if it is missing.
              onPointerEnter={() => setOpenIndex((current) => (current === null ? null : index))}
              className={cn(
                "rounded-titlebar px-2 py-0.5 text-[12px] outline-none",
                "hover:bg-titlebar-hover focus-visible:ring-2 focus-visible:ring-ring",
                "data-[state=open]:bg-titlebar-active",
              )}
            >
              {section.title}
            </button>
          </DropdownMenuTrigger>

          <DropdownMenuContent
            align="start"
            sideOffset={2}
            onCloseAutoFocus={(event) => {
              if (moving.current) {
                moving.current = false;
                event.preventDefault();
              }
            }}
          >
            {keyed(section).map(({ key, entry }) =>
              entry.kind === "separator" ? (
                <DropdownMenuSeparator key={key} />
              ) : (
                <DropdownMenuItem
                  key={key}
                  data-menu-id={entry.id}
                  disabled={!entry.enabled}
                  // Only ever set for an item that can *never* be enabled. An
                  // item greyed because nothing is selected explains itself;
                  // one greyed because the feature does not exist does not.
                  title={entry.unavailable_reason ?? undefined}
                  onSelect={() => onSelect(entry.id)}
                >
                  {entry.label}
                  {entry.accelerator ? (
                    <DropdownMenuShortcut>
                      {displayAccelerator(entry.accelerator)}
                    </DropdownMenuShortcut>
                  ) : null}
                </DropdownMenuItem>
              ),
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      ))}
    </div>
  );
}
