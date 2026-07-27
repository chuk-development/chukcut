/**
 * The property that keeps the reference honest: it is *generated* from the
 * menu bar Rust resolves, so no accelerator the menu advertises can be
 * missing from it — not for the table as it is today, and not for any item
 * anyone adds tomorrow.
 */

import { describe, expect, it } from "vitest";

import {
  buildShortcutGroups,
  displayKeys,
  EXTRA_SHORTCUTS,
  shortcutCount,
} from "@/modules/workspace/lib/shortcuts";
import type { MenuEntryView, MenuSectionView } from "@/modules/workspace/types";

function item(id: string, label: string, accelerator: string | null): MenuEntryView {
  return {
    kind: "item",
    id,
    label,
    accelerator,
    enabled: true,
    detail: null,
    unavailable_reason: null,
  };
}

/**
 * The bar as `menu.rs` describes it today — every item that carries an
 * accelerator, in its real section, including one inside a submenu. If the
 * table gains a key this fixture does not know, the generator still shows it
 * (the property below is unconditional); refreshing the fixture is only
 * needed to test that the *grouping* of the new key is sensible.
 */
const BAR: MenuSectionView[] = [
  {
    title: "File",
    entries: [
      item("file.new", "New Project", "Ctrl+N"),
      item("file.open", "Open Project…", "Ctrl+O"),
      {
        kind: "submenu",
        id: "file.recent",
        label: "Recent Projects",
        enabled: true,
        entries: [item("file.recent.clear", "Clear List", null)],
      },
      { kind: "separator" },
      item("file.save", "Save", "Ctrl+S"),
      item("file.save_as", "Save As…", "Ctrl+Shift+S"),
      item("file.import", "Import Media…", "Ctrl+I"),
      item("file.export", "Export…", "Ctrl+E"),
      item("file.project_settings", "Project Settings…", null),
      item("file.quit", "Quit", "Ctrl+Q"),
    ],
  },
  {
    title: "Edit",
    entries: [
      item("edit.undo", "Undo", "Ctrl+Z"),
      item("edit.redo", "Redo", "Ctrl+Shift+Z"),
      item("edit.cut", "Cut", "Ctrl+X"),
      item("edit.copy", "Copy", "Ctrl+C"),
      item("edit.paste", "Paste", "Ctrl+V"),
      item("edit.duplicate", "Duplicate", "Ctrl+D"),
      item("edit.delete", "Delete Clip", "Del"),
      item("edit.select_all", "Select All", "Ctrl+A"),
      item("edit.split", "Split Clip", "C"),
    ],
  },
  {
    title: "View",
    entries: [
      item("view.zoom_in", "Zoom In", "Ctrl+="),
      item("view.zoom_out", "Zoom Out", "Ctrl+-"),
      item("view.zoom_fit", "Fit Timeline", "Ctrl+0"),
      item("view.fullscreen", "Toggle Fullscreen", "F"),
    ],
  },
  {
    title: "Help",
    entries: [item("help.shortcuts", "Keyboard Shortcuts", "?")],
  },
];

/** Every accelerator anywhere in a sections array, submenus included. */
function accelerators(sections: MenuSectionView[]): string[] {
  const walk = (entries: MenuEntryView[]): string[] =>
    entries.flatMap((entry) => {
      if (entry.kind === "separator") return [];
      if (entry.kind === "submenu") return walk(entry.entries);
      return entry.accelerator ? [entry.accelerator] : [];
    });
  return sections.flatMap((section) => walk(section.entries));
}

function allKeys(sections: MenuSectionView[]): string[] {
  return buildShortcutGroups(sections).flatMap((group) =>
    group.shortcuts.map((shortcut) => shortcut.keys),
  );
}

describe("the generated reference", () => {
  it("shows every accelerator the menu table advertises", () => {
    const keys = allKeys(BAR);
    const advertised = accelerators(BAR);
    expect(advertised.length).toBeGreaterThanOrEqual(18);
    for (const accelerator of advertised) {
      expect(keys).toContain(displayKeys(accelerator));
    }
  });

  it("cannot drop an item it has never heard of — unknowns land in Application", () => {
    const novel: MenuSectionView[] = [
      { title: "Tools", entries: [item("tools.frobnicate", "Frobnicate", "Ctrl+F9")] },
    ];
    const groups = buildShortcutGroups(novel);
    const application = groups.find((group) => group.title === "Application");
    expect(application?.shortcuts.some((s) => s.keys === displayKeys("Ctrl+F9"))).toBe(true);
  });

  it("files the keys where a user would look for them", () => {
    const groups = new Map(buildShortcutGroups(BAR).map((g) => [g.title, g.shortcuts]));
    const keysOf = (title: string) => (groups.get(title) ?? []).map((s) => s.keys);

    // Clip surgery is Clips, undo and zoom are the Timeline, F is Transport.
    expect(keysOf("Clips")).toContain(displayKeys("Ctrl+X"));
    expect(keysOf("Clips")).toContain("C");
    expect(keysOf("Timeline")).toContain(displayKeys("Ctrl+Z"));
    expect(keysOf("Timeline")).toContain(displayKeys("Ctrl+0"));
    expect(keysOf("Transport")).toContain("F");
    expect(keysOf("Application")).toContain(displayKeys("Ctrl+S"));
  });

  it("keeps the keys the menu cannot advertise, transport first among them", () => {
    const groups = new Map(buildShortcutGroups([]).map((g) => [g.title, g.shortcuts]));
    const transport = (groups.get("Transport") ?? []).map((s) => s.keys);

    // J/K/L are bound in src/lib/shortcuts.ts and appear in no menu.
    for (const key of ["J", "K", "L", "Space", "Home", "End"]) {
      expect(transport).toContain(key);
    }
    // J is honest about the engine: no reverse play, so it says what it does.
    const j = (groups.get("Transport") ?? []).find((s) => s.keys === "J");
    expect(j?.description).toMatch(/step back/i);
    expect(j?.note).toMatch(/reverse play/i);
  });

  it("counts what it shows, for the dialog's subtitle", () => {
    const groups = buildShortcutGroups(BAR);
    const extras = Object.values(EXTRA_SHORTCUTS).reduce((n, list) => n + list.length, 0);
    expect(shortcutCount(groups)).toBe(accelerators(BAR).length + extras);
  });

  it("strips the menu's ellipsis from a description", () => {
    const groups = buildShortcutGroups(BAR);
    const all = groups.flatMap((g) => g.shortcuts.map((s) => s.description));
    expect(all).toContain("Open Project");
    expect(all).not.toContain("Open Project…");
  });
});
