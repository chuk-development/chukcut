/**
 * The keyboard reference.
 *
 * There are a few dozen bindings and, until this existed, no way to find out
 * about any of them except by reading the source or hovering every icon in the
 * toolbar. The list itself is in `lib/shortcuts.ts`; this only draws it.
 */

import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { SHORTCUT_GROUPS, shortcutCount } from "@/modules/workspace/lib/shortcuts";

interface ShortcutsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ShortcutsDialog({ open, onOpenChange }: ShortcutsDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[600px]">
        <DialogHeader>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
          <DialogDescription>
            {shortcutCount()} of them. Single keys do nothing while you are typing in a field.
          </DialogDescription>
        </DialogHeader>

        <DialogBody className="max-h-[min(64vh,520px)] overflow-y-auto">
          <div className="grid grid-cols-1 gap-x-8 gap-y-5 sm:grid-cols-2">
            {SHORTCUT_GROUPS.map((group) => (
              <section key={group.title} aria-label={group.title}>
                <h3 className="text-[12px] font-semibold text-panel-foreground">{group.title}</h3>
                {group.caption ? (
                  <p className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
                    {group.caption}
                  </p>
                ) : null}
                <dl className="mt-2 flex flex-col gap-1">
                  {group.shortcuts.map((shortcut) => (
                    <div
                      key={`${group.title}-${shortcut.keys}-${shortcut.description}`}
                      className="flex items-baseline gap-3"
                    >
                      <dt className="w-[92px] shrink-0 text-right">
                        <Keys keys={shortcut.keys} />
                      </dt>
                      <dd className="min-w-0 flex-1 text-[12px] leading-snug">
                        {shortcut.description}
                        {shortcut.note ? (
                          <span className="ml-1.5 text-[11px] text-muted-foreground">
                            {shortcut.note}
                          </span>
                        ) : null}
                      </dd>
                    </div>
                  ))}
                </dl>
              </section>
            ))}
          </div>
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}

/** Keycaps. Split on `+` so each key is its own cap, the way a keyboard looks. */
function Keys({ keys }: { keys: string }) {
  return (
    <span className="inline-flex flex-wrap justify-end gap-0.5">
      {keys.split("+").map((key, position) => (
        <span key={`${keys}-${key}`} className="contents">
          {position > 0 ? <span className="text-[10px] text-muted-foreground">+</span> : null}
          <kbd className="rounded-[3px] border border-border bg-surface px-1 py-px font-mono text-[10px] leading-[14px] text-panel-foreground">
            {key}
          </kbd>
        </span>
      ))}
    </span>
  );
}
