// SPDX-License-Identifier: GPL-3.0-or-later
// The shape of a keymap preset file (`keymaps/nle.json`, `keymaps/capcut.json`, WP-2H.05), and the
// ids bindings go by. Pure types and functions with no imports of the app: the editor reads the
// files through `model.ts`, and the conflict-check script (`scripts/check-keymaps.ts`) reads them
// with plain `node`.
import type { KeyContext } from "./commands.ts";
import type { KeyPlatform } from "./stroke.ts";

/** One binding of a preset file. */
export interface PresetBinding {
  readonly command: string;
  readonly args?: Readonly<Record<string, string>>;
  /** Spelled keys: `Mod+Shift+Z`, `Mod+K Mod+S`. The first is the one a menu shows. */
  readonly keys: readonly string[];
  /** Overrides the command's usual context. */
  readonly when?: KeyContext;
  /** Only on these platforms (Redo shows another key on macOS). */
  readonly platforms?: readonly KeyPlatform[];
  /** A best guess, to check against the program the preset imitates (notes N7). */
  readonly unverified?: boolean;
}

export interface PresetFile {
  readonly id: string;
  readonly name: string;
  readonly note?: string;
  readonly bindings: readonly PresetBinding[];
}

/** The one argument a bindable entry may fix (`workspace.switch_workspace` has `workspace`). */
const ARG_NAMES: Readonly<Record<string, string>> = {
  "workspace.switch_workspace": "workspace",
};

/** The id a binding is stored under: the command, plus its fixed argument (`command:value`). */
export function bindingId(command: string, args?: Readonly<Record<string, unknown>>): string {
  const value = args === undefined ? undefined : Object.values(args)[0];
  return value === undefined ? command : `${command}:${String(value)}`;
}

/** The command and fixed arguments a binding id names. */
export function parseBindingId(id: string): {
  command: string;
  args?: Record<string, string>;
} {
  const at = id.indexOf(":");
  if (at < 0) return { command: id };
  const command = id.slice(0, at);
  const name = ARG_NAMES[command];
  return name === undefined ? { command: id } : { command, args: { [name]: id.slice(at + 1) } };
}
