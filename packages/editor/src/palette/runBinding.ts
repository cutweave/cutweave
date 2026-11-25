// SPDX-License-Identifier: GPL-3.0-or-later
// Running a command a key is bound to (WP-2H.05): the keymap's global dispatcher hands it a
// binding, and it takes the same road choosing the command in the palette would, so a shortcut and
// the palette can never do two different things. A command that needs arguments the context cannot
// give (a form) is not run from a key; the person is told where to find it.

import { i18n } from "../i18n/index.ts";
import type { KeyBinding } from "../keymap/model.ts";
import { itemSelection } from "../menus/adapters.ts";
import { menuServices } from "../menus/defaults.ts";
import { toastStore } from "../state/toastStore.ts";
import { runWorkspaceCommand } from "../workspace/commands.ts";
import { buildEntries, type PaletteEntry, planFor, runEntry } from "./entries.ts";
import { appPaletteEnv } from "./env.ts";
import { togglePalette } from "./shortcut.ts";

const sameArgs = (
  a: Readonly<Record<string, unknown>> | undefined,
  b: Readonly<Record<string, unknown>> | undefined,
): boolean => JSON.stringify(a ?? {}) === JSON.stringify(b ?? {});

/** The palette's row for a binding. */
function entryFor(binding: KeyBinding): PaletteEntry | undefined {
  return buildEntries(appPaletteEnv()).find(
    (e) => e.command === binding.command && sameArgs(e.args, binding.args),
  );
}

/** Runs `binding`'s command; resolves when it is done and rejects with the command's own error. */
export async function runBinding(binding: KeyBinding): Promise<void> {
  if (binding.command === "palette.open") {
    togglePalette();
    return;
  }
  if (binding.command.startsWith("workspace.")) {
    runWorkspaceCommand(binding.command, binding.args ?? {});
    return;
  }
  const env = appPaletteEnv();
  const entry = entryFor(binding);
  if (entry === undefined) {
    // A UI command the palette has no row for (`app.open_settings` with no page): run it as it is.
    const ui = menuServices.ui.get(binding.command);
    const target = { surface: "keymap", ids: itemSelection.current() };
    await ui?.run(binding.args ?? {}, {
      target,
      ids: target.ids,
      call: (n, a) => menuServices.call(n, a),
    });
    return;
  }
  if (entry.disabled !== undefined) {
    toastStore.getState().push({ tone: "warning", title: entry.title, message: entry.disabled });
    return;
  }
  const plan = planFor(entry, env);
  if (plan.kind === "form") {
    toastStore.getState().push({
      tone: "warning",
      title: entry.title,
      message: i18n.t("keymap.needsArguments"),
    });
    return;
  }
  await runEntry(entry, plan.args, {
    services: menuServices,
    selection: () => itemSelection.current(),
  });
}
