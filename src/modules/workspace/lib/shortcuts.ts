/**
 * The keyboard reference, generated rather than curated.
 *
 * There are two sources of truth for what a key does, and this file invents
 * neither. The menu table in `src-tauri/.../workspace/menu.rs` advertises an
 * accelerator on every item that has one, and the bar the webview receives
 * (`MenuSectionView[]`) *is* that table — so the reference reads the keys out
 * of the live bar, and a shortcut added to the menu appears here without
 * anyone remembering to copy it. `buildShortcutGroups` is that walk, and its
 * test pins the property that no accelerator in the bar can fail to appear.
 *
 * The keys the menu does not advertise — transport, tools, the mouse chords —
 * have no table to be read from, so they are catalogued in [`EXTRA_SHORTCUTS`]
 * next to a pointer at their owner. If you add a binding that the menu will
 * not advertise, add it there in the same commit. The reference being wrong is
 * worse than it not existing: a user who tries a listed shortcut and gets
 * nothing stops trusting the whole list.
 */

import type { MenuEntryView, MenuSectionView } from "@/modules/workspace/types";

export interface Shortcut {
  /** The keys, in the form they are written on a keyboard. */
  keys: string;
  description: string;
  /**
   * When the key does something different depending on where you are, say so.
   * Two entries with the same keys and no note read as a mistake.
   */
  note?: string;
}

export interface ShortcutGroup {
  title: string;
  /** One line on why this group is a group, when that is not obvious. */
  caption?: string;
  shortcuts: Shortcut[];
}

/**
 * On macOS Ctrl is written ⌘ and every handler already accepts either, since
 * they all test `event.ctrlKey || event.metaKey`. The label is chosen once,
 * here, rather than by each caller.
 */
export const MODIFIER = navigatorIsApple() ? "⌘" : "Ctrl";

function navigatorIsApple(): boolean {
  if (typeof navigator === "undefined") return false;
  return /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent || "");
}

function mod(rest: string): string {
  return `${MODIFIER}+${rest}`;
}

/** An accelerator as this machine writes it: `Ctrl+S` → `⌘+S` on a Mac. */
export function displayKeys(accelerator: string): string {
  return accelerator.replace("Ctrl", MODIFIER);
}

// ---------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------

const GROUPS = ["Transport", "Timeline", "Clips", "Application"] as const;
export type GroupTitle = (typeof GROUPS)[number];

const CAPTIONS: Partial<Record<GroupTitle, string>> = {
  Transport: "The playhead follows the audio clock, so stepping is frame-exact.",
  Clips: "The clipboard outlives the project it was filled from, so clips can cross projects.",
};

/**
 * Which group a menu item's key belongs in. Ids not listed fall back to
 * Application, so a new menu item cannot vanish from the reference by not
 * being mapped — it merely lands in the general group until someone files it.
 */
const GROUP_OF_MENU_ID: Record<string, GroupTitle> = {
  "edit.undo": "Timeline",
  "edit.redo": "Timeline",
  "view.zoom_in": "Timeline",
  "view.zoom_out": "Timeline",
  "view.zoom_fit": "Timeline",
  "edit.cut": "Clips",
  "edit.copy": "Clips",
  "edit.paste": "Clips",
  "edit.duplicate": "Clips",
  "edit.delete": "Clips",
  "edit.select_all": "Clips",
  "edit.split": "Clips",
  "view.fullscreen": "Transport",
};

/**
 * Colour on top of a menu-derived row: things the table cannot say because
 * they are about a *second* binding for the same action.
 */
const NOTES_BY_MENU_ID: Record<string, string> = {
  "edit.redo": `${MODIFIER}+Y does the same`,
  "edit.delete": "Backspace does the same",
  "help.shortcuts": "F1 does the same",
  "file.quit": "Asks before discarding unsaved work",
};

// ---------------------------------------------------------------------------
// The bindings the menu does not advertise
// ---------------------------------------------------------------------------

/**
 * Owners, for the next person: Space/←/→/Home/End are `Preview.tsx`, J/K/L are
 * `src/lib/shortcuts.ts`, the tool letters and Esc are `Timeline.tsx`,
 * Ctrl+, and Alt are `App.tsx`/`MenuBar.tsx`, the wheel chord is the
 * timeline's wheel handler.
 */
export const EXTRA_SHORTCUTS: Record<GroupTitle, Shortcut[]> = {
  Transport: [
    { keys: "Space", description: "Play or pause" },
    { keys: "L", description: "Play" },
    { keys: "K", description: "Pause" },
    {
      keys: "J",
      description: "Pause and step back one frame",
      note: "Reverse play is not supported by the engine",
    },
    { keys: "←  →", description: "Step one frame" },
    { keys: "Shift+←  →", description: "Step ten frames" },
    { keys: "Home", description: "Jump to the start" },
    { keys: "End", description: "Jump to the end" },
    { keys: "Esc", description: "Leave fullscreen", note: "Only while fullscreen" },
  ],
  Timeline: [
    { keys: "V", description: "Select tool" },
    { keys: "B", description: "Razor tool" },
    { keys: "S", description: "Snapping on or off" },
    { keys: `${MODIFIER}+wheel`, description: "Zoom the timeline around the pointer" },
  ],
  Clips: [
    { keys: "Esc", description: "Clear the selection" },
    { keys: mod("click"), description: "Add a clip to the selection, or take it out" },
    { keys: "Shift+click", description: "Select the run of clips along one lane" },
    { keys: "Drag", description: "Rubber-band select", note: "On empty lane space" },
  ],
  Application: [
    { keys: mod(","), description: "Settings" },
    {
      keys: "Alt",
      description: "Open the menu bar",
      note: "Then ← → between menus, ↑ ↓ inside one",
    },
  ],
};

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/** Every item in a drawn bar that advertises a key, submenus included. */
function acceleratedItems(
  entries: MenuEntryView[],
): { id: string; label: string; accelerator: string }[] {
  return entries.flatMap((entry) => {
    if (entry.kind === "separator") return [];
    if (entry.kind === "submenu") return acceleratedItems(entry.entries);
    return entry.accelerator
      ? [{ id: entry.id, label: entry.label, accelerator: entry.accelerator }]
      : [];
  });
}

/**
 * The reference, from the live bar plus the extras.
 *
 * The property the test pins: **every accelerator in `sections` appears in the
 * result.** There is no filter and no allowlist on that path — an unknown id
 * falls back to the Application group rather than falling out.
 */
export function buildShortcutGroups(sections: MenuSectionView[]): ShortcutGroup[] {
  const byGroup = new Map<GroupTitle, Shortcut[]>(GROUPS.map((title) => [title, []]));

  for (const section of sections) {
    for (const item of acceleratedItems(section.entries)) {
      const group = GROUP_OF_MENU_ID[item.id] ?? "Application";
      byGroup.get(group)?.push({
        keys: displayKeys(item.accelerator),
        // The menu label is already the action's name; "…" is a menu-ism.
        description: item.label.replace(/…$/, ""),
        note: NOTES_BY_MENU_ID[item.id],
      });
    }
  }

  for (const title of GROUPS) {
    byGroup.get(title)?.push(...EXTRA_SHORTCUTS[title]);
  }

  return GROUPS.map((title) => ({
    title,
    caption: CAPTIONS[title],
    shortcuts: byGroup.get(title) ?? [],
  }));
}

/** Total, for the dialog's subtitle — a count nobody has to keep in step by hand. */
export function shortcutCount(groups: ShortcutGroup[]): number {
  return groups.reduce((total, group) => total + group.shortcuts.length, 0);
}
