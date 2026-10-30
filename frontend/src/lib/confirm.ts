import { confirm } from "@tauri-apps/plugin-dialog";

/** Ask the user to confirm an action. Use this instead of `window.confirm`:
 *  the dialog plugin replaces that with an async version, so its result is
 *  a Promise (always truthy) and `if (window.confirm(...))` never waits. */
export function confirmAction(message: string): Promise<boolean> {
  return confirm(message, { title: "JenderalCode", kind: "warning" });
}
