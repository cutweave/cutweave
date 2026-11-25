// SPDX-License-Identifier: GPL-3.0-or-later
// The preset check (WP-2H.05): a preset file must load with no conflicts, bind only commands that
// exist, spell every key so it parses, and cover enough commands to be a real preset. The Vitest
// suite runs it on both presets and `scripts/check-keymaps.ts` runs it on the files from the
// command line (`pnpm --filter @cutweave/editor check:keymaps`). Pure: no imports of the app.
import { contextsOverlap, KEY_COMMANDS, type KeyContext, keyCommand } from "./commands.ts";
import { bindingId, type PresetFile } from "./preset.ts";
import { type KeyCombo, type KeyPlatform, parseKeys, spell, strokeKey } from "./stroke.ts";

/** The fewest commands a preset binds. */
export const MIN_PRESET_COMMANDS = 30;

export const CHECKED_PLATFORMS: readonly KeyPlatform[] = ["windows", "linux", "mac"];

/** A command the editor has beyond the table: a panel's toggle, a workspace command. */
const EXTRA_COMMAND = /^workspace\.(?:toggle_panel\.[a-z_]+|reset_layout|reset_all|reopen_panels)$/;

export interface PresetProblem {
  readonly preset: string;
  readonly platform: KeyPlatform;
  readonly message: string;
}

interface Placed {
  readonly id: string;
  readonly command: string;
  readonly context: KeyContext;
  readonly fixed: boolean;
  readonly key: KeyCombo;
}

const clash = (a: KeyCombo, b: KeyCombo): boolean =>
  strokeKey(a) === strokeKey(b) &&
  (a.next === undefined || b.next === undefined || spell(a.next) === spell(b.next));

/** Everything wrong with `file` on `platform`; an empty list is a preset that loads. */
export function checkPreset(
  file: PresetFile,
  platform: KeyPlatform,
  isKnown: (command: string) => boolean = (c) =>
    keyCommand(c) !== undefined || EXTRA_COMMAND.test(c),
): PresetProblem[] {
  const problems: PresetProblem[] = [];
  const say = (message: string): void => {
    problems.push({ preset: file.id, platform, message });
  };
  const seen = new Set<string>();
  const placed: Placed[] = [];
  for (const entry of file.bindings) {
    if (entry.platforms !== undefined && !entry.platforms.includes(platform)) continue;
    const id = bindingId(entry.command, entry.args);
    if (!isKnown(entry.command)) say(`${id} is not a command the editor has`);
    if (seen.has(id)) say(`${id} is bound twice`);
    seen.add(id);
    const def = keyCommand(entry.command);
    const context = entry.when ?? def?.context ?? "global";
    for (const text of entry.keys) {
      const key = parseKeys(text, platform);
      if (key === null) {
        say(`${id}: "${text}" is not a key`);
        continue;
      }
      if (spell(key) !== text) say(`${id}: "${text}" should be spelled "${spell(key)}"`);
      placed.push({ id, command: entry.command, context, fixed: def?.fixed === true, key });
    }
  }
  for (let i = 0; i < placed.length; i++) {
    for (let j = i + 1; j < placed.length; j++) {
      const a = placed[i] as Placed;
      const b = placed[j] as Placed;
      // The text fields' and dialogs' own keys are not the keymap's to dispatch.
      if (a.fixed || b.fixed || a.id === b.id) continue;
      if (contextsOverlap(a.context, b.context) && clash(a.key, b.key)) {
        say(`${a.id} and ${b.id} both use ${spell(a.key)}`);
      }
    }
  }
  const commands = new Set(placed.map((p) => p.command));
  if (commands.size < MIN_PRESET_COMMANDS) {
    say(`binds ${commands.size} commands; a preset needs at least ${MIN_PRESET_COMMANDS}`);
  }
  // Every command the table calls bindable should have a key in a preset that claims it.
  return problems;
}

/** The commands of the table that a preset leaves with no key (informational). */
export function unboundCommands(file: PresetFile, platform: KeyPlatform): string[] {
  const bound = new Set(
    file.bindings
      .filter((b) => b.platforms === undefined || b.platforms.includes(platform))
      .map((b) => b.command),
  );
  return KEY_COMMANDS.filter((c) => c.devOnly !== true && !bound.has(c.id)).map((c) => c.id);
}
