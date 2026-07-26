/**
 * The three buttons that replace a window decoration.
 *
 * They are worth a test for one reason: nothing else in the app can tell you
 * they are wired up. A minimise button that calls the wrong command looks
 * exactly like a minimise button that works, right up until the window will not
 * go down — and with `decorations: false` there is no second way to do any of
 * this. So the assertions are against what crossed the boundary, by name.
 *
 * Close is the one that matters most. It must be a *request* — the same one the
 * window's own close button used to make — so that Rust can hold the window open
 * while the unsaved-changes guard runs. A button wired to `destroy()` would pass
 * a "does it close" test and lose someone's work.
 */

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { WindowControls } from "@/modules/workspace/components/WindowControls";
import { type IpcHarness, installIpc } from "@/test/ipc";

const MINIMIZE = "plugin:window|minimize";
const TOGGLE_MAXIMIZE = "plugin:window|toggle_maximize";
const CLOSE = "plugin:window|close";
const IS_MAXIMIZED = "plugin:window|is_maximized";

let ipc: IpcHarness;

beforeEach(() => {
  ipc = installIpc();
  ipc.handle(IS_MAXIMIZED, false);
  ipc.handle(MINIMIZE, null);
  ipc.handle(TOGGLE_MAXIMIZE, null);
  ipc.handle(CLOSE, null);
});

afterEach(() => {
  ipc.restore();
});

describe("the window controls", () => {
  it("minimises through the window API", async () => {
    render(<WindowControls />);

    await userEvent.click(screen.getByRole("button", { name: "Minimise" }));

    await waitFor(() => expect(ipc.count(MINIMIZE)).toBe(1));
    expect(ipc.lastCall(MINIMIZE)).toEqual({ label: "main" });
  });

  it("maximises and restores with the same call, so the window decides", async () => {
    render(<WindowControls />);

    await userEvent.click(screen.getByRole("button", { name: "Maximise" }));

    await waitFor(() => expect(ipc.count(TOGGLE_MAXIMIZE)).toBe(1));
  });

  it("asks the window to close rather than destroying it", async () => {
    render(<WindowControls />);

    await userEvent.click(screen.getByRole("button", { name: "Close" }));

    await waitFor(() => expect(ipc.count(CLOSE)).toBe(1));
    // `close` is prevented in Rust and answered by the unsaved-changes guard.
    // `destroy` is not, and would take the document with it.
    expect(ipc.log.some((entry) => entry.command === "plugin:window|destroy")).toBe(false);
  });

  it("follows the window rather than remembering what it asked for", async () => {
    // A tiling window manager or a double-click on the strip maximises without
    // going through the button, and a restore icon on a window that is not
    // maximised is a small lie that makes the whole app feel untrustworthy.
    ipc.handle(IS_MAXIMIZED, true);
    render(<WindowControls />);

    expect(await screen.findByRole("button", { name: "Restore" })).toBeInTheDocument();
  });

  it("stays a set of buttons when the window cannot be reached at all", async () => {
    ipc.fail(IS_MAXIMIZED, "no window here");
    ipc.fail(MINIMIZE, "no window here");
    render(<WindowControls />);

    // A dead button, not a dead app: the editor has survived a missing webview
    // since the white-screen regression, and this must not undo that.
    await userEvent.click(screen.getByRole("button", { name: "Minimise" }));

    expect(screen.getByRole("button", { name: "Close" })).toBeInTheDocument();
  });
});
