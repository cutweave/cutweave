// SPDX-License-Identifier: GPL-3.0-or-later
// Turns a surface's table, the contributions to it and a resolved target into the entries the
// design system's `Menu` draws (WP-2H.12). Pure but for the callbacks it wires: nothing here
// mutates the document; a chosen row runs its command through `services.call`.

import { needsRequestId } from "@cutweave/protocol";
import type { MenuEntry, MenuItem, MenuLeaf } from "@cutweave/ui";
import type { TKey } from "../i18n/index.ts";
import { guardedCall, type RunContext } from "./commands.ts";
import { clampOrder, MAX_PLUGIN_ITEMS, ORDER_MAX } from "./registry.ts";
import type { MenuServices } from "./services.ts";
import type { LabelSpec, MenuContext, MenuRow, MenuTarget } from "./types.ts";
import { evaluateWhen } from "./when.ts";

export interface ResolvedMenu {
  /** The menu's accessible name: "Clip actions for interview_A.mov". */
  readonly name: string;
  readonly entries: readonly MenuEntry[];
  readonly context: MenuContext;
}

type Run = () => void | Promise<void>;

const TRAILING_GROUP = "contributions";

const spec = (value: LabelSpec | ((ctx: MenuContext) => LabelSpec), ctx: MenuContext): LabelSpec =>
  typeof value === "function" ? value(ctx) : value;

/** A command row resolved against the context, before it is grouped. */
interface Resolved {
  readonly group: string;
  readonly order: number;
  readonly entry: MenuLeaf | Extract<MenuEntry, { kind: "submenu" }>;
}

function resolveRow(
  services: MenuServices,
  row: MenuRow,
  ctx: MenuContext,
  choose: (run: Run) => void,
  override?: { label?: string; icon?: MenuItem["icon"] },
): Resolved | null {
  if (!evaluateWhen(row.visible, ctx.keys)) return null;
  const order = clampOrder(row.order, ORDER_MAX);

  if (row.submenu !== undefined) {
    const items: MenuLeaf[] = [];
    for (const sub of [...row.submenu.rows].sort((a, b) => a.order - b.order)) {
      const resolved = resolveRow(services, sub, ctx, choose);
      if (resolved !== null && resolved.entry.kind !== "submenu") items.push(resolved.entry);
    }
    if (items.length === 0) return null;
    const label = services.t(row.submenu.label);
    return {
      group: row.group,
      order,
      entry: { kind: "submenu", id: row.id ?? `sub-${label}`, label, items },
    };
  }

  const command = row.command;
  if (command === undefined) return null;
  const ui = services.ui.get(command);
  const info = ui === undefined ? services.commands.get(command) : undefined;
  // A command nothing has registered (its WP has not landed) is left out, not shown broken.
  if (ui === undefined && info === undefined) return null;
  const labelKey: TKey = (
    ctx.ids.length > 1 && info?.labelMany !== undefined
      ? info.labelMany
      : (ui?.label ?? info?.label)
  ) as TKey;
  const icon = override?.icon ?? ui?.icon ?? info?.icon;
  const danger = ui?.danger === true || info?.destructive === true;

  // Enabled: the row's own clause and check first (their reason is the most specific), then the
  // rules every document-changing command shares.
  let reason: TKey | undefined;
  let enabled = evaluateWhen(row.when, ctx.keys) && (row.enabledIf?.(ctx) ?? true);
  if (!enabled && row.reason !== undefined) {
    reason = typeof row.reason === "function" ? row.reason(ctx) : row.reason;
  }
  if (enabled && info !== undefined && needsRequestId(command)) {
    if (ctx.keys["project.readOnly"] === true) {
      enabled = false;
      reason = "contextMenu.reason.readOnly";
    } else if (ctx.keys["engine.live"] === false) {
      enabled = false;
      reason = "contextMenu.reason.noEngine";
    }
  }

  const label =
    override?.label ??
    services.t(
      row.label === undefined
        ? { key: labelKey, vars: { count: ctx.ids.length } }
        : spec(row.label, ctx),
    );
  const shortcut = row.noHint === true ? undefined : services.keysFor(command);
  const entry: MenuItem = {
    id: row.id ?? command,
    label,
    icon,
    shortcut,
    disabled: enabled ? undefined : true,
    disabledReason: !enabled && reason !== undefined ? services.t({ key: reason }) : undefined,
    variant: row.variant ?? (danger ? "danger" : undefined),
    ...(row.checked !== undefined && {
      checked: row.checked(ctx),
      ...(row.radio === true && { choice: "radio" as const }),
    }),
    onSelect: () => {
      choose(() => execute(services, row, ctx));
    },
  };
  return { group: row.group, order, entry };
}

async function execute(services: MenuServices, row: MenuRow, ctx: MenuContext): Promise<void> {
  const command = row.command;
  if (command === undefined) return;
  const call = guardedCall((name, args) => services.call(name, args));
  const args = row.args?.(ctx);
  const ui = services.ui.get(command);
  if (ui !== undefined) {
    const run: RunContext = { target: ctx.target, ids: ctx.ids, call };
    await ui.run(args, run);
    return;
  }
  await call(command, args ?? {});
}

/** The rows of a contribution as a table row (the command supplies the label). */
function contributionRow(c: {
  command: string;
  group?: string;
  order: number;
  when?: string;
  reason?: TKey;
  args?: MenuRow["args"];
}): MenuRow {
  return {
    command: c.command,
    group: c.group ?? TRAILING_GROUP,
    order: c.order,
    ...(c.when !== undefined && { when: c.when }),
    ...(c.reason !== undefined && { reason: c.reason }),
    ...(c.args !== undefined && { args: c.args }),
  };
}

/**
 * The menu for `target`, or `null` when its surface has no table (a right-click there shows
 * nothing). `choose` receives the work of the chosen row so the host can run it once the menu
 * has closed and the focus is back.
 */
export function buildMenu(
  services: MenuServices,
  target: MenuTarget,
  choose: (run: Run) => void,
): ResolvedMenu | null {
  const def = services.surfaces.get(target.surface);
  if (def === undefined) return null;
  const keys = {
    ...services.baseKeys(),
    "selection.count": target.ids.length,
    ...def.keys?.(target),
    ...target.keys,
  };
  const ctx: MenuContext = { target, ids: target.ids, keys, playhead: services.playhead() };

  const groups = new Map<string, Resolved[]>();
  const groupOrder = [...def.groups];
  const add = (resolved: Resolved | null): void => {
    if (resolved === null) return;
    if (!groups.has(resolved.group)) groups.set(resolved.group, []);
    groups.get(resolved.group)?.push(resolved);
    if (!groupOrder.includes(resolved.group)) groupOrder.push(resolved.group);
  };
  for (const row of def.rows) add(resolveRow(services, row, ctx, choose));

  // First-party contributions join the groups they name; plugin items are gathered below.
  const byPlugin = new Map<string, { name: string; items: Resolved[] }>();
  for (const c of services.contributions.forSurface(target.surface)) {
    const resolved = resolveRow(services, contributionRow(c), ctx, choose, {
      ...(c.label !== undefined && { label: c.label }),
      ...(c.icon !== undefined && { icon: c.icon }),
    });
    if (resolved === null) continue;
    if (c.plugin === undefined) {
      add(resolved);
      continue;
    }
    const slot = byPlugin.get(c.plugin.id) ?? { name: c.plugin.name, items: [] };
    slot.items.push({ ...resolved, group: TRAILING_GROUP, order: c.order });
    byPlugin.set(c.plugin.id, slot);
  }

  const trailing: MenuEntry[] = [];
  for (const [pluginId, { name, items }] of byPlugin) {
    const leaves = items
      .sort((a, b) => a.order - b.order)
      .map((r) => r.entry)
      .filter((e): e is MenuLeaf => e.kind !== "submenu");
    trailing.push(...leaves.slice(0, MAX_PLUGIN_ITEMS));
    if (leaves.length > MAX_PLUGIN_ITEMS) {
      trailing.push({
        kind: "submenu",
        id: `plugin-${pluginId}`,
        label: name,
        items: leaves.slice(MAX_PLUGIN_ITEMS),
      });
    }
  }

  const entries: MenuEntry[] = [];
  let first = true;
  const push = (rows: readonly MenuEntry[], id: string): void => {
    if (rows.length === 0) return;
    if (!first) entries.push({ kind: "separator", id: `sep-${id}` });
    first = false;
    entries.push(...rows);
  };
  for (const group of groupOrder) {
    const rows = (groups.get(group) ?? [])
      .map((r, i) => ({ r, i }))
      .sort((a, b) => a.r.order - b.r.order || a.i - b.i)
      .map(({ r }) => r.entry);
    push(rows, group);
  }
  push(trailing, "plugins");

  if (entries.length === 0) return null;
  return { name: services.t(def.name(target)), entries, context: ctx };
}
