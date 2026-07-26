/**
 * Minimise, maximise/restore, close — the three buttons the window manager used
 * to draw for us.
 *
 * They exist because `decorations: false` bought a menu bar we can theme and
 * took the title bar with it. Everything they do goes through
 * `workspace/lib/window.ts`, which is where the permissions this needs are
 * written down.
 *
 * Close is the same action as File → Quit, deliberately: Rust prevents the
 * close, the webview runs its unsaved-changes guard, and only then does the
 * window go. Wiring the X straight to a `destroy()` is how an editor loses
 * someone's work.
 *
 * Colours and size come from `--titlebar-*` in `src/styles/globals.css`.
 */

import { MinusIcon, SquareIcon, XIcon } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { cn } from "@/lib/utils";
import {
  closeWindow,
  isWindowMaximized,
  minimizeWindow,
  toggleMaximizeWindow,
} from "@/modules/workspace/lib/window";

export function WindowControls() {
  const [maximized, setMaximized] = useState(false);

  /**
   * Follow the window rather than remembering what we asked for.
   *
   * A tiling window manager, a keyboard shortcut or a double-click on the strip
   * can maximise the window without going through these buttons, and a restore
   * icon on a window that is not maximised is the kind of small lie that makes
   * a UI feel untrustworthy. Resize is the only signal available, and every
   * maximise is one.
   */
  const refresh = useCallback(() => {
    void isWindowMaximized().then((value) => {
      if (value !== null) setMaximized(value);
    });
  }, []);

  useEffect(() => {
    refresh();
    window.addEventListener("resize", refresh);
    return () => window.removeEventListener("resize", refresh);
  }, [refresh]);

  return (
    <div data-slot="window-controls" className="flex items-center">
      <ControlButton label="Minimise" onClick={() => void minimizeWindow()}>
        <MinusIcon />
      </ControlButton>
      <ControlButton
        label={maximized ? "Restore" : "Maximise"}
        onClick={() => void toggleMaximizeWindow().then(refresh)}
      >
        <SquareIcon className={maximized ? "scale-[0.8]" : undefined} />
      </ControlButton>
      <ControlButton label="Close" destructive onClick={() => void closeWindow()}>
        <XIcon />
      </ControlButton>
    </div>
  );
}

function ControlButton({
  label,
  destructive,
  onClick,
  children,
}: {
  label: string;
  destructive?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={cn(
        "grid size-titlebar shrink-0 place-items-center outline-none",
        "focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
        "[&_svg]:size-3.5 [&_svg]:shrink-0",
        destructive
          ? "hover:bg-titlebar-close hover:text-destructive-foreground"
          : "hover:bg-titlebar-hover",
      )}
    >
      {children}
    </button>
  );
}
