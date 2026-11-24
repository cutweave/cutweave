// SPDX-License-Identifier: GPL-3.0-or-later
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { Tooltip } from "./Tooltip.tsx";

export type StatusTone = "success" | "warning" | "danger" | "neutral";

const TONE_ICON: Record<StatusTone, IconName> = {
  success: "check",
  warning: "triangle-alert",
  danger: "circle-alert",
  neutral: "info",
};

export interface StatusChipProps {
  tone: StatusTone;
  /** One to three words, sentence case: "Exported", "Missing media". */
  children: string;
  /**
   * The tone's icon by default. Set it when states of one tone must still read apart without
   * colour (connecting vs. not connected).
   */
  icon?: IconName | undefined;
  /**
   * Icon only, for tight bars such as the title bar: the words become a tooltip and stay in the
   * text for screen readers. Give each state its own icon (`icon`), so the shape still tells them
   * apart.
   */
  iconOnly?: boolean | undefined;
  className?: string | undefined;
}

/**
 * Status as icon plus word, never colour alone. Static: it takes no focus. Icon-only chips show
 * the word as a tooltip on hover.
 */
export function StatusChip({ tone, children, icon, iconOnly = false, className }: StatusChipProps) {
  const chip = (
    <span
      className={cx("cw-status label", iconOnly && "cw-status--icon", className)}
      data-tone={tone}
    >
      <Icon name={icon ?? TONE_ICON[tone]} size="sm" />
      {iconOnly ? <span className="cw-visually-hidden">{children}</span> : children}
    </span>
  );
  return iconOnly ? <Tooltip label={children}>{chip}</Tooltip> : chip;
}
