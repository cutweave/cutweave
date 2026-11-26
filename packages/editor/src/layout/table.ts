// SPDX-License-Identifier: GPL-3.0-or-later
// The layout table of `item.apply_layout` (WP-2C.08), read from the same file the engine embeds
// (`crates/cutweave-ops/data/layouts.json`), so the diagrams and the menus never drift from what
// the command does. The engine computes every placement; this file only draws the cells.
import type { LayoutCell } from "@cutweave/ui";
import raw from "../../../../crates/cutweave-ops/data/layouts.json";

interface GridCell {
  readonly col: number;
  readonly row: number;
  readonly col_span?: number;
  readonly row_span?: number;
}

interface RectCell {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

interface RawLayout {
  readonly id: string;
  readonly label: string;
  readonly columns?: readonly number[];
  readonly rows?: readonly number[];
  readonly cells: readonly (GridCell | RectCell)[];
}

export interface LayoutDef {
  /** The ID `item.apply_layout` takes. */
  readonly id: string;
  /** English name (the editor shows the translated one). */
  readonly label: string;
  /** How many video clips it takes. */
  readonly count: number;
  /** The cells in fractions of the canvas at gap 0, in the order clips fill them. */
  readonly cells: readonly LayoutCell[];
}

/** Start and end of each track of a grid, as fractions of the canvas. */
function tracks(weights: readonly number[]): [number, number][] {
  const sum = weights.reduce((a, b) => a + b, 0);
  let at = 0;
  return weights.map((w): [number, number] => {
    const span: [number, number] = [at / sum, (at + w) / sum];
    at += w;
    return span;
  });
}

function cellsOf(layout: RawLayout): LayoutCell[] {
  const cols = tracks(layout.columns ?? [1]);
  const rows = tracks(layout.rows ?? [1]);
  return layout.cells.map((cell) => {
    if ("col" in cell) {
      const c = Math.min(cell.col, cols.length - 1);
      const r = Math.min(cell.row, rows.length - 1);
      const lastCol = cols[Math.min(c + (cell.col_span ?? 1) - 1, cols.length - 1)];
      const lastRow = rows[Math.min(r + (cell.row_span ?? 1) - 1, rows.length - 1)];
      const x0 = cols[c]?.[0] ?? 0;
      const y0 = rows[r]?.[0] ?? 0;
      return { x: x0, y: y0, w: (lastCol?.[1] ?? 1) - x0, h: (lastRow?.[1] ?? 1) - y0 };
    }
    return { x: cell.x, y: cell.y, w: cell.w, h: cell.h };
  });
}

/** Every layout of the table, in table order. */
export const LAYOUTS: readonly LayoutDef[] = (
  raw as unknown as { readonly layouts: readonly RawLayout[] }
).layouts.map((l) => {
  const cells = cellsOf(l);
  return { id: l.id, label: l.label, count: cells.length, cells };
});

/** The layouts for exactly `count` clips. */
export const layoutsFor = (count: number): readonly LayoutDef[] =>
  LAYOUTS.filter((l) => l.count === count);

export const findLayout = (id: string): LayoutDef | undefined => LAYOUTS.find((l) => l.id === id);

/** The layout "Place side by side" uses for `count` clips: side by side, columns, or a grid. */
export const SIDE_BY_SIDE: Readonly<Record<number, string>> = {
  2: "side_by_side",
  3: "columns_3",
  4: "grid_2x2",
};
