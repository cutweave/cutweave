// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixContextMenu from "@radix-ui/react-context-menu";
import * as RadixDropdownMenu from "@radix-ui/react-dropdown-menu";
import type { ReactNode } from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";

export interface MenuItem {
  kind?: "item";
  id: string;
  /** Verb phrase that matches the command name: "Split at playhead". */
  label: string;
  icon?: IconName | undefined;
  /** Shortcut text from the live keymap, shown as real characters in mono. */
  shortcut?: string | undefined;
  /** Disabled items stay visible (so users learn they exist) and are skipped by the arrows. */
  disabled?: boolean | undefined;
  /** `agent` is the "Ask agent..." entry point (sparkles, `agent-text`); `danger` is Delete. */
  variant?: "agent" | "danger" | undefined;
  onSelect?: (() => void) | undefined;
}

export interface MenuSeparator {
  kind: "separator";
  id?: string | undefined;
}

export type MenuEntry = MenuItem | MenuSeparator;

export interface MenuProps {
  items: readonly MenuEntry[];
  /** `context` opens on right-click (or the context-menu key) over `children`; `dropdown` opens from a button. */
  mode?: "context" | "dropdown";
  /** The anchor: the region to right-click, or the trigger button (a single focusable element). */
  children: ReactNode;
  /** Names the menu, e.g. "Clip actions for interview_A.mov". */
  "aria-label": string;
  /** Dropdown mode only. */
  open?: boolean | undefined;
  defaultOpen?: boolean | undefined;
  onOpenChange?: ((open: boolean) => void) | undefined;
  className?: string | undefined;
}

type MenuKit = typeof RadixContextMenu | typeof RadixDropdownMenu;

/** Class names of one command row (shared with `MenuBar`). */
export function menuItemClass(entry: Pick<MenuItem, "variant">): string {
  return cx("cw-menu__item body", entry.variant && `cw-menu__item--${entry.variant}`);
}

/** Icon, label and shortcut of one command row (shared with `MenuBar`). */
export function MenuItemBody({ entry }: { entry: MenuItem }) {
  const icon = entry.icon ?? (entry.variant === "agent" ? "sparkles" : undefined);
  return (
    <>
      {icon ? <Icon name={icon} size="md" /> : null}
      <span className="cw-menu__label">{entry.label}</span>
      {entry.shortcut ? <span className="cw-menu__kbd numeric">{entry.shortcut}</span> : null}
    </>
  );
}

/** Floating command list. Focus is trapped while open and restored to the anchor on close. */
export function Menu({
  items,
  mode = "context",
  children,
  "aria-label": ariaLabel,
  open,
  defaultOpen,
  onOpenChange,
  className,
}: MenuProps) {
  const kit: MenuKit = mode === "context" ? RadixContextMenu : RadixDropdownMenu;
  const content = (
    <kit.Portal>
      <kit.Content
        aria-label={ariaLabel}
        // Radix labels dropdowns by their trigger; the menu's own name is more specific.
        aria-labelledby={undefined}
        className={cx("cw-menu", className)}
        loop
        // Dropdowns hang from the trigger's leading edge (4 = space-1); context menus open at the pointer.
        {...(mode === "dropdown"
          ? { align: "start" as const, sideOffset: 4, collisionPadding: 8 }
          : {})}
      >
        {items.map((entry, index) => {
          if (entry.kind === "separator") {
            return <kit.Separator key={entry.id ?? `sep-${index}`} className="cw-menu__sep" />;
          }
          return (
            <kit.Item
              key={entry.id}
              textValue={entry.label}
              disabled={entry.disabled}
              onSelect={() => entry.onSelect?.()}
              className={menuItemClass(entry)}
            >
              <MenuItemBody entry={entry} />
            </kit.Item>
          );
        })}
      </kit.Content>
    </kit.Portal>
  );

  if (mode === "context") {
    return (
      <RadixContextMenu.Root {...(onOpenChange ? { onOpenChange } : {})}>
        <RadixContextMenu.Trigger asChild>{children}</RadixContextMenu.Trigger>
        {content}
      </RadixContextMenu.Root>
    );
  }
  return (
    <RadixDropdownMenu.Root
      open={open}
      defaultOpen={defaultOpen}
      {...(onOpenChange ? { onOpenChange } : {})}
    >
      <RadixDropdownMenu.Trigger asChild>{children}</RadixDropdownMenu.Trigger>
      {content}
    </RadixDropdownMenu.Root>
  );
}
