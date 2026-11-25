// SPDX-License-Identifier: GPL-3.0-or-later
// The application menu bar of a custom title bar (WP-1M.09): File, Edit, View, Help drawn in
// the page instead of by the OS. Built on Radix Menubar and behaving like a native Windows menu:
//
// - Alt (pressed and released alone) or F10 moves the focus to the first menu and back; the
//   access letters are underlined while the keyboard drives the bar. Alt+letter opens a menu.
// - Left and Right move between menus (open or not), Down/Enter/Space open one, Escape closes the
//   open menu and a second Escape leaves the bar.
// - A native menu never takes the focus away from the page. Here the open menu holds it, so
//   choosing a command first puts the focus back where it was (a text field, say) and only then
//   runs the command: Cut, Paste or Undo act on that field.
// - While a modal dialog hides the rest of the page (Radix `aria-hidden`), the access keys do
//   nothing, as a native menu bar is disabled while its window shows a modal dialog.
import * as RadixMenubar from "@radix-ui/react-menubar";
import {
  type FocusEvent,
  type KeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { type MenuItem, MenuItemBody, type MenuSeparator, menuItemClass } from "./Menu.tsx";

/** A nested list (File > Open recent). */
export interface MenuBarSubmenu {
  kind: "submenu";
  id: string;
  label: string;
  icon?: IconName | undefined;
  disabled?: boolean | undefined;
  items: readonly (MenuItem | MenuSeparator)[];
}

export type MenuBarEntry = MenuItem | MenuSeparator | MenuBarSubmenu;

export interface MenuBarMenu {
  /** Also the value `value` / `onValueChange` use for this menu. */
  id: string;
  /** One word in sentence case: "File". */
  label: string;
  /** The access letter (Alt+letter); the label's first letter by default. */
  mnemonic?: string | undefined;
  items: readonly MenuBarEntry[];
}

export interface MenuBarProps {
  menus: readonly MenuBarMenu[];
  /** Names the bar, e.g. "Application menu". */
  "aria-label": string;
  /** Alt, F10 and Alt+letter reach the bar from anywhere in the window (default true). */
  accessKeys?: boolean | undefined;
  /** The open menu's id, or `""` when none is open. */
  value?: string | undefined;
  defaultValue?: string | undefined;
  onValueChange?: ((value: string) => void) | undefined;
  className?: string | undefined;
}

/** The access letter of a menu, lower-case. */
function accessLetter(menu: MenuBarMenu): string {
  return (menu.mnemonic ?? menu.label.charAt(0)).toLowerCase();
}

/** The label with its access letter wrapped, so it can be underlined in keyboard mode. */
function TriggerLabel({ menu }: { menu: MenuBarMenu }) {
  const letter = accessLetter(menu);
  const at = menu.label.toLowerCase().indexOf(letter);
  if (letter === "" || at < 0) return <span>{menu.label}</span>;
  return (
    <span>
      {menu.label.slice(0, at)}
      <span className="cw-menubar__key">{menu.label.slice(at, at + 1)}</span>
      {menu.label.slice(at + 1)}
    </span>
  );
}

const MENU_LAYER = "[data-radix-menubar-content]";

function Entries({
  items,
  choose,
}: {
  items: readonly MenuBarEntry[];
  choose: (run: (() => void) | undefined) => void;
}): ReactNode {
  return items.map((entry, index) => {
    if (entry.kind === "separator") {
      return <RadixMenubar.Separator key={entry.id ?? `sep-${index}`} className="cw-menu__sep" />;
    }
    if (entry.kind === "submenu") {
      return (
        <RadixMenubar.Sub key={entry.id}>
          <RadixMenubar.SubTrigger
            className="cw-menu__item body"
            disabled={entry.disabled}
            textValue={entry.label}
          >
            {entry.icon ? <Icon name={entry.icon} size="md" /> : null}
            <span className="cw-menu__label">{entry.label}</span>
            <Icon name="chevron-right" size="md" />
          </RadixMenubar.SubTrigger>
          <RadixMenubar.Portal>
            <RadixMenubar.SubContent className="cw-menu" collisionPadding={8} loop>
              <Entries items={entry.items} choose={choose} />
            </RadixMenubar.SubContent>
          </RadixMenubar.Portal>
        </RadixMenubar.Sub>
      );
    }
    if (entry.checked !== undefined) {
      // A choice (View > Panels): a check mark and `menuitemcheckbox` / `menuitemradio`.
      return (
        <RadixMenubar.CheckboxItem
          key={entry.id}
          textValue={entry.label}
          disabled={entry.disabled}
          checked={entry.checked}
          role={entry.choice === "radio" ? "menuitemradio" : "menuitemcheckbox"}
          onSelect={() => choose(entry.onSelect)}
          className={cx(menuItemClass(entry), "cw-menu__item--check")}
        >
          <span className="cw-menu__check" aria-hidden="true">
            {entry.checked ? <Icon name="check" size="md" /> : null}
          </span>
          <MenuItemBody entry={entry} />
        </RadixMenubar.CheckboxItem>
      );
    }
    return (
      <RadixMenubar.Item
        key={entry.id}
        textValue={entry.label}
        disabled={entry.disabled}
        onSelect={() => choose(entry.onSelect)}
        className={menuItemClass(entry)}
      >
        <MenuItemBody entry={entry} />
      </RadixMenubar.Item>
    );
  });
}

/** Application menu bar for a custom title bar; see the file comment for the keyboard model. */
export function MenuBar({
  menus,
  "aria-label": ariaLabel,
  accessKeys = true,
  value: valueProp,
  defaultValue,
  onValueChange,
  className,
}: MenuBarProps) {
  const [ownValue, setOwnValue] = useState(defaultValue ?? "");
  const value = valueProp ?? ownValue;
  const valueRef = useRef(value);
  valueRef.current = value;
  const setValue = useCallback(
    (next: string) => {
      valueRef.current = next;
      if (valueProp === undefined) setOwnValue(next);
      onValueChange?.(next);
    },
    [valueProp, onValueChange],
  );

  const rootRef = useRef<HTMLDivElement>(null);
  /**
   * The element outside the bar that has (or last had) the focus: where the focus goes back to
   * when the bar is done. Null when the page itself has it (nothing focused).
   */
  const lastOutside = useRef<HTMLElement | null>(null);
  /** A command was chosen and the focus already went back: keep it there on close. */
  const focusRestored = useRef(false);
  /** Underline the access letters (the keyboard drives the bar). */
  const [showKeys, setShowKeys] = useState(false);

  const inside = useCallback((node: unknown): boolean => {
    if (!(node instanceof Node)) return false;
    if (rootRef.current?.contains(node)) return true;
    return node instanceof Element && node.closest(MENU_LAYER) !== null;
  }, []);

  // Follow the focus outside the bar. Moving between menus can pass the focus through the page
  // body for a moment (a closing menu unmounts first); only focus that leaves for, or arrives at,
  // an element outside the bar counts.
  useEffect(() => {
    const outside = (el: EventTarget | null): el is HTMLElement =>
      el instanceof HTMLElement && el !== document.body && !inside(el);
    const active = document.activeElement;
    if (outside(active)) lastOutside.current = active;
    const onFocusIn = (e: globalThis.FocusEvent): void => {
      if (outside(e.target)) lastOutside.current = e.target;
    };
    const onFocusOut = (e: globalThis.FocusEvent): void => {
      if (!outside(e.target)) return;
      // Into the bar: keep it (the focus comes back here). Anywhere else, or nowhere: follow.
      if (e.relatedTarget === null || outside(e.relatedTarget)) {
        lastOutside.current = outside(e.relatedTarget) ? e.relatedTarget : null;
      }
    };
    document.addEventListener("focusin", onFocusIn, true);
    document.addEventListener("focusout", onFocusOut, true);
    return () => {
      document.removeEventListener("focusin", onFocusIn, true);
      document.removeEventListener("focusout", onFocusOut, true);
    };
  }, [inside]);

  /** Gives the focus back to where it was before the bar (or to nothing). */
  const giveFocusBack = useCallback(() => {
    const target = lastOutside.current;
    setShowKeys(false);
    if (target?.isConnected) {
      target.focus({ preventScroll: true });
      return;
    }
    const active = document.activeElement;
    if (active instanceof HTMLElement && inside(active)) active.blur();
  }, [inside]);

  const trigger = useCallback((id: string): HTMLElement | null => {
    const root = rootRef.current;
    if (root === null) return null;
    const all = root.querySelectorAll<HTMLElement>("[data-menu-id]");
    return [...all].find((el) => el.dataset.menuId === id) ?? null;
  }, []);

  /** Alt or F10: into the bar (first menu focused, none open) or back out of it. */
  const toggleBar = useCallback(() => {
    const active = document.activeElement;
    if (valueRef.current !== "" || inside(active)) {
      setValue("");
      giveFocusBack();
      return;
    }
    const first = menus[0];
    if (first === undefined) return;
    setShowKeys(true);
    trigger(first.id)?.focus();
  }, [giveFocusBack, inside, menus, setValue, trigger]);

  /** Alt+letter (or the letter on a focused menu): open that menu with its first command focused. */
  const openFromKeyboard = useCallback(
    (menu: MenuBarMenu) => {
      setShowKeys(true);
      const el = trigger(menu.id);
      if (el === null) return;
      el.focus();
      if (valueRef.current === menu.id) return;
      // Radix opens a menu "from the keyboard" (its first command focused) only from the
      // trigger's own key handler: press Down on the trigger.
      el.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }),
      );
    },
    [trigger],
  );

  const byLetter = useCallback(
    (key: string): MenuBarMenu | undefined =>
      key.length === 1 ? menus.find((m) => accessLetter(m) === key.toLowerCase()) : undefined,
    [menus],
  );

  // Window-wide access keys. Capture phase, so a focused field cannot swallow them; a native
  // menu bar takes Alt and F10 from any control in its window too.
  useEffect(() => {
    if (!accessKeys) return;
    let armed = false;
    /** A modal layer hides the page (and this bar) from assistive tech and the keyboard. */
    const blocked = (): boolean => {
      const root = rootRef.current;
      return root === null || root.closest('[aria-hidden="true"], [inert]') !== null;
    };
    const onKeyDown = (e: globalThis.KeyboardEvent): void => {
      if (e.key === "Alt") {
        // Alt alone arms; Ctrl+Alt (AltGr), Shift+Alt and Meta+Alt do not.
        armed = e.repeat ? armed : !e.ctrlKey && !e.metaKey && !e.shiftKey;
        return;
      }
      armed = false;
      if (blocked()) return;
      if (e.key === "F10" && !e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
        e.preventDefault();
        toggleBar();
        return;
      }
      if (e.altKey && !e.ctrlKey && !e.metaKey) {
        const menu = byLetter(e.key);
        if (menu !== undefined) {
          e.preventDefault();
          openFromKeyboard(menu);
        }
      }
    };
    const onKeyUp = (e: globalThis.KeyboardEvent): void => {
      if (e.key !== "Alt" || !armed) return;
      armed = false;
      if (blocked()) return;
      // Windows would otherwise move the focus to the (hidden) system menu.
      e.preventDefault();
      toggleBar();
    };
    const disarm = (): void => {
      armed = false;
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("pointerdown", disarm, true);
    window.addEventListener("blur", disarm);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("pointerdown", disarm, true);
      window.removeEventListener("blur", disarm);
    };
  }, [accessKeys, byLetter, openFromKeyboard, toggleBar]);

  /** A command was chosen: the focus goes back first, then the command runs. */
  const choose = useCallback(
    (run: (() => void) | undefined) => {
      focusRestored.current = true;
      giveFocusBack();
      run?.();
    },
    [giveFocusBack],
  );

  // React focus events travel through portals, so this also sees focus leaving an open menu.
  const onBlurCapture = (e: FocusEvent<HTMLDivElement>): void => {
    if (e.relatedTarget !== null && !inside(e.relatedTarget)) setShowKeys(false);
  };
  // Keys on a focused (closed) menu; keys inside an open menu belong to Radix.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>): void => {
    if (!(e.target instanceof Node) || !rootRef.current?.contains(e.target)) return;
    if (e.key === "Escape" && valueRef.current === "") {
      e.preventDefault();
      giveFocusBack();
      return;
    }
    if (!e.altKey && !e.ctrlKey && !e.metaKey && e.key.length === 1 && e.key !== " ") {
      // A letter picks a menu; other characters do nothing here (the page's single-key
      // shortcuts wait until the focus is back on the page), as in a native menu bar.
      e.preventDefault();
      const menu = byLetter(e.key);
      if (menu !== undefined) openFromKeyboard(menu);
    }
  };

  return (
    <RadixMenubar.Root
      ref={rootRef}
      aria-label={ariaLabel}
      className={cx("cw-menubar", className)}
      data-access-keys={showKeys ? "" : undefined}
      value={value}
      onValueChange={setValue}
      onBlurCapture={onBlurCapture}
      onKeyDown={onKeyDown}
      onPointerDown={() => setShowKeys(false)}
    >
      {menus.map((menu) => (
        <RadixMenubar.Menu key={menu.id} value={menu.id}>
          <RadixMenubar.Trigger
            className="cw-menubar__trigger body"
            data-menu-id={menu.id}
            aria-keyshortcuts={`Alt+${accessLetter(menu).toUpperCase()}`}
          >
            <TriggerLabel menu={menu} />
          </RadixMenubar.Trigger>
          <RadixMenubar.Portal>
            <RadixMenubar.Content
              className="cw-menu cw-menubar__menu"
              sideOffset={4}
              collisionPadding={8}
              loop
              onCloseAutoFocus={(e) => {
                // The command's target already has the focus back; keep it off the trigger.
                if (!focusRestored.current) return;
                focusRestored.current = false;
                e.preventDefault();
              }}
            >
              <Entries items={menu.items} choose={choose} />
            </RadixMenubar.Content>
          </RadixMenubar.Portal>
        </RadixMenubar.Menu>
      ))}
    </RadixMenubar.Root>
  );
}
