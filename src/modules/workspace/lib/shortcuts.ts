/**
 * Every keyboard shortcut the app has, in one list.
 *
 * This is a *catalogue*, not the binding table — the handlers themselves live
 * with the thing they act on (`Timeline.tsx`, `Preview.tsx`, `App.tsx`), which
 * is where they belong, because a shortcut that fires when its panel is not
 * mounted is a bug. The cost of that arrangement is that nothing is
 * discoverable, and this file pays it back.
 *
 * If you add a binding, add it here in the same commit. The reference being
 * wrong is worse than it not existing: a user who tries a listed shortcut and
 * gets nothing stops trusting the whole list.
 */

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

export const SHORTCUT_GROUPS: ShortcutGroup[] = [
  {
    title: "Project",
    shortcuts: [
      { keys: mod("N"), description: "New project" },
      { keys: mod("O"), description: "Open a project" },
      { keys: mod("S"), description: "Save" },
      { keys: mod("Shift+S"), description: "Save as…" },
      { keys: mod("E"), description: "Export the timeline" },
      { keys: mod(","), description: "Settings" },
      { keys: "?", description: "This list", note: "F1 does the same" },
    ],
  },
  {
    title: "Editing",
    caption: "Undo works across every edit; importing media is deliberately not undoable.",
    shortcuts: [
      { keys: mod("Z"), description: "Undo" },
      { keys: mod("Shift+Z"), description: "Redo", note: `${MODIFIER}+Y does the same` },
      { keys: "C", description: "Split the clip under the playhead" },
      { keys: "Del", description: "Delete the selected clip", note: "Backspace does the same" },
      { keys: "Esc", description: "Clear the selection" },
    ],
  },
  {
    title: "Tools",
    shortcuts: [
      { keys: "V", description: "Select tool" },
      { keys: "B", description: "Razor tool" },
      { keys: "S", description: "Snapping on or off" },
      { keys: `${MODIFIER}+wheel`, description: "Zoom the timeline around the pointer" },
    ],
  },
  {
    title: "Playback",
    caption: "The playhead follows the audio clock, so stepping is frame-exact.",
    shortcuts: [
      { keys: "Space", description: "Play or pause" },
      { keys: "←  →", description: "Step one frame" },
      { keys: "Shift+←  →", description: "Step ten frames" },
      { keys: "Home", description: "Jump to the start" },
      { keys: "End", description: "Jump to the end" },
      { keys: "F", description: "Fullscreen preview" },
      { keys: "Esc", description: "Leave fullscreen", note: "Only while fullscreen" },
    ],
  },
];

/** Total, for the dialog's subtitle — a count nobody has to keep in step by hand. */
export function shortcutCount(): number {
  return SHORTCUT_GROUPS.reduce((total, group) => total + group.shortcuts.length, 0);
}
