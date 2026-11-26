// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixContextMenu from "@radix-ui/react-context-menu";
import * as RadixDropdownMenu from "@radix-ui/react-dropdown-menu";
import { type ReactNode, useId } from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { Tooltip } from "./Tooltip.tsx";

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
  /**
   * One line saying why a disabled row cannot run ("Track is locked"). Read as the row's
   * description and shown as a tooltip on hover; leave it out for an enabled row.
   */
  disabledReason?: string | undefined;
  /** `agent` is the "Ask agent..." entry point (sparkles, `agent-text`); `danger` is Delete. */
  variant?: "agent" | "danger" | undefined;
  onSelect?: (() => void) | undefined;
  /**
   * Makes the row a choice: `true` shows a check and the row is announced as checked. Use it for
   * settings of a view (filters, sort); commands leave it out.
   */
  checked?: boolean | undefined;
  /** With `checked`: one of a group (`menuitemradio`) instead of an independent option. */
  choice?: "radio" | undefined;
  /** Selecting keeps the menu open, so several options can be set in one visit. */
  keepOpen?: boolean | undefined;
}

export interface MenuSeparator {
  kind: "separator";
  id?: string | undefined;
}

/** A heading for the rows below it ("Kind", "Sort by"); not focusable. */
export interface MenuLabel {
  kind: "label";
  id: string;
  label: string;
}

/** A row of a submenu: commands, separators and headings (the design spec allows one level). */
export type MenuLeaf = MenuItem | MenuSeparator | MenuLabel;

/** A row that opens a nested list ("Move to": Left, Right, Bottom). One level only. */
export interface MenuSubmenu {
  kind: "submenu";
  id: string;
  label: string;
  icon?: IconName | undefined;
  disabled?: boolean | undefined;
  disabledReason?: string | undefined;
  items: readonly MenuLeaf[];
}

export type MenuEntry = MenuLeaf | MenuSubmenu;

export interface MenuProps {
  items: readonly MenuEntry[];
  /**
   * `context` opens on right-click (or the context-menu key) over `children`; `dropdown` opens
   * from a button; `point` is controlled (`open`, `onOpenChange`) and hangs from `anchor`, a
   * viewport point, with no children: the editor's context-menu host uses it for every surface.
   */
  mode?: "context" | "dropdown" | "point";
  /** The anchor: the region to right-click, or the trigger button (a single focusable element). */
  children?: ReactNode;
  /** `point` mode: where the menu opens, in viewport CSS pixels. */
  anchor?: { readonly x: number; readonly y: number } | undefined;
  /**
   * Called when the menu has closed and would return focus to its anchor. Call
   * `event.preventDefault()` to place the focus yourself (the host returns it to the surface).
   */
  onCloseAutoFocus?: ((event: Event) => void) | undefined;
  /** Names the menu, e.g. "Clip actions for interview_A.mov". */
  "aria-label": string;
  /** Dropdown and point modes. */
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

/** Which leading slots a list reserves, so every label shares one column. */
export interface MenuGutters {
  readonly check: boolean;
  readonly icon: boolean;
}

interface GutterEntry {
  id?: string | undefined;
  kind?: string | undefined;
  icon?: IconName | undefined;
  variant?: string | undefined;
  checked?: boolean | undefined;
}

/** A list reserves a check slot if any row is checkable and an icon slot if any row has an icon. */
export function menuGutters(items: readonly GutterEntry[]): MenuGutters {
  let check = false;
  let icon = false;
  for (const e of items) {
    if (e.kind === "separator" || e.kind === "label") continue;
    if (e.checked !== undefined) check = true;
    if (e.icon !== undefined || e.variant === "agent") icon = true;
  }
  return { check, icon };
}

/** Empty slot that keeps a row without an icon aligned with its neighbours. */
export function MenuGutter() {
  return <span className="cw-menu__gutter" aria-hidden="true" />;
}

/** Check slot, icon, label and shortcut of one command row (shared with `MenuBar`). */
export function MenuItemBody({ entry, gutters }: { entry: MenuItem; gutters: MenuGutters }) {
  const icon = entry.icon ?? (entry.variant === "agent" ? "sparkles" : undefined);
  return (
    <>
      {gutters.check ? (
        <span className="cw-menu__check" aria-hidden="true">
          {entry.checked ? <Icon name="check" size="md" /> : null}
        </span>
      ) : null}
      {icon ? <Icon name={icon} size="md" /> : gutters.icon ? <MenuGutter /> : null}
      <Label text={entry.label} reason={entry.disabled ? entry.disabledReason : undefined} />
      {entry.shortcut ? <span className="cw-menu__kbd numeric">{entry.shortcut}</span> : null}
    </>
  );
}

/**
 * A row's text. With a reason it is described by it (`aria-describedby`, for screen readers)
 * and a tooltip shows it on hover; Radix does not focus disabled rows, so the keyboard reaches
 * the reason through the description.
 */
function Label({ text, reason }: { text: string; reason: string | undefined }) {
  const id = useId();
  if (reason === undefined) return <span className="cw-menu__label">{text}</span>;
  return (
    <>
      <Tooltip label={reason} side="right" delayDuration={300}>
        <span className="cw-menu__label" aria-describedby={id}>
          {text}
        </span>
      </Tooltip>
      {/* `hidden` keeps the reason out of the row's name; a description may still point at it. */}
      <span id={id} hidden>
        {reason}
      </span>
    </>
  );
}

/** The rows of a list, for any of the Radix kits. */
function Rows({ kit, items }: { kit: MenuKit; items: readonly MenuEntry[] }): ReactNode {
  const gutters = menuGutters(items);
  return items.map((entry, index) => {
    if (entry.kind === "separator") {
      return <kit.Separator key={entry.id ?? `sep-${index}`} className="cw-menu__sep" />;
    }
    if (entry.kind === "label") {
      return (
        <kit.Label key={entry.id} className="cw-menu__heading label">
          {entry.label}
        </kit.Label>
      );
    }
    if (entry.kind === "submenu") {
      return (
        <kit.Sub key={entry.id}>
          <kit.SubTrigger
            className="cw-menu__item body"
            disabled={entry.disabled}
            textValue={entry.label}
          >
            {gutters.check ? <MenuGutter /> : null}
            {entry.icon ? (
              <Icon name={entry.icon} size="md" />
            ) : gutters.icon ? (
              <MenuGutter />
            ) : null}
            <Label text={entry.label} reason={entry.disabled ? entry.disabledReason : undefined} />
            <Icon name="chevron-right" size="md" />
          </kit.SubTrigger>
          <kit.Portal>
            <kit.SubContent className="cw-menu" collisionPadding={8} loop>
              <Rows kit={kit} items={entry.items} />
            </kit.SubContent>
          </kit.Portal>
        </kit.Sub>
      );
    }
    const select = (event: Event) => {
      if (entry.keepOpen) event.preventDefault();
      entry.onSelect?.();
    };
    if (entry.checked !== undefined) {
      return (
        <kit.CheckboxItem
          key={entry.id}
          textValue={entry.label}
          disabled={entry.disabled}
          checked={entry.checked}
          role={entry.choice === "radio" ? "menuitemradio" : "menuitemcheckbox"}
          onSelect={select}
          className={cx(menuItemClass(entry), "cw-menu__item--check")}
        >
          <MenuItemBody entry={entry} gutters={gutters} />
        </kit.CheckboxItem>
      );
    }
    return (
      <kit.Item
        key={entry.id}
        textValue={entry.label}
        disabled={entry.disabled}
        onSelect={select}
        className={menuItemClass(entry)}
      >
        <MenuItemBody entry={entry} gutters={gutters} />
      </kit.Item>
    );
  });
}

/** Floating command list. Focus is trapped while open and restored to the anchor on close. */
export function Menu({
  items,
  mode = "context",
  children,
  anchor,
  "aria-label": ariaLabel,
  open,
  defaultOpen,
  onOpenChange,
  onCloseAutoFocus,
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
        {...(onCloseAutoFocus ? { onCloseAutoFocus } : {})}
        // Dropdowns hang from the trigger's leading edge (4 = space-1); context menus open at the pointer.
        {...(mode === "dropdown"
          ? { align: "start" as const, sideOffset: 4, collisionPadding: 8 }
          : mode === "point"
            ? { align: "start" as const, sideOffset: 0, collisionPadding: 8 }
            : {})}
      >
        <Rows kit={kit} items={items} />
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
  if (mode === "point") {
    // A zero-size button at the point is the anchor. Not modal: a second right-click elsewhere
    // closes this menu and still reaches the surface under it.
    return (
      <RadixDropdownMenu.Root
        open={open ?? false}
        modal={false}
        {...(onOpenChange ? { onOpenChange } : {})}
      >
        <RadixDropdownMenu.Trigger asChild>
          <button
            type="button"
            tabIndex={-1}
            aria-hidden="true"
            className="cw-menu__anchor"
            style={{ left: anchor?.x ?? 0, top: anchor?.y ?? 0 }}
          />
        </RadixDropdownMenu.Trigger>
        {content}
      </RadixDropdownMenu.Root>
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
