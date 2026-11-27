// SPDX-License-Identifier: GPL-3.0-or-later
// The colour labels a clip or a marker can carry (issue #222). The choices are the design
// system's `label-*` tokens; the project stores the token's value (`#rrggbb`), which is the same
// in both themes, so a stored colour names its label again when the menu or the dialog opens.
import type { TKey } from "../i18n/index.ts";
import { activeDocument } from "../shell/windows.ts";

export const LABEL_COLORS = [
  "red",
  "orange",
  "lime",
  "green",
  "teal",
  "blue",
  "purple",
  "pink",
] as const;

export type LabelColor = (typeof LABEL_COLORS)[number];

export const LABEL_COLOR_NAME: Readonly<Record<LabelColor, TKey>> = {
  red: "contextMenu.labelColor.red",
  orange: "contextMenu.labelColor.orange",
  lime: "contextMenu.labelColor.lime",
  green: "contextMenu.labelColor.green",
  teal: "contextMenu.labelColor.teal",
  blue: "contextMenu.labelColor.blue",
  purple: "contextMenu.labelColor.purple",
  pink: "contextMenu.labelColor.pink",
};

/** The CSS custom property of a label colour, for `var()`. */
export const labelColorVar = (name: LabelColor): string => `--label-${name}`;

/**
 * The value of a label colour as the document stores it (`#rrggbb`, lowercase), read from the
 * stylesheet of the app's document; `null` when the token is not there.
 */
export function labelColorValue(name: LabelColor): string | null {
  const doc = activeDocument();
  const value = doc.defaultView
    ?.getComputedStyle(doc.documentElement)
    .getPropertyValue(labelColorVar(name))
    .trim()
    .toLowerCase();
  return value !== undefined && /^#[0-9a-f]{6}$/.test(value) ? value : null;
}

/** Which label colour a stored colour is, or `null` for none or a colour that is not a label. */
export function labelColorOf(color: string | null | undefined): LabelColor | null {
  if (color === null || color === undefined) return null;
  const wanted = color.toLowerCase();
  return LABEL_COLORS.find((name) => labelColorValue(name) === wanted) ?? null;
}
