// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixTooltip from "@radix-ui/react-tooltip";
import type { HTMLAttributes, ReactElement, Ref } from "react";
import { usePortalContainer } from "./PortalContainer.ts";

export interface TooltipProps
  extends Omit<HTMLAttributes<HTMLElement>, "children" | "title" | "color"> {
  /** Verb phrase naming the action: "Split at playhead". */
  label: string;
  /** Current shortcut from the keymap, one cap per key: ["Shift", "Backspace"]. */
  keys?: readonly string[] | undefined;
  /** The anchor. A single focusable element that still carries its own accessible name. */
  children: ReactElement;
  side?: "top" | "right" | "bottom" | "left" | undefined;
  /** Hover delay in ms before showing; keyboard focus shows immediately. */
  delayDuration?: number | undefined;
  /**
   * Props from a parent that wraps this tooltip as its trigger (a dropdown `Menu` around a
   * `Tooltip` around a button) reach the button, not this component.
   */
  ref?: Ref<HTMLButtonElement> | undefined;
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
  ...triggerProps
}: TooltipProps) {
  // In a detached panel's window the tooltip shows there (`PortalContainerProvider`).
  const container = usePortalContainer();
  return (
    <RadixTooltip.Provider delayDuration={delayDuration}>
      <RadixTooltip.Root>
        <RadixTooltip.Trigger asChild {...triggerProps}>
          {children}
        </RadixTooltip.Trigger>
        <RadixTooltip.Portal container={container ?? undefined}>
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
