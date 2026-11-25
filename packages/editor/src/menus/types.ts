// SPDX-License-Identifier: GPL-3.0-or-later
// Shapes shared by the context-menu framework (WP-2H.12, ADR-0026). A surface is a named place a
// right-click can land; its menu is data (rows that name commands), built when it opens.
import type { IconName } from "@cutweave/ui";
import type { TKey } from "../i18n/index.ts";
import type { ContextValues } from "./when.ts";

/** A viewport point in CSS pixels. */
export interface ScreenPoint {
  readonly x: number;
  readonly y: number;
}

/** How the menu was asked for. */
export type OpenVia = "pointer" | "keyboard" | "touch";

/**
 * What the menu acts on, resolved once when it opens (by IDs, never indexes: ADR-0004). The
 * caller names what was hit; the host applies the selection rules and fills `ids` with what the
 * menu really acts on (the whole selection when the hit was part of it).
 */
export interface MenuTarget {
  readonly surface: string;
  /** What was hit: a clip, a card, a track. Empty for a place (a lane gap, the bin background). */
  readonly ids: readonly string[];
  /** Where on the surface: a timeline time and track, for "Move playhead here" style items. */
  readonly place?: { readonly time?: number; readonly trackId?: string };
  /** What to call the target in the menu's name: "interview_A.mov". */
  readonly name?: string;
  /** Surface-specific facts a row's `args` or `enabledIf` reads (the edge of a trim handle...). */
  readonly data?: unknown;
  /** Context keys the surface knows about the target (`track.locked`, `item.linked`...). */
  readonly keys?: ContextValues;
}

/** What a row's callbacks read. */
export interface MenuContext {
  readonly target: MenuTarget;
  /** What the menu acts on: the hit item, or the whole selection when the hit was in it. */
  readonly ids: readonly string[];
  readonly keys: ContextValues;
  /** The playhead, in flicks, when the editor has one. */
  readonly playhead: number | null;
}

/** A row's label: a catalog key, with variables (`count` picks the plural). */
export interface LabelSpec {
  readonly key: TKey;
  readonly vars?: Readonly<Record<string, string | number>>;
}

/**
 * One row of a surface's table, as data. `command` is a catalog command (`clip.split`) or a UI
 * command (`workspace.close_panel`); label, icon, destructive and undoable come from it, the
 * shortcut hint from the keymap. A row with `submenu` has no command of its own.
 */
export interface MenuRow {
  /** Distinguishes two rows of one command (Mute and Solo are both `track.set`). */
  readonly id?: string;
  readonly command?: string;
  readonly group: string;
  /** Within the group, ascending. Clamped to 0..999. */
  readonly order: number;
  /** The row is enabled while this holds (see `when.ts`); no clause: always. */
  readonly when?: string;
  /** The row is left out while this does not hold (it makes no sense for the target's kind). */
  readonly visible?: string;
  /** For first-party tables only: a check the key list cannot say. */
  readonly enabledIf?: (ctx: MenuContext) => boolean;
  /** Shown when the row is disabled: one line, "Track is locked". */
  readonly reason?: TKey | ((ctx: MenuContext) => TKey);
  readonly args?: (ctx: MenuContext) => unknown;
  /** Overrides the command's label, to name the object ("Delete 3 clips"). */
  readonly label?: LabelSpec | ((ctx: MenuContext) => LabelSpec);
  /** The row does something else with the command (Append is an Insert elsewhere): no key hint. */
  readonly noHint?: boolean;
  /** A check item (Mute, Lock): the row shows its state. */
  readonly checked?: (ctx: MenuContext) => boolean;
  /** With `checked`: one of a group of choices (Sort by, Quality), shown as a radio. */
  readonly radio?: boolean;
  readonly variant?: "danger" | "agent";
  /** A nested list of command rows (one level). */
  readonly submenu?: { readonly label: LabelSpec; readonly rows: readonly MenuRow[] };
}

/** How a surface reads and sets the selection, so the host can apply the target rules. */
export interface SelectionAdapter {
  /** The selected IDs of this surface's kind, in selection order. */
  current(): readonly string[];
  /** Replaces the selection (`view.select`: not an undo step). */
  set(ids: readonly string[]): void;
  /** What a right-click on nothing does: the timeline keeps the selection, the bin clears it. */
  readonly empty: "keep" | "clear";
}

export interface MenuSurfaceDef {
  /** The stable surface ID (ADR-0026). */
  readonly id: string;
  /** The menu's accessible name: "Clip actions for {{name}}". */
  readonly name: (target: MenuTarget) => LabelSpec;
  /** The groups in display order; a separator sits between two that have rows. */
  readonly groups: readonly string[];
  readonly rows: readonly MenuRow[];
  readonly selection?: SelectionAdapter;
  /** Context keys derived from the resolved target (the track's lock, the item's link). */
  readonly keys?: (target: MenuTarget) => ContextValues;
}

/** A row added from outside the surface's own table (a feature WP, a plugin). */
export interface MenuContribution {
  readonly surface: string;
  readonly command: string;
  readonly group?: string;
  readonly order: number;
  readonly when?: string;
  /** Shown when `when` does not hold: "Select one clip". First-party rows only. */
  readonly reason?: TKey;
  readonly args?: (ctx: MenuContext) => unknown;
  /** The plugin that adds it. Its items form a trailing group, three at most, then a submenu. */
  readonly plugin?: { readonly id: string; readonly name: string };
  /** Plain text. A plugin supplies its own; a first-party row takes the command's label. */
  readonly label?: string;
  readonly icon?: IconName;
}
