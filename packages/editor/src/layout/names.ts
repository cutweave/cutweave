// SPDX-License-Identifier: GPL-3.0-or-later
import type { LayoutOption } from "@cutweave/ui";
import type { TKey } from "../i18n/index.ts";
import { layoutsFor } from "./table.ts";

/** The words for a layout by its table ID. A layout a pack adds shows its table label. */
export const LAYOUT_NAMES: Readonly<Record<string, TKey>> = {
  side_by_side: "layout.names.sideBySide",
  top_bottom: "layout.names.topBottom",
  pip_top_left: "layout.names.pipTopLeft",
  pip_top_right: "layout.names.pipTopRight",
  pip_bottom_left: "layout.names.pipBottomLeft",
  pip_bottom_right: "layout.names.pipBottomRight",
  columns_3: "layout.names.columns3",
  one_large_two_small: "layout.names.oneLargeTwoSmall",
  grid_2x2: "layout.names.grid2x2",
};

/** The picker's options for `count` clips, named with `t`. */
export function layoutOptions(count: number, t: (key: TKey) => string): LayoutOption[] {
  return layoutsFor(count).map((l) => {
    const key = LAYOUT_NAMES[l.id];
    return { id: l.id, label: key === undefined ? l.label : t(key), cells: l.cells };
  });
}
