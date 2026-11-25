// SPDX-License-Identifier: GPL-3.0-or-later
import { i18n } from "../i18n/index.ts";
import type { PanelDef } from "./panelRegistry.ts";

/** The id of the panel tab component registered in `Dock` (title, close button). */
export const PANEL_TAB_COMPONENT = "header";

/** Panel options shared by every way a panel is added to the dock. */
export function panelOptions(def: PanelDef) {
  const min = def.minSize?.() ?? {};
  return {
    id: def.id,
    component: def.id,
    tabComponent: PANEL_TAB_COMPONENT,
    title: i18n.t(def.title),
    params: { titleKey: def.title },
    ...(min.width !== undefined ? { minimumWidth: min.width } : {}),
    ...(min.height !== undefined ? { minimumHeight: min.height } : {}),
  };
}
