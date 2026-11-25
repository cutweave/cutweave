// SPDX-License-Identifier: GPL-3.0-or-later
// How the editor carries out a registry command its own way (WP-2H.04). Most catalog commands
// the palette runs go straight to the engine with the arguments of the form. A few have an
// editor-side behaviour a bare call would lose: Split at playhead reads the selection and the
// playhead as the timeline's `S` does, Undo and Redo keep the session's busy rules, the project
// commands open their dialogs. Each of those registers an action under the command's name; the
// command is still the registered one, so menus, MCP and the CLI keep their own way in.

export type PaletteAction = (args: Readonly<Record<string, unknown>>) => void | Promise<void>;

const actions = new Map<string, PaletteAction>();

/** Carries out `command` with `action` when the palette runs it. Returns the unregister function. */
export function registerPaletteAction(command: string, action: PaletteAction): () => void {
  actions.set(command, action);
  return () => {
    if (actions.get(command) === action) actions.delete(command);
  };
}

export function paletteAction(command: string): PaletteAction | undefined {
  return actions.get(command);
}
