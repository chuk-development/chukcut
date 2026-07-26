/**
 * The prompt in front of anything that would throw work away.
 *
 * Mounted once, at the top of the tree, and driven by the workspace store
 * rather than by whichever button asked — the question outlives the menu it was
 * raised from, and a dialog owned by a dropdown item disappears when the
 * dropdown closes.
 *
 * Three answers, in the order they should be reached for: save, cancel,
 * discard. Discard is destructive and is styled as such and placed away from
 * the other two, because the muscle memory for "the rightmost button in a
 * dialog is the safe one" is real.
 */

import { AlertTriangleIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useProjectStore } from "@/modules/project/store";
import { useWorkspaceStore } from "@/modules/workspace/store";

export function UnsavedChangesDialog() {
  const intent = useWorkspaceStore((s) => s.discardPrompt);
  const answer = useWorkspaceStore((s) => s.answerDiscardPrompt);
  const name = useProjectStore((s) => s.project?.name ?? "this project");
  const path = useProjectStore((s) => s.path);

  return (
    <Dialog
      open={intent !== null}
      // Escape and the backdrop both mean "I did not mean to do that", which is
      // cancel — never discard.
      onOpenChange={(open) => {
        if (!open) answer("cancel");
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Save your changes?</DialogTitle>
          <DialogDescription>
            {name} has edits that are not on disk. They are lost if you continue{" "}
            {intent ? `${intent} ` : ""}without saving.
          </DialogDescription>
        </DialogHeader>

        <DialogBody className="flex items-start gap-2.5">
          <AlertTriangleIcon className="mt-px size-4 shrink-0 text-destructive" />
          <p className="text-[12px] leading-relaxed text-muted-foreground">
            {path ? (
              <>
                Saving writes to <span className="font-mono text-[11px]">{path}</span>.
              </>
            ) : (
              "This project has never been saved, so saving will ask where to put it."
            )}
          </p>
        </DialogBody>

        <DialogFooter className="justify-between">
          <Button variant="destructive" size="md" onClick={() => answer("discard")}>
            Discard
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" size="md" onClick={() => answer("cancel")}>
              Cancel
            </Button>
            <Button variant="default" size="md" onClick={() => answer("save")} autoFocus>
              Save
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
