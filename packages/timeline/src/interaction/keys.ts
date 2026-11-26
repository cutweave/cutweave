// SPDX-License-Identifier: GPL-3.0-or-later
// The machine's default keys. The editor's keymap (WP-2H.05) owns the keys and tells the machine
// which action a press is (`key.down.action`); this table answers only when the host does not.
import type { MachineAction, Modifiers } from "./types.ts";

/** One default binding: a `KeyboardEvent.key` (letters match either case) and whether Shift. */
export interface MachineKey {
  readonly action: MachineAction;
  readonly key: string;
  readonly shift?: boolean;
}

export const MACHINE_KEYS: readonly MachineKey[] = [
  { action: "cancel", key: "Escape" },
  { action: "split", key: "s" },
  { action: "delete", key: "Delete" },
  { action: "delete", key: "Backspace" },
  { action: "ripple_delete", key: "Delete", shift: true },
  { action: "ripple_delete", key: "Backspace", shift: true },
  { action: "toggle_snapping", key: "n" },
  { action: "nudge_left", key: "," },
  { action: "nudge_right", key: "." },
  { action: "nudge_left_10", key: "<", shift: true },
  { action: "nudge_left_10", key: ",", shift: true },
  { action: "nudge_right_10", key: ">", shift: true },
  { action: "nudge_right_10", key: ".", shift: true },
  { action: "tool.select", key: "v" },
  { action: "tool.ripple", key: "b" },
  { action: "tool.roll", key: "r" },
  { action: "tool.slip", key: "y" },
  { action: "tool.slide", key: "u" },
  { action: "tool.razor", key: "c" },
  { action: "tool.hand", key: "h" },
  { action: "tool.zoom", key: "z" },
];

/** The keys that are only modifiers: they change how a drag behaves, never act on their own. */
export const MODIFIER_KEYS: ReadonlySet<string> = new Set(["Shift", "Alt", "Control", "Meta"]);

/** The default action of a key press, or null. Ctrl, Cmd and Alt combinations have none. */
export function machineKeyAction(key: string, mods: Modifiers = {}): MachineAction | null {
  if (key === "Escape") return "cancel";
  if (mods.ctrl === true || mods.meta === true || mods.alt === true) return null;
  const k = key.length === 1 ? key.toLowerCase() : key;
  const shift = mods.shift === true;
  const hit = MACHINE_KEYS.find((b) => b.key === k && (b.shift === true) === shift);
  return hit?.action ?? null;
}
