// SPDX-License-Identifier: GPL-3.0-or-later
// Keys no command can be bound to (WP-2H.05), each with the reason the editor shows. They belong
// to the menu bar, the window, the text fields, the dialogs or the context-menu host; taking
// them would break something the person relies on. The presets may use some of them for the
// handlers that own them (Esc cancels a drag); only a person's own assignment is checked.
import type { TKey } from "../i18n/index.ts";
import { type KeyCombo, type KeyPlatform, strokeKey } from "./stroke.ts";

/** Why a key cannot be bound: a catalog key the page shows. */
export type ReservedReason = Extract<TKey, `keymap.reserved.${string}`>;

/** The menu bar's first letters (File, Edit, View, Help): its Alt access keys. */
const MENU_LETTERS = ["F", "E", "V", "H"] as const;

interface Rule {
  readonly reason: ReservedReason;
  /** Spelled strokes (see `stroke.ts`) the rule holds for, on every platform. */
  readonly keys: readonly string[];
  readonly only?: readonly KeyPlatform[];
}

const RULES: readonly Rule[] = [
  { reason: "keymap.reserved.menuBar", keys: ["F10"], only: ["windows", "linux"] },
  {
    reason: "keymap.reserved.menuBar",
    keys: MENU_LETTERS.map((letter) => `Alt+${letter}`),
    only: ["windows", "linux"],
  },
  // Shift+F10 and the Menu key open the context menu of whatever has the focus (WP-2H.12).
  { reason: "keymap.reserved.contextMenu", keys: ["Shift+F10", "ContextMenu"] },
  { reason: "keymap.reserved.closeWindow", keys: ["Alt+F4"], only: ["windows", "linux"] },
  {
    reason: "keymap.reserved.clipboard",
    keys: ["Mod+C", "Mod+X", "Mod+V", "Mod+A"],
  },
  { reason: "keymap.reserved.dialogs", keys: ["Escape", "Enter", "Tab", "Shift+Tab"] },
];

/**
 * Why `combo` (its first stroke, and its second when it is a chord) cannot be bound, or `null`.
 * A chord is checked stroke by stroke, so `Mod+K Escape` is refused too.
 */
export function reservedReason(combo: KeyCombo, platform: KeyPlatform): ReservedReason | null {
  for (let stroke: KeyCombo | undefined = combo; stroke !== undefined; stroke = stroke.next) {
    const spelled = strokeKey(stroke);
    for (const rule of RULES) {
      if (rule.only !== undefined && !rule.only.includes(platform)) continue;
      if (rule.keys.includes(spelled)) return rule.reason;
    }
  }
  return null;
}
