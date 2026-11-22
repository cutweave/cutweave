// SPDX-License-Identifier: GPL-3.0-or-later
import type { SVGProps } from "react";
import { CUSTOM_ICONS, type CustomIconName } from "./custom.tsx";
import { LUCIDE_ICONS, type LucideIconName, type SvgComponent } from "./lucide-set.ts";

export type IconName = LucideIconName | CustomIconName;
export type IconSize = "sm" | "md" | "lg";

export const CUSTOM_ICON_NAMES = Object.keys(CUSTOM_ICONS) as CustomIconName[];

/** Every icon name, Lucide first then custom glyphs. Drives the icon gallery story (WP-0K.05). */
export const ICON_NAMES: readonly IconName[] = [
  ...(Object.keys(LUCIDE_ICONS) as LucideIconName[]),
  ...CUSTOM_ICON_NAMES,
];

/** CSS size token for each `IconSize` (icon-sm, icon-md, icon-lg in design/tokens.json). */
export const ICON_SIZE_TOKENS: Record<IconSize, string> = {
  sm: "var(--icon-sm)",
  md: "var(--icon-md)",
  lg: "var(--icon-lg)",
};

/** Stroke on the 24-unit grid; scales with the icon (design/README.md, Iconography). */
export const ICON_STROKE_WIDTH = 1.75;

export interface IconProps extends Omit<SVGProps<SVGSVGElement>, "ref" | "name"> {
  name: IconName;
  /** `sm` inside clips and chips, `md` in panels and menus (default), `lg` in toolbar and transport. */
  size?: IconSize;
  /** Accessible name. Omit for decorative icons, which are `aria-hidden`. */
  label?: string;
}

function resolve(name: IconName): SvgComponent {
  if (name in CUSTOM_ICONS) return CUSTOM_ICONS[name as CustomIconName];
  return LUCIDE_ICONS[name as LucideIconName];
}

export function Icon({ name, size = "md", label, style, ...rest }: IconProps) {
  const Cmp = resolve(name);
  const dim = ICON_SIZE_TOKENS[size];
  const a11y =
    label === undefined
      ? ({ "aria-hidden": true } as const)
      : ({ role: "img", "aria-label": label } as const);
  return (
    <Cmp
      data-icon={name}
      data-size={size}
      focusable="false"
      strokeWidth={ICON_STROKE_WIDTH}
      {...rest}
      {...a11y}
      style={{ width: dim, height: dim, flexShrink: 0, ...style }}
    />
  );
}
