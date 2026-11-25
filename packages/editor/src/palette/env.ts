// SPDX-License-Identifier: GPL-3.0-or-later
// The palette's view of the running app: the context keys `when` clauses read, what the context
// can fill in an argument form, and the sources of the list (WP-2H.04).
import { itemSelection } from "../menus/adapters.ts";
import { menuServices } from "../menus/defaults.ts";
import type { MenuServices } from "../menus/services.ts";
import type { ContextValues } from "../menus/when.ts";
import { settingsPages } from "../settings/registry.ts";
import { docStore } from "../state/docStore.ts";
import { selectTimelineItems } from "../state/selectors.ts";
import { viewStore } from "../state/viewStore.ts";
import { listWorkspaceCommands } from "../workspace/commands.ts";
import { registeredPanels } from "../workspace/panelRegistry.ts";
import type { Prefill } from "./argForm.ts";
import { commandCatalog } from "./catalog.ts";
import type { PaletteEnv } from "./entries.ts";

const KIND_NAMES: Readonly<Record<string, string>> = { media: "video" };

/** The context keys at this moment, as a menu opening on the timeline would have them. */
export function paletteKeys(services: MenuServices): ContextValues {
  const ids = itemSelection.current();
  const items = selectTimelineItems(docStore.getState().project);
  const playhead = viewStore.getState().playhead;
  const selected = new Set(ids);
  const chosen = ids.length > 0 ? items.filter((i) => selected.has(i.id)) : items;
  const kinds = new Set(
    chosen.filter((i) => selected.has(i.id)).map((i) => KIND_NAMES[i.kind] ?? i.kind),
  );
  return {
    ...services.baseKeys(),
    "selection.count": ids.length,
    ...(ids.length > 0 && { "selection.kind": kinds.size === 1 ? [...kinds][0] : "mixed" }),
    "playhead.inItem": chosen.some((i) => i.start <= playhead && playhead < i.start + i.duration),
  };
}

/** The app's palette environment. */
export function appPaletteEnv(services: MenuServices = menuServices): PaletteEnv {
  return {
    services,
    catalog: () => commandCatalog.getState().commands,
    panels: registeredPanels,
    workspaceCommands: listWorkspaceCommands,
    settingsPages,
    keys: () => paletteKeys(services),
    prefill: (): Prefill => ({ items: itemSelection.current(), playhead: services.playhead() }),
  };
}
