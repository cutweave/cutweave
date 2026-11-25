// SPDX-License-Identifier: GPL-3.0-or-later
// What the palette lists and what choosing a row does (WP-2H.04). One list, built from the same
// sources as the menus and the keymap: the engine catalog (every non-hidden command), the UI
// commands (`UiCommandTable`), the workspace commands and panels (WP-2H.01) and the settings
// pages and rows (WP-1M.11). A row's `command` and `args` are the stable pair the shortcut
// editor (WP-2H.05) binds, so the palette's list is the bindable list.
//
// `when` clauses read the context keys menus already use (`menus/when.ts`); a row whose clause
// does not hold is left out. A mutating command is shown but disabled, with the reason, while
// the engine is down or the project is read-only.
import { needsRequestId } from "@cutweave/protocol";
import type { TKey } from "../i18n/index.ts";
import { guardedCall } from "../menus/commands.ts";
import type { MenuServices } from "../menus/services.ts";
import type { LabelSpec, MenuTarget } from "../menus/types.ts";
import { type ContextValues, evaluateWhen } from "../menus/when.ts";
import type { SettingsPageDef } from "../settings/registry.ts";
import { WORKSPACES } from "../state/viewStore.ts";
import type { WorkspaceCommand } from "../workspace/commands.ts";
import { runWorkspaceCommand, TOGGLE_PANEL_PREFIX } from "../workspace/commands.ts";
import type { PanelDef } from "../workspace/panelRegistry.ts";
import { paletteAction } from "./actions.ts";
import { fullyPrefilled, needsArguments, type Prefill, prefillFor } from "./argForm.ts";
import { type CatalogCommand, type JsonSchema, listedCatalog } from "./catalog.ts";
import { type Searchable, searchable } from "./search.ts";

export type EntryKind = "command" | "ui" | "workspace" | "panel" | "settings";

export interface PaletteEntry {
  /** Unique in the list: the command, plus its fixed arguments when it has them. */
  readonly key: string;
  /** The stable command id (`clip.split`, `workspace.reset_layout`): what a shortcut binds. */
  readonly command: string;
  /** Arguments the entry fixes (a workspace's id, a settings page); not the form's. */
  readonly args?: Readonly<Record<string, unknown>>;
  readonly kind: EntryKind;
  readonly title: string;
  readonly summary?: string;
  /** Small labels after the title: the command's namespace, "Panel", "Settings". */
  readonly chips: readonly string[];
  /** The bound shortcut as text, when the keymap has one. */
  readonly shortcut?: string;
  /** Why the row cannot run now; it is shown and not chosen. */
  readonly disabled?: string;
  /** Arguments the person gives in a form, when the command has required ones. */
  readonly schema?: JsonSchema;
  readonly search: Searchable;
  /** The catalog entry behind a `command` row (its `undoable`, `destructive`). */
  readonly catalog?: CatalogCommand;
}

/** What the list is built from; tests and the app each give one. */
export interface PaletteEnv {
  readonly services: MenuServices;
  readonly catalog: () => readonly CatalogCommand[];
  readonly panels: () => readonly PanelDef[];
  readonly workspaceCommands: () => readonly WorkspaceCommand[];
  readonly settingsPages: () => readonly SettingsPageDef[];
  /** The context keys now (selection, playhead, engine). */
  readonly keys: () => ContextValues;
  readonly prefill: () => Prefill;
}

const keyOf = (command: string, args?: Readonly<Record<string, unknown>>): string =>
  args === undefined ? command : `${command}#${JSON.stringify(args)}`;

const spec = (key: TKey, vars?: Record<string, string | number>): LabelSpec =>
  vars === undefined ? { key } : { key, vars };

/** Every row the palette lists in this context, in the order it shows with no query. */
export function buildEntries(env: PaletteEnv): PaletteEntry[] {
  const { services } = env;
  const t = (key: TKey, vars?: Record<string, string | number>): string =>
    services.t(spec(key, vars));
  const keys = env.keys();
  const out: PaletteEntry[] = [];

  const add = (e: Omit<PaletteEntry, "search" | "key"> & { key?: string }): void => {
    const key = e.key ?? keyOf(e.command, e.args);
    out.push({
      ...e,
      key,
      search: searchable(e.title, [...e.chips, e.summary ?? "", e.command, e.shortcut ?? ""]),
    });
  };

  // Engine commands: the catalog, with the editor's own label when it has one.
  const taken = new Set<string>();
  for (const command of listedCatalog(env.catalog())) {
    const info = services.commands.get(command.name);
    if (!evaluateWhen(info?.when, keys)) continue;
    taken.add(command.name);
    let disabled: string | undefined;
    if (needsRequestId(command.name)) {
      if (keys["project.readOnly"] === true) disabled = t("contextMenu.reason.readOnly");
      else if (keys["engine.live"] === false) disabled = t("contextMenu.reason.noEngine");
    }
    const shortcut = services.keysFor(command.name);
    add({
      command: command.name,
      kind: "command",
      title: info === undefined ? command.summary : t(info.label, { count: 1 }),
      ...(info !== undefined && { summary: command.summary }),
      chips: [command.namespace],
      ...(shortcut !== undefined && { shortcut }),
      ...(disabled !== undefined && { disabled }),
      ...(needsArguments(command.inputSchema) && { schema: command.inputSchema }),
      catalog: command,
    });
  }

  // Editor-side commands in the same shape (menus, later the keymap).
  for (const ui of services.ui.list()) {
    if (ui.hidden === true || taken.has(ui.id) || !evaluateWhen(ui.when, keys)) continue;
    const shortcut = services.keysFor(ui.id);
    add({
      command: ui.id,
      kind: "ui",
      title: t(ui.label),
      chips: [ui.id.split(".")[0] ?? ui.id],
      ...(shortcut !== undefined && { shortcut }),
    });
  }

  // Workspaces and panels (WP-2H.01).
  const panelDefs = new Map(env.panels().map((p) => [p.id, p]));
  for (const c of env.workspaceCommands()) {
    if (c.id.startsWith(TOGGLE_PANEL_PREFIX)) {
      const panel = c.panel === undefined ? undefined : panelDefs.get(c.panel);
      if (panel === undefined) continue;
      add({
        command: c.id,
        kind: "panel",
        title: t("workspace.commands.togglePanelNamed", { name: t(panel.title) }),
        chips: [t("palette.chip.panel")],
      });
    } else if (c.id === "workspace.switch_workspace") {
      for (const id of WORKSPACES) {
        add({
          command: c.id,
          args: { workspace: id },
          kind: "workspace",
          title: t("palette.switchTo", { name: t(`workspace.${id}`) }),
          chips: [t("palette.chip.workspace")],
        });
      }
    } else if (c.panel === undefined && !/^workspace\.(open|close|toggle)_panel$/.test(c.id)) {
      // The per-panel toggles above are the way to open or close one panel; the three
      // argument-taking forms of the same commands are for scripts.
      add({
        command: c.id,
        kind: "workspace",
        title: t(c.title),
        chips: [t("palette.chip.workspace")],
      });
    }
  }

  // Settings pages and rows (WP-1M.11), opened at the row.
  for (const page of env.settingsPages()) {
    const pageTitle = t(page.title);
    add({
      command: "app.open_settings",
      args: { page: page.id },
      kind: "settings",
      title: t("palette.settingsPage", { page: pageTitle }),
      chips: [t("palette.chip.settings")],
    });
    for (const row of page.rows) {
      add({
        command: "app.open_settings",
        args: { page: page.id, row: row.id },
        kind: "settings",
        title: t(row.label),
        summary: t(row.description),
        chips: [t("palette.chip.settings"), pageTitle],
      });
    }
  }
  return out;
}

/** What running a row needs from the editor. */
export interface RunEnv {
  readonly services: MenuServices;
  readonly selection: () => readonly string[];
}

/** A row's form: the fields' starting values come from the context. */
export interface FormPlan {
  readonly schema: JsonSchema;
  readonly prefilled: Readonly<Record<string, unknown>>;
}

/**
 * How choosing a row goes on: `run` when it can run at once, `form` when the person has to give
 * arguments first (the form starts with what the context answers).
 */
export function planFor(
  entry: PaletteEntry,
  env: Pick<PaletteEnv, "prefill">,
):
  | { readonly kind: "run"; readonly args: Record<string, unknown> }
  | { readonly kind: "form"; readonly plan: FormPlan } {
  const fixed = { ...(entry.args ?? {}) };
  if (entry.schema === undefined) return { kind: "run", args: fixed };
  const prefilled = prefillFor(entry.schema, env.prefill());
  // A command the editor carries out itself (Split at playhead) reads its own context.
  if (paletteAction(entry.command) !== undefined || fullyPrefilled(entry.schema, prefilled)) {
    return { kind: "run", args: { ...prefilled, ...fixed } };
  }
  return { kind: "form", plan: { schema: entry.schema, prefilled } };
}

/** Runs a row with its arguments. Rejects with the command's own error. */
export async function runEntry(
  entry: PaletteEntry,
  args: Record<string, unknown>,
  env: RunEnv,
): Promise<void> {
  const { services } = env;
  switch (entry.kind) {
    case "panel":
    case "workspace":
      runWorkspaceCommand(entry.command, args);
      return;
    case "ui": {
      const command = services.ui.get(entry.command);
      if (command === undefined) return;
      const target: MenuTarget = { surface: "palette", ids: env.selection() };
      await command.run(args, {
        target,
        ids: target.ids,
        call: guardedCall((name, a) => services.call(name, a)),
      });
      return;
    }
    case "settings": {
      const command = services.ui.get(entry.command);
      const target: MenuTarget = { surface: "palette", ids: [] };
      await command?.run(args, { target, ids: [], call: (name, a) => services.call(name, a) });
      return;
    }
    case "command": {
      const action = paletteAction(entry.command);
      if (action !== undefined) {
        await action(args);
        return;
      }
      await guardedCall((name, a) => services.call(name, a))(entry.command, args);
    }
  }
}
