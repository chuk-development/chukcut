/**
 * The system file manager.
 *
 * `opener:default` grants `reveal-item-in-dir` and *not* `open-path`, which is
 * the right shape: showing the user where a file landed needs no more authority
 * than that, and handing an arbitrary path to whatever program claims the
 * extension needs a great deal more. If "open the exported file" is ever wanted,
 * it is a capability change in `src-tauri/capabilities/`, not a call added here.
 */

import { revealItemInDir } from "@tauri-apps/plugin-opener";

export async function revealInFileManager(path: string): Promise<void> {
  await revealItemInDir(path);
}
