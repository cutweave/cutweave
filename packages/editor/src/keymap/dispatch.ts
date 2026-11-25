// SPDX-License-Identifier: GPL-3.0-or-later
// The keymap's window-wide dispatcher (WP-2H.05): every `global` binding no other listener owns
// (a panel toggle, the command palette, any palette command a person bound to a key) runs from
// here, with chords (`Mod+K Mod+S`), IME safety and the text-field rule: a plain key typed in a
// field is the field's. The menu bar's own commands stay with `useAppShortcuts` (they follow the
// menu items' enabled state), the transport's with its keymap, the timeline's, the bin's and the
// Source view's with their panels; all of them read the same keymap.
import { useEffect } from "react";
import { i18n } from "../i18n/index.ts";
import { runBinding } from "../palette/runBinding.ts";
import { isTextEntry } from "../session/useProject.ts";
import { toastStore } from "../state/toastStore.ts";
import { KEY_COMMANDS } from "./commands.ts";
import type { KeyBinding } from "./model.ts";
import { chordsStartingWith, completeChord, isComposing, matchEvent, strokeOf } from "./store.ts";
import type { KeyCombo } from "./stroke.ts";

/** How long a chord waits for its second stroke. */
export const CHORD_TIMEOUT_MS = 1500;

/**
 * The commands another listener runs from their key: the menu bar's (they follow the menu items'
 * enabled state), the transport's, the bin's. Only the palette's key is the dispatcher's own.
 */
const OWNED: ReadonlySet<string> = new Set(
  KEY_COMMANDS.map((c) => c.id).filter((id) => id !== "palette.open"),
);

/** Whether a stroke is typing: no modifier and not a function key. */
function isPlainTyping(stroke: KeyCombo): boolean {
  return (
    stroke.mod !== true &&
    stroke.ctrl !== true &&
    stroke.alt !== true &&
    !/^F\d{1,2}$/.test(stroke.key)
  );
}

const report = (error: unknown): void => {
  toastStore.getState().push({
    tone: "danger",
    title: i18n.t("contextMenu.failed"),
    message: error instanceof Error ? error.message : String(error),
  });
};

function run(binding: KeyBinding): void {
  void runBinding(binding).catch(report);
}

/** Mount once (the shell): runs the global bindings no other listener owns. */
export function useKeymapDispatcher(): void {
  useEffect(() => {
    let pending: { first: KeyCombo; at: number } | null = null;

    // A chord's second stroke is taken before any other listener sees it (it would be Save).
    const onSecond = (event: KeyboardEvent): void => {
      if (pending === null || isComposing(event)) return;
      const stroke = strokeOf(event);
      if (stroke === null) return;
      const { first, at } = pending;
      pending = null;
      if (event.timeStamp - at > CHORD_TIMEOUT_MS) return;
      event.preventDefault();
      event.stopPropagation();
      if (stroke.key === "Escape" && stroke.mod !== true) return;
      const done = completeChord(first, stroke, ["global"]);
      if (done !== null) run(done);
    };

    const onKey = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || event.repeat || isComposing(event)) return;
      const stroke = strokeOf(event);
      if (stroke === null) return;
      if (isTextEntry(event.target) && isPlainTyping(stroke)) return;
      if (chordsStartingWith(stroke, ["global"]).length > 0) {
        event.preventDefault();
        pending = { first: stroke, at: event.timeStamp };
        return;
      }
      const binding = matchEvent(event, ["global"]);
      if (binding === null || binding.fixed || OWNED.has(binding.command)) return;
      event.preventDefault();
      run(binding);
    };

    window.addEventListener("keydown", onSecond, true);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onSecond, true);
      window.removeEventListener("keydown", onKey);
    };
  }, []);
}
