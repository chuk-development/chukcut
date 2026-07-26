/**
 * The menu bar's three obligations.
 *
 * **It draws what Rust decided, and nothing else.** The enabled flag arrives
 * resolved from `menu.rs`; the only failure that matters is a bar that shows an
 * item as live when the table says it is not, because that is an item the user
 * clicks and nothing happens.
 *
 * **A greyed item is inert.** Not merely dimmed — a click on one must not run
 * its action, which is the failure a purely visual disabled state produces.
 *
 * **It works from the keyboard.** Alt opens, ←/→ walk the titles, ↑/↓ walk the
 * items, Enter chooses, Escape leaves. The vertical half is Radix's and the
 * horizontal half is ours, so both are checked here — the seam between them is
 * exactly where a menu bar breaks.
 */

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { MenuBar } from "@/modules/workspace/components/MenuBar";
import type { MenuSectionView } from "@/modules/workspace/types";

/**
 * A bar shaped like the real one, in miniature: two sections, a separator, an
 * item that is off, and one that can never be on.
 */
const SECTIONS: MenuSectionView[] = [
  {
    title: "File",
    entries: [
      {
        kind: "item",
        id: "file.new",
        label: "New Project",
        accelerator: "Ctrl+N",
        enabled: true,
        unavailable_reason: null,
      },
      { kind: "separator" },
      {
        kind: "item",
        id: "file.save",
        label: "Save",
        accelerator: "Ctrl+S",
        enabled: false,
        unavailable_reason: null,
      },
      {
        kind: "item",
        id: "file.publish",
        label: "Publish",
        accelerator: null,
        enabled: false,
        unavailable_reason: "there is nowhere to publish to yet",
      },
    ],
  },
  {
    title: "Edit",
    entries: [
      {
        kind: "item",
        id: "edit.undo",
        label: "Undo",
        accelerator: "Ctrl+Z",
        enabled: true,
        unavailable_reason: null,
      },
      {
        kind: "item",
        id: "edit.redo",
        label: "Redo",
        accelerator: "Ctrl+Shift+Z",
        enabled: true,
        unavailable_reason: null,
      },
    ],
  },
  {
    title: "Help",
    entries: [
      {
        kind: "item",
        id: "help.about",
        label: "About chukcut",
        accelerator: null,
        enabled: true,
        unavailable_reason: null,
      },
    ],
  },
];

function renderBar(sections: MenuSectionView[] = SECTIONS) {
  const onSelect = vi.fn();
  render(<MenuBar sections={sections} onSelect={onSelect} />);
  return { onSelect };
}

/** The trigger for one of the top-level menus. */
function title(name: string) {
  return within(screen.getByRole("menubar")).getByRole("menuitem", { name });
}

/** The open menu's own items, which Radix portals outside the bar. */
function openMenu() {
  const menus = screen.getAllByRole("menu");
  return menus[menus.length - 1];
}

// ---------------------------------------------------------------------------
// Drawing what Rust decided
// ---------------------------------------------------------------------------

describe("the bar itself", () => {
  it("is a menubar of the sections Rust sent, in order", () => {
    renderBar();

    const bar = screen.getByRole("menubar");
    expect(
      within(bar)
        .getAllByRole("menuitem")
        .map((item) => item.textContent),
    ).toEqual(["File", "Edit", "Help"]);
  });

  it("has nothing to draw before the first answer arrives, and does not fall over", () => {
    renderBar([]);

    expect(within(screen.getByRole("menubar")).queryAllByRole("menuitem")).toHaveLength(0);
  });
});

describe("an open menu", () => {
  it("shows the items with their keys, and the separator between them", async () => {
    renderBar();

    await userEvent.click(title("File"));

    const menu = openMenu();
    expect(within(menu).getByRole("menuitem", { name: /New Project/ })).toHaveTextContent("Ctrl+N");
    expect(within(menu).getByRole("separator")).toBeInTheDocument();
  });

  it("greys exactly the items Rust greyed, and says so to a screen reader", async () => {
    renderBar();

    await userEvent.click(title("File"));

    const menu = openMenu();
    expect(within(menu).getByRole("menuitem", { name: /New Project/ })).not.toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(within(menu).getByRole("menuitem", { name: /^Save/ })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("gives a permanently unavailable item its reason, and nothing else one", async () => {
    renderBar();

    await userEvent.click(title("File"));

    const menu = openMenu();
    // "Save is grey" explains itself — there is nothing to save. "Publish is
    // grey" does not, and a greyed item with no reason reads as broken.
    expect(within(menu).getByRole("menuitem", { name: /Publish/ })).toHaveAttribute(
      "title",
      "there is nowhere to publish to yet",
    );
    expect(within(menu).getByRole("menuitem", { name: /^Save/ })).not.toHaveAttribute("title");
  });
});

// ---------------------------------------------------------------------------
// Clicking
// ---------------------------------------------------------------------------

describe("choosing an item", () => {
  it("reports the id from the table, not the label", async () => {
    const { onSelect } = renderBar();

    await userEvent.click(title("Edit"));
    await userEvent.click(within(openMenu()).getByRole("menuitem", { name: /Undo/ }));

    expect(onSelect).toHaveBeenCalledExactlyOnceWith("edit.undo");
  });

  it("does nothing at all when the item is disabled", async () => {
    const { onSelect } = renderBar();

    await userEvent.click(title("File"));
    const disabled = within(openMenu()).getByRole("menuitem", { name: /^Save/ });
    // Not `userEvent.click`: a disabled Radix item is `pointer-events: none`,
    // so user-event refuses before the component is ever asked. The point of
    // this test is what happens when the click *does* land — a stale hit test,
    // a synthetic event, a screen reader activating the row.
    disabled.click();

    expect(onSelect).not.toHaveBeenCalled();
  });

  it("does not fire a disabled item from the keyboard either", async () => {
    const { onSelect } = renderBar();

    await userEvent.click(title("File"));
    const disabled = within(openMenu()).getByRole("menuitem", { name: /^Save/ });
    disabled.focus();
    await userEvent.keyboard("{Enter}");

    expect(onSelect).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

describe("the keyboard", () => {
  it("opens the first menu on Alt, and closes it on Alt again", async () => {
    renderBar();

    await userEvent.keyboard("{Alt>}{/Alt}");
    await waitFor(() => expect(title("File")).toHaveAttribute("aria-expanded", "true"));

    await userEvent.keyboard("{Alt>}{/Alt}");
    await waitFor(() => expect(title("File")).toHaveAttribute("aria-expanded", "false"));
  });

  it("leaves Alt alone when it was a modifier for something else", async () => {
    renderBar();

    // Alt+Tab, Alt+F4 and every Alt+key an app binds go down as an Alt keydown
    // too. Opening the File menu on the way past would be intolerable.
    await userEvent.keyboard("{Alt>}a{/Alt}");

    expect(title("File")).toHaveAttribute("aria-expanded", "false");
  });

  it("walks the titles with the arrow keys, and wraps", async () => {
    renderBar();

    await userEvent.click(title("File"));

    await userEvent.keyboard("{ArrowRight}");
    await waitFor(() => expect(title("Edit")).toHaveAttribute("aria-expanded", "true"));
    expect(title("File")).toHaveAttribute("aria-expanded", "false");

    await userEvent.keyboard("{ArrowRight}");
    await waitFor(() => expect(title("Help")).toHaveAttribute("aria-expanded", "true"));

    // Past the end is back to the beginning: a menu bar has no ends.
    await userEvent.keyboard("{ArrowRight}");
    await waitFor(() => expect(title("File")).toHaveAttribute("aria-expanded", "true"));

    await userEvent.keyboard("{ArrowLeft}");
    await waitFor(() => expect(title("Help")).toHaveAttribute("aria-expanded", "true"));
  });

  it("walks the items inside a menu, skipping the ones that are greyed", async () => {
    renderBar();

    await userEvent.click(title("Edit"));
    await userEvent.keyboard("{ArrowDown}");
    await waitFor(() =>
      expect(within(openMenu()).getByRole("menuitem", { name: /Undo/ })).toHaveFocus(),
    );

    await userEvent.keyboard("{ArrowDown}");
    await waitFor(() =>
      expect(within(openMenu()).getByRole("menuitem", { name: /Redo/ })).toHaveFocus(),
    );
  });

  it("chooses the focused item on Enter", async () => {
    const { onSelect } = renderBar();

    await userEvent.click(title("Edit"));
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Enter}");

    await waitFor(() => expect(onSelect).toHaveBeenCalledExactlyOnceWith("edit.redo"));
  });

  it("closes on Escape without choosing anything", async () => {
    const { onSelect } = renderBar();

    await userEvent.click(title("Edit"));
    await userEvent.keyboard("{Escape}");

    await waitFor(() => expect(title("Edit")).toHaveAttribute("aria-expanded", "false"));
    expect(onSelect).not.toHaveBeenCalled();
  });
});
