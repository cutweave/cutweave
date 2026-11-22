// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixTooltip from "@radix-ui/react-tooltip";
import type { ReactElement } from "react";

export interface TooltipProps {
  /** Verb phrase naming the action: "Split at playhead". */
  label: string;
  /** Current shortcut from the keymap, one cap per key: ["Shift", "Backspace"]. */
  keys?: readonly string[] | undefined;
  /** The anchor. A single focusable element that still carries its own accessible name. */
  children: ReactElement;
  side?: "top" | "right" | "bottom" | "left" | undefined;
  /** Hover delay in ms before showing; keyboard focus shows immediately. */
  delayDuration?: number | undefined;
}

/** Shares the hover delay between tooltips; optional, `Tooltip` works without it. */
export const TooltipProvider = RadixTooltip.Provider;

/**
 * Names a control on hover or focus, with its shortcut in mono key caps. Escape or blur hides it.
 * Never holds interactive content.
 */
export function Tooltip({
  label,
  keys,
  children,
  side = "bottom",
  delayDuration = 500,
}: TooltipProps) {
  return (
    <RadixTooltip.Provider delayDuration={delayDuration}>
      <RadixTooltip.Root>
        <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
        <RadixTooltip.Portal>
          <RadixTooltip.Content side={side} className="cw-tooltip label">
            {label}
            {keys?.map((key) => (
              <kbd key={key} className="cw-kbd numeric">
                {key}
              </kbd>
            ))}
          </RadixTooltip.Content>
        </RadixTooltip.Portal>
      </RadixTooltip.Root>
    </RadixTooltip.Provider>
  );
}
